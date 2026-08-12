//! # garden-render — GPU renderer for Garden
//!
//! Draws a [`Scene`] of primitives — solid quads and monospace text runs —
//! using `wgpu` (quads) and `glyphon` (text shaping + glyph atlas). The
//! renderer knows nothing about editors or layout; callers build a fresh
//! `Scene` whenever state changes and call [`Renderer::render`]. There is no
//! animation loop.
//!
//! Renderers are built on a shared [`GpuContext`] — one wgpu
//! instance/adapter/device/queue for the whole process — while pipelines, the
//! glyph atlas, and the render target stay **per-renderer**:
//!
//! - [`Renderer`] — bound to a winit window, presents frames to its surface
//!   (and can also [`capture`](Renderer::capture) offscreen). A second window
//!   reuses the first renderer's context ([`Renderer::gpu_context`] +
//!   [`Renderer::with_context`]) instead of creating another device.
//! - [`HeadlessRenderer`] — no window or surface at all; renders scenes into
//!   an offscreen texture and reads them back. Used by the headless run mode
//!   for the debug server's `/screenshot` endpoint.
//!
//! Coordinates are in **logical pixels** (the same units winit reports for
//! logical sizes); the renderer multiplies by the scale factor internally.
//! See `docs/architecture.md` in the workspace root for the crate contract.

mod globals;
mod image;
mod mesh;
mod quad;
mod text;

pub use text::{AtlasStats, FONT_SIZE, LINE_HEIGHT_RATIO};

use std::sync::Arc;

use image::ImagePipeline;
use mesh::MeshPipeline;
use quad::QuadPipeline;
use text::{TextRun, TextStack};
use winit::window::Window;

/// An RGBA color with components in `0.0..=1.0`, in **sRGB space** (the
/// values you'd read off a hex color picker). The renderer converts to
/// linear space before writing to the sRGB surface, so colors appear on
/// screen exactly as specified.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// Opaque color from RGB components.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Color {
        Color { r, g, b, a: 1.0 }
    }

    /// Color from RGBA components.
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    /// RGB in linear space (alpha unchanged), for writing to an sRGB target.
    pub(crate) fn to_linear(self) -> [f32; 4] {
        [
            srgb_to_linear(self.r),
            srgb_to_linear(self.g),
            srgb_to_linear(self.b),
            self.a,
        ]
    }
}

/// One sRGB channel to linear (the exact piecewise transfer function).
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// An axis-aligned rectangle in logical pixels; `(x, y)` is the top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Rectangle from origin and size.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// Whether `(x, y)` lies inside the rectangle — left/top edges inclusive,
    /// right/bottom exclusive (the usual half-open hit-test convention).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// One vertex of a [`Primitive::Mesh`]: a position in logical pixels and an
/// sRGB color (the renderer linearizes before writing, exactly like quads).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub pos: (f32, f32),
    pub color: Color,
}

impl Vertex {
    pub const fn new(pos: (f32, f32), color: Color) -> Vertex {
        Vertex { pos, color }
    }
}

/// The typographic axes a text run can vary beyond its size. Panel scripts
/// set these through the petal-ui draw protocol; everything Garden itself
/// draws uses the default (the embedded face, regular, upright, unspaced).
///
/// Weight and slant are requests to the shaper: with only JetBrains Mono
/// Regular embedded, cosmic-text answers a bold request with the regular face
/// unless the machine has a matching one installed. Letter-spacing, by
/// contrast, is applied by the caller placing glyphs — it always takes effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// CSS numeric weight, 100–900 (400 regular, 700 bold).
    pub weight: u16,
    pub italic: bool,
    /// Letter-spacing in logical px, added after each glyph.
    pub spacing: f32,
}

/// CSS regular weight — what every run Garden itself draws uses.
pub const REGULAR_WEIGHT: u16 = 400;

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            weight: REGULAR_WEIGHT,
            italic: false,
            spacing: 0.0,
        }
    }
}

/// One drawable element of a [`Scene`].
pub enum Primitive {
    /// Solid rectangle (backgrounds, cursors; borders are built from 4 thin
    /// quads by the caller).
    Quad { rect: Rect, color: Color },
    /// One run of monospace text starting at `pos` (top-left of the first
    /// glyph's line box), clipped to `clip`. `size` is the font size in
    /// logical pixels — [`FONT_SIZE`] for editor/chrome text; panel scripts
    /// choose their own per run. `style` carries the typographic axes beyond
    /// size; editor and chrome text leave it at
    /// [`TextStyle::default`] (regular, upright).
    Text {
        pos: (f32, f32),
        text: String,
        color: Color,
        clip: Rect,
        size: f32,
        style: TextStyle,
    },
    /// A flat-shaded triangle list (`vertices.len()` is a multiple of 3),
    /// scissored to `clip`. The general geometry primitive: callers tessellate
    /// lines/circles/polygons into triangles on the CPU. Unlike [`Quad`],
    /// clipping is a real GPU scissor — a triangle can't be CPU-clipped to a
    /// rect — so a caller that needs containment passes the bounding `clip`.
    Mesh { vertices: Vec<Vertex>, clip: Rect },
    /// A PNG bitmap loaded from `source`, scaled to `rect`, and scissored to
    /// `clip`. Relative sources resolve from Garden's working directory.
    Image {
        rect: Rect,
        source: String,
        alpha: f32,
        clip: Rect,
    },
}

/// Everything needed to draw one frame: a background clear color plus an
/// ordered list of primitives.
///
/// **Painter's order holds across the whole list**: a primitive later in
/// `primitives` composites over an earlier one, whatever their kinds. The list
/// is split into maximal runs of the same kind and each run is drawn by its own
/// pipeline at its own point in the pass — quads instanced, meshes and images
/// one scissored draw per `clip` group, and each stretch of text through its own
/// glyphon renderer (all sharing one atlas). Order *within* a mesh is its
/// triangle order.
///
/// Interleaving costs one pipeline switch per run, and scenes alternate only a
/// handful of times (a panel's shapes tessellate into a few meshes), so this is
/// a few extra draw calls, not one per primitive.
pub struct Scene {
    pub bg: Color,
    pub primitives: Vec<Primitive>,
}

/// A frame read back from the GPU: tightly packed RGBA8 pixels (sRGB-encoded
/// bytes), `width * height * 4` long, in physical pixels.
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Monospace cell metrics — `(advance_width, line_height)` in logical pixels —
/// measured by shaping the embedded font on the CPU. No GPU setup required,
/// so frontends that may never render (headless, terminal) can still use the
/// same layout math as the windowed renderer.
pub fn cell_metrics() -> (f32, f32) {
    text::measure_cell_standalone()
}

/// Per-codepoint advance ratios (glyph advance ÷ font size) for ASCII, measured
/// from the embedded font on the CPU — the table petal-ui's `text_width` sums
/// so a Petal panel script measures text the way this renderer draws it.
/// Ratios, not pixel widths, so one table serves every font size.
pub fn ascii_advance_ratios() -> Vec<f64> {
    text::measure_ascii_advances_standalone()
}

/// The shared GPU handles every renderer is built on: one wgpu
/// instance/adapter/device/queue for the whole process. Renderers layer their
/// own pipelines, glyph atlas, and render target on top — per-frame buffers
/// are grow-only and must **not** be shared, or two windows rendering in the
/// same tick would corrupt each other's frames — but they all submit to this
/// one device/queue.
///
/// All four wgpu handles are internally reference-counted, so `GpuContext` is
/// [`Clone`] and clones are cheap handle copies of the same GPU state.
#[derive(Clone)]
pub struct GpuContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl GpuContext {
    /// Create a context with no display handle, blocking on GPU setup. The
    /// adapter is requested without a compatible surface, so this suits
    /// offscreen rendering ([`HeadlessRenderer`]) only. Returns an error
    /// instead of panicking — a headless session can keep running without a
    /// GPU; only screenshots become unavailable.
    pub fn new_headless() -> Result<GpuContext, String> {
        pollster::block_on(Self::new_headless_async())
    }

    /// Async form of [`new_headless`](Self::new_headless).
    pub async fn new_headless_async() -> Result<GpuContext, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|err| format!("no suitable GPU adapter: {err}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(|err| format!("failed to create wgpu device: {err}"))?;
        Ok(GpuContext {
            instance,
            adapter,
            device,
            queue,
        })
    }
}

/// How many samples the scene pass renders at.
///
/// Garden's shapes are plain tessellated triangles — a circle is a fan, a line
/// is an extruded quad, a rounded corner is a fan of its own — carrying no
/// coverage information of their own. At one sample every diagonal, ring and
/// corner therefore rasterizes with binary coverage and visibly steps, which is
/// exactly what a panel drawing meters, rings and hand-drawn glyphs is made of.
/// Text was never affected: glyphon's atlas is antialiased already.
///
/// 4× is the level every desktop backend supports for a renderable format, and
/// [`supported_samples`] falls back to 1 rather than failing if an adapter
/// disagrees — a jagged frame beats no frame.
const WANTED_SAMPLES: u32 = 4;

/// [`WANTED_SAMPLES`] if this adapter can multisample `format`, else 1.
fn supported_samples(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> u32 {
    if adapter
        .get_texture_format_features(format)
        .flags
        .sample_count_supported(WANTED_SAMPLES)
    {
        WANTED_SAMPLES
    } else {
        1
    }
}

/// Per-renderer GPU state: device/queue handles (shared via [`GpuContext`]),
/// the quad and text pipelines, and the current render-target geometry
/// (physical pixels + scale factor).
struct GpuCore {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    scale_factor: f64,
    quads: QuadPipeline,
    meshes: MeshPipeline,
    images: ImagePipeline,
    text: TextStack,
    /// The multisampled color target the scene pass draws into, resolved into
    /// the surface (or the capture texture) at the end of the pass. `None` when
    /// `samples == 1`, in which case the pass draws to the target directly.
    /// Built on demand and rebuilt when the target size changes.
    samples: u32,
    msaa: Option<wgpu::TextureView>,
    msaa_size: (u32, u32),
    /// The current scene split into consecutive same-kind runs, in scene
    /// order. Recording walks this so painter's order holds across primitive
    /// kinds — see [`Scene`].
    batches: Vec<Batch>,
    /// Index ranges of the text batches, positionally matching the
    /// `BatchKind::Text` entries in `batches`.
    text_batches: Vec<std::ops::Range<usize>>,
}

/// Which pipeline draws a [`Batch`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BatchKind {
    Quad,
    Mesh,
    Image,
    Text,
}

/// One maximal run of consecutive same-kind primitives, as a half-open range
/// of indices *within that kind's* staged list. Each pipeline stages in scene
/// order, so these ranges address it directly.
#[derive(Clone, Copy, Debug)]
struct Batch {
    kind: BatchKind,
    start: usize,
    end: usize,
}

/// Split `primitives` into maximal runs of the same kind, numbering each run
/// against a per-kind running count.
fn batch_primitives(primitives: &[Primitive]) -> Vec<Batch> {
    let mut batches: Vec<Batch> = Vec::new();
    // Next unused index for each kind, in `BatchKind` order.
    let mut next = [0usize; 4];
    for p in primitives {
        let kind = match p {
            Primitive::Quad { .. } => BatchKind::Quad,
            Primitive::Mesh { .. } => BatchKind::Mesh,
            Primitive::Image { .. } => BatchKind::Image,
            Primitive::Text { .. } => BatchKind::Text,
        };
        let slot = &mut next[kind as usize];
        let index = *slot;
        *slot += 1;
        match batches.last_mut() {
            Some(last) if last.kind == kind => last.end = index + 1,
            _ => batches.push(Batch {
                kind,
                start: index,
                end: index + 1,
            }),
        }
    }
    batches
}

impl GpuCore {
    fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        scale_factor: f64,
        samples: u32,
    ) -> GpuCore {
        let quads = QuadPipeline::new(&device, format, samples);
        let meshes = MeshPipeline::new(&device, format, samples);
        let images = ImagePipeline::new(&device, format, samples);
        let text = TextStack::new(&device, &queue, format, samples);
        GpuCore {
            device,
            queue,
            format,
            width,
            height,
            scale_factor,
            quads,
            meshes,
            images,
            text,
            samples,
            msaa: None,
            msaa_size: (0, 0),
            batches: Vec::new(),
            text_batches: Vec::new(),
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
    }

    /// Make sure the multisampled target matches the current size. Called from
    /// [`prepare_scene`](Self::prepare_scene), which both the present and the
    /// capture path run before recording, so neither can reach the pass with a
    /// stale attachment after a resize.
    fn ensure_msaa(&mut self) {
        if self.samples == 1 {
            return;
        }
        let size = (self.width.max(1), self.height.max(1));
        if self.msaa.is_some() && self.msaa_size == size {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("garden msaa target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: self.samples,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.msaa = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.msaa_size = size;
    }

    /// Stage `scene` for drawing at the current target size: upload quad
    /// instances and shape/upload text runs. Shared by present and capture.
    fn prepare_scene(&mut self, scene: &Scene) {
        self.ensure_msaa();
        let scale_factor = self.scale_factor as f32;
        let physical = (self.width, self.height);
        let logical = (
            physical.0 as f32 / scale_factor,
            physical.1 as f32 / scale_factor,
        );

        self.quads.prepare(
            &self.device,
            &self.queue,
            logical,
            scene.primitives.iter().filter_map(|p| match p {
                Primitive::Quad { rect, color } => Some((rect, color)),
                Primitive::Text { .. } | Primitive::Mesh { .. } | Primitive::Image { .. } => None,
            }),
        );

        self.meshes.prepare(
            &self.device,
            &self.queue,
            logical,
            scene.primitives.iter().filter_map(|p| match p {
                Primitive::Mesh { vertices, clip } => Some((vertices.as_slice(), clip)),
                Primitive::Quad { .. } | Primitive::Text { .. } | Primitive::Image { .. } => None,
            }),
        );

        self.images.prepare(
            &self.device,
            &self.queue,
            logical,
            scene.primitives.iter().filter_map(|p| match p {
                Primitive::Image {
                    rect,
                    source,
                    alpha,
                    clip,
                } => Some((rect, source.as_str(), *alpha, clip)),
                Primitive::Quad { .. } | Primitive::Text { .. } | Primitive::Mesh { .. } => None,
            }),
        );

        let texts: Vec<TextRun<'_>> = scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::Text {
                    pos,
                    text,
                    color,
                    clip,
                    size,
                    style,
                } => Some(TextRun {
                    pos: *pos,
                    text,
                    color: *color,
                    clip: *clip,
                    size: *size,
                    style: *style,
                }),
                Primitive::Quad { .. } | Primitive::Mesh { .. } | Primitive::Image { .. } => None,
            })
            .collect();
        self.batches = batch_primitives(&scene.primitives);
        self.text_batches = self
            .batches
            .iter()
            .filter(|b| b.kind == BatchKind::Text)
            .map(|b| b.start..b.end)
            .collect();
        self.text.prepare(
            &self.device,
            &self.queue,
            physical,
            scale_factor,
            &texts,
            &self.text_batches,
        );
    }

    /// Record the scene pass (clear + quads + text) staged by
    /// [`prepare_scene`](Self::prepare_scene) into `view`.
    fn record_scene_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        bg: Color,
    ) {
        // With MSAA the pass draws into the multisampled texture and resolves
        // into `target` on the way out; without it, straight into `target`. The
        // multisampled surface itself is never read afterwards, so it is
        // discarded rather than stored — the resolve already happened.
        let (view, resolve_target) = match &self.msaa {
            Some(msaa) => (msaa, Some(target)),
            None => (target, None),
        };
        let store = if resolve_target.is_some() {
            wgpu::StoreOp::Discard
        } else {
            wgpu::StoreOp::Store
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("garden scene pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    // Clear colors are written to the sRGB target as
                    // linear values, so convert like everything else.
                    load: wgpu::LoadOp::Clear({
                        let [r, g, b, a] = bg.to_linear();
                        wgpu::Color {
                            r: r as f64,
                            g: g as f64,
                            b: b as f64,
                            a: a as f64,
                        }
                    }),
                    store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        // Walk the scene's batches in order so a quad drawn after a text run
        // covers it, the way the primitive list says it should.
        let physical = (self.width, self.height);
        let scale = self.scale_factor as f32;
        let mut text_batch = 0usize;
        for batch in &self.batches {
            match batch.kind {
                BatchKind::Quad => self
                    .quads
                    .render(&mut pass, batch.start as u32..batch.end as u32),
                BatchKind::Mesh => {
                    self.meshes
                        .render(&mut pass, physical, scale, batch.start..batch.end)
                }
                BatchKind::Image => {
                    self.images
                        .render(&mut pass, physical, scale, batch.start..batch.end)
                }
                BatchKind::Text => {
                    self.text.render_batch(text_batch, &mut pass);
                    text_batch += 1;
                }
            }
        }
    }

    /// Render `scene` into an offscreen texture at the current target size
    /// and read it back as tightly packed RGBA8 pixels (sRGB-encoded bytes,
    /// ready for PNG). Blocks until the GPU readback completes.
    fn capture(&mut self, scene: &Scene) -> Capture {
        let (width, height) = (self.width.max(1), self.height.max(1));
        self.prepare_scene(scene);

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("garden capture target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Rows in the readback buffer must be 256-byte aligned.
        let unpadded_row = width as usize * 4;
        let padded_row = unpadded_row.div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("garden capture readback"),
            size: (padded_row * height as usize) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("garden capture"),
            });
        self.record_scene_pass(&mut encoder, &view, scene.bg);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row as u32),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));
        self.text.end_frame();

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("wgpu poll failed during capture");
        rx.recv()
            .expect("capture map_async callback dropped")
            .expect("failed to map capture readback buffer");

        let bgra = matches!(
            self.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let data = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity(unpadded_row * height as usize);
        for row in data.chunks(padded_row) {
            let row = &row[..unpadded_row];
            if bgra {
                for px in row.chunks_exact(4) {
                    rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            } else {
                rgba.extend_from_slice(row);
            }
        }
        drop(data);
        readback.unmap();

        Capture {
            width,
            height,
            rgba,
        }
    }
}

/// Outcome of a [`Renderer::render`] call.
///
/// `render` never re-schedules its own redraw. When the surface is temporarily
/// unavailable (occluded, asleep, outdated) it returns [`FrameOutcome::Skipped`]
/// and leaves retry timing to the frontend, which retries on its throttled poll
/// cadence. This is deliberate: the renderer used to call
/// `window.request_redraw()` on every unavailable frame, but a pending redraw is
/// dispatched immediately (it does not honor the event loop's `WaitUntil`), so a
/// persistently unavailable surface — e.g. a window left occluded while the
/// display sleeps overnight — spun an unthrottled redraw loop that pinned the
/// CPU and leaked un-presented GPU drawables, growing to tens of GB.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// The frame was drawn and presented.
    Presented,
    /// The surface was unavailable; no frame was presented. The frontend should
    /// retry later on its own cadence rather than busy-looping.
    Skipped,
}

/// GPU renderer bound to one winit window.
///
/// Construction blocks on wgpu adapter/device setup (via `pollster`); after
/// that, call [`resize`](Renderer::resize) on window resize events and
/// [`render`](Renderer::render) on redraw requests.
pub struct Renderer {
    context: GpuContext,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    core: GpuCore,
    // Dropped after the surface (field order matters: the surface borrows
    // the window via Arc, and winit/wgpu interact badly if the window dies
    // while a surface still exists).
    window: Arc<Window>,
}

impl Renderer {
    /// Create a renderer for `window`, blocking on GPU setup.
    ///
    /// # Panics
    /// Panics if no suitable GPU adapter or surface can be created — there is
    /// nothing useful the windowed app can do without one.
    pub fn new(window: Arc<Window>) -> Renderer {
        pollster::block_on(Self::new_async(window))
    }

    async fn new_async(window: Arc<Window>) -> Renderer {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(window.clone()),
        ));
        let surface = instance
            .create_surface(window.clone())
            .expect("failed to create wgpu surface");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no suitable GPU adapter found");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("failed to create wgpu device");
        let context = GpuContext {
            instance,
            adapter,
            device,
            queue,
        };

        Self::configure_for_window(context, surface, window)
            .expect("failed to configure wgpu surface")
    }

    /// Create a renderer for `window` on an existing shared [`GpuContext`]
    /// instead of setting up a fresh instance/adapter/device — how a second
    /// window reuses the first window's GPU. The surface comes from the
    /// context's instance; only the per-window pipelines, glyph atlas, and
    /// surface configuration are created here.
    ///
    /// Errors if the context can't create surfaces (a headless context has no
    /// display handle) or if the context's adapter can't present to this
    /// window's surface.
    pub fn with_context(context: &GpuContext, window: Arc<Window>) -> Result<Renderer, String> {
        let surface = context
            .instance
            .create_surface(window.clone())
            .map_err(|err| format!("failed to create wgpu surface: {err}"))?;
        Self::configure_for_window(context.clone(), surface, window)
    }

    /// Shared tail of [`new_async`](Self::new_async) and
    /// [`with_context`](Self::with_context): pick the surface format,
    /// configure the surface, and build the per-window [`GpuCore`] on the
    /// context's device/queue.
    fn configure_for_window(
        context: GpuContext,
        surface: wgpu::Surface<'static>,
        window: Arc<Window>,
    ) -> Result<Renderer, String> {
        let physical_size = window.inner_size();

        // Empty capabilities mean the adapter can't present to this surface
        // (possible when the surface didn't pick the adapter, as in
        // `with_context`).
        let caps = surface.get_capabilities(&context.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| "surface is incompatible with the GPU adapter".to_string())?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: physical_size.width.max(1),
            height: physical_size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&context.device, &config);

        let core = GpuCore::new(
            context.device.clone(),
            context.queue.clone(),
            format,
            config.width,
            config.height,
            window.scale_factor(),
            supported_samples(&context.adapter, format),
        );

        Ok(Renderer {
            context,
            surface,
            config,
            core,
            window,
        })
    }

    /// The shared [`GpuContext`] this renderer runs on. Hand it to
    /// [`Renderer::with_context`] / [`HeadlessRenderer::with_context`] to
    /// build further renderers on the same device instead of creating
    /// another one.
    pub fn gpu_context(&self) -> GpuContext {
        self.context.clone()
    }

    /// One line describing the GPU this renderer actually got, e.g.
    /// `Apple M2 Pro (Metal, IntegratedGpu, vsync)`.
    ///
    /// Worth being able to check rather than assume: everything here is built
    /// for a real GPU — smooth scrolling in particular leans on the rasterizer
    /// placing quads and glyphs at fractional pixel positions every frame —
    /// and wgpu will silently fall back to a software adapter (`Cpu`) when no
    /// hardware one is available, at which point frames get expensive instead
    /// of wrong. The present mode is included because it is the other half of
    /// how a scroll *feels*: `Fifo` is vsync, so frames land on the display's
    /// own cadence rather than tearing.
    pub fn adapter_description(&self) -> String {
        let info = self.context.adapter.get_info();
        format!(
            "{} ({:?}, {:?}, {:?})",
            info.name, info.backend, info.device_type, self.config.present_mode
        )
    }

    /// Resize the surface to a new physical (pixel) size.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.core.device, &self.config);
        self.core.resize(width, height);
    }

    /// Draw one frame: clear to `scene.bg`, then all quads (one instanced
    /// draw call), then all text runs.
    ///
    /// If the surface is temporarily unavailable (occluded, outdated, lost),
    /// the frame is skipped and [`FrameOutcome::Skipped`] is returned. The
    /// renderer does **not** request its own redraw — see [`FrameOutcome`] for
    /// why that would busy-loop; the frontend retries on its throttled cadence.
    pub fn render(&mut self, scene: &Scene) -> FrameOutcome {
        self.core.scale_factor = self.window.scale_factor();
        self.core.prepare_scene(scene);

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return FrameOutcome::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.core.device, &self.config);
                return FrameOutcome::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .context
                    .instance
                    .create_surface(self.window.clone())
                    .expect("failed to recreate wgpu surface");
                self.surface.configure(&self.core.device, &self.config);
                return FrameOutcome::Skipped;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                panic!("wgpu surface validation error")
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.core
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("garden frame"),
                });
        self.core.record_scene_pass(&mut encoder, &view, scene.bg);

        self.core.queue.submit(Some(encoder.finish()));
        frame.present();
        self.core.text.end_frame();
        FrameOutcome::Presented
    }

    /// Render `scene` into an offscreen texture at the current window size
    /// and read it back. Used by the debug server's screenshot endpoint, so
    /// it works without screen-recording permissions.
    pub fn capture(&mut self, scene: &Scene) -> Capture {
        self.core.scale_factor = self.window.scale_factor();
        self.core.capture(scene)
    }

    /// Monospace cell metrics at the configured font size:
    /// `(advance_width, line_height)` in logical pixels. garden-app uses this
    /// for all layout math (cursor x = col * advance, click→col = x / advance).
    pub fn cell_size(&self) -> (f32, f32) {
        self.core.text.cell_size()
    }

    /// The window's current scale factor (physical pixels per logical pixel).
    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    /// Glyph-atlas pressure as of the last prepared frame — how many text runs
    /// and distinct font sizes it was asked to hold, and whether it ever
    /// overflowed. Meant for `/state`: a nonzero `overflows` is the signal that
    /// text is missing from the screen for a reason the pixels cannot explain.
    pub fn text_atlas_stats(&self) -> AtlasStats {
        self.core.text.atlas_stats()
    }
}

/// GPU renderer with no window or surface: renders scenes offscreen and reads
/// them back via [`capture`](HeadlessRenderer::capture). Used by the headless
/// run mode to serve `/screenshot` without ever opening a window.
pub struct HeadlessRenderer {
    core: GpuCore,
}

impl HeadlessRenderer {
    /// Create a surface-less renderer targeting `logical_size` at
    /// `scale_factor` (physical size = logical × scale). Returns an error
    /// instead of panicking — a headless session can keep running without a
    /// GPU; only screenshots become unavailable.
    pub fn new(logical_size: (f32, f32), scale_factor: f64) -> Result<HeadlessRenderer, String> {
        pollster::block_on(Self::new_async(logical_size, scale_factor))
    }

    async fn new_async(
        logical_size: (f32, f32),
        scale_factor: f64,
    ) -> Result<HeadlessRenderer, String> {
        let context = GpuContext::new_headless_async().await?;
        Self::with_context(&context, logical_size, scale_factor)
    }

    /// Create a renderer on an existing shared [`GpuContext`] instead of
    /// setting up a fresh device — only the per-renderer pipelines, glyph
    /// atlas, and offscreen target size are created here.
    pub fn with_context(
        context: &GpuContext,
        logical_size: (f32, f32),
        scale_factor: f64,
    ) -> Result<HeadlessRenderer, String> {
        let width = (logical_size.0 as f64 * scale_factor).round() as u32;
        let height = (logical_size.1 as f64 * scale_factor).round() as u32;
        Ok(HeadlessRenderer {
            core: GpuCore::new(
                context.device.clone(),
                context.queue.clone(),
                wgpu::TextureFormat::Rgba8UnormSrgb,
                width.max(1),
                height.max(1),
                scale_factor,
                supported_samples(&context.adapter, wgpu::TextureFormat::Rgba8UnormSrgb),
            ),
        })
    }

    /// Resize the offscreen target (logical pixels).
    pub fn resize(&mut self, logical_size: (f32, f32)) {
        let width = (logical_size.0 as f64 * self.core.scale_factor).round() as u32;
        let height = (logical_size.1 as f64 * self.core.scale_factor).round() as u32;
        self.core.resize(width, height);
    }

    /// Render `scene` offscreen and read it back as RGBA8 pixels.
    pub fn capture(&mut self, scene: &Scene) -> Capture {
        self.core.capture(scene)
    }

    /// Monospace cell metrics, identical to [`Renderer::cell_size`].
    pub fn cell_size(&self) -> (f32, f32) {
        self.core.text.cell_size()
    }

    /// Glyph-atlas pressure, identical to [`Renderer::text_atlas_stats`].
    pub fn text_atlas_stats(&self) -> AtlasStats {
        self.core.text.atlas_stats()
    }
}
