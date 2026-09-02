//! # garden-render — GPU renderer for Garden
//!
//! Draws a [`Scene`] of primitives — quads, meshes, images and text runs, plus
//! offscreen canvases that can be drawn into, snapshotted from the frame,
//! blurred and composited back — using `wgpu` and `glyphon` (text shaping +
//! glyph atlas). The
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

mod blur;
pub mod fonts;
mod globals;
mod image;
mod mesh;
mod quad;
mod text;

pub use text::{last_atlas_stats, AtlasStats, FONT_SIZE, LINE_HEIGHT_RATIO};

use std::collections::HashMap;
use std::sync::Arc;

use blur::Filters;
use image::{ImageDraw, ImagePipeline, ImageSource};
use mesh::MeshPipeline;
use quad::QuadPipeline;
use text::{TextRun, TextStack};
use winit::window::Window;

/// An RGBA color with components in `0.0..=1.0`, in **sRGB space** (the
/// values you'd read off a hex color picker).
///
/// These values reach the render target unchanged: the scene renders into a
/// target whose format holds sRGB-*encoded* bytes without a transfer function
/// (`Rgba8Unorm`/`Bgra8Unorm`, not the `…Srgb` pair), so `ALPHA_BLENDING`
/// composites gamma-encoded values — the same space CSS, Core Graphics and
/// Figma blend in. Black at 50% over white is `#808080` here, as it is
/// everywhere a designer would check it.
///
/// The renderer used to linearize on the CPU and let the sRGB target re-encode
/// on store, which is physically correct light-mixing but matches nothing a
/// panel author compares against: the same 50% black came out `#bbbbbb`, and
/// every translucent overlay, hairline and shadow in the ecosystem was tuned
/// against a value that disagreed with the design tool it came from.
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

    /// The four components as the shaders want them — sRGB-encoded, straight
    /// through. Named rather than inlined so the (deliberate) absence of a
    /// transfer function is visible at every call site.
    pub(crate) fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
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

/// A rounded-rectangle mask applied per *fragment*, on top of the rectangular
/// GPU scissor.
///
/// A scissor rect cannot express a rounded corner — it is four integer edges —
/// so a panel that clips its contents to a rounded card had, until this, no
/// way to say so. The mask closes that: the fragment shader evaluates the
/// signed distance to the rounded rect and feathers coverage across one
/// physical pixel at the boundary, which is why a circular crop comes out with
/// a clean antialiased edge rather than a staircase.
///
/// [`NONE`](ClipMask::NONE) — a zero radius — means "no mask", and costs one
/// compare in the shader: everything Garden itself draws stays on the cheap
/// path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipMask {
    pub rect: Rect,
    /// Corner radius in logical pixels, clamped in the shader to half the
    /// shorter side. Zero (or less) disables the mask.
    pub radius: f32,
}

impl ClipMask {
    /// No mask: the rectangular scissor is the whole of the clipping.
    pub const NONE: ClipMask = ClipMask {
        rect: Rect::new(0.0, 0.0, 0.0, 0.0),
        radius: 0.0,
    };

    /// Mask to `rect` with `radius`-px corners.
    pub const fn rounded(rect: Rect, radius: f32) -> ClipMask {
        ClipMask { rect, radius }
    }

    /// Whether this mask does nothing.
    pub fn is_none(self) -> bool {
        self.radius <= 0.0
    }
}

/// One vertex of a [`Primitive::Mesh`]: a position in logical pixels, an sRGB
/// color, and the rounded-rect mask its fragments are cut against.
///
/// The mask rides on the vertex rather than on the primitive because every
/// vertex of a mesh shares it: the caller flushes a fresh mesh whenever the
/// clip changes, so stamping it here is free of extra plumbing and cannot get
/// out of step with the geometry it applies to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub pos: (f32, f32),
    pub color: Color,
    pub mask: ClipMask,
}

impl Vertex {
    /// A vertex with no rounded mask — the rectangular clip on its
    /// [`Primitive::Mesh`] is the whole of the clipping.
    pub const fn new(pos: (f32, f32), color: Color) -> Vertex {
        Vertex {
            pos,
            color,
            mask: ClipMask::NONE,
        }
    }

    /// A vertex whose fragments are additionally cut against `mask`.
    pub const fn masked(pos: (f32, f32), color: Color, mask: ClipMask) -> Vertex {
        Vertex { pos, color, mask }
    }
}

/// The typographic axes a text run can vary beyond its size. Panel scripts
/// set these through the petal-ui draw protocol; everything Garden itself
/// draws uses the default (the monospace face, regular, upright, unspaced).
///
/// Weight and slant are requests to the shaper, answered from the cuts the
/// chosen family actually ships: a face with a real Bold (Inter, and most
/// system families) gets it, and a face with only one weight — the embedded
/// JetBrains Mono — has its bold synthesized by over-drawing. Letter-spacing,
/// by contrast, is applied by the caller placing glyphs, so it always takes
/// effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// CSS numeric weight, 100–900 (400 regular, 700 bold).
    pub weight: u16,
    pub italic: bool,
    /// Letter-spacing in logical px, added after each glyph.
    pub spacing: f32,
    /// Which family to shape with — an interned [`fonts::FontId`], so a style
    /// can name any face on the machine and still be `Copy`.
    pub font: fonts::FontId,
}

/// CSS regular weight — what every run Garden itself draws uses.
pub const REGULAR_WEIGHT: u16 = 400;

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            weight: REGULAR_WEIGHT,
            italic: false,
            spacing: 0.0,
            font: fonts::FontId::MONO,
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
    /// A triangle list (`vertices.len()` is a multiple of 3), scissored to
    /// `clip`. The general geometry primitive: callers tessellate
    /// lines/circles/polygons into triangles on the CPU. Unlike [`Quad`],
    /// clipping is a real GPU scissor — a triangle can't be CPU-clipped to a
    /// rect — so a caller that needs containment passes the bounding `clip`.
    ///
    /// Colour is **per vertex** and interpolated affinely, which is what makes
    /// a gradient one shape's worth of triangles rather than a stack of bands,
    /// and a soft shadow one non-overlapping mesh rather than N translucent
    /// layers. A vertex also carries a [`ClipMask`], for the rounded corners
    /// the rectangular scissor cannot express.
    Mesh { vertices: Vec<Vertex>, clip: Rect },
    /// A PNG bitmap loaded from `source`, scaled to `rect`, and scissored to
    /// `clip`. Relative sources resolve from Garden's working directory.
    ///
    /// `mask` is the rounded-rect cut its fragments survive — the circular
    /// avatar and the rounded thumbnail, which the rectangular scissor cannot
    /// express. It rides on the primitive rather than on a vertex because an
    /// image is one instanced quad with no vertex stream of its own.
    Image {
        rect: Rect,
        source: String,
        alpha: f32,
        clip: Rect,
        mask: ClipMask,
    },
    /// Create (or re-create, cleared) offscreen canvas `id` of logical
    /// `size`, transparent. Canvas ids are per scene: the same id in the next
    /// scene reuses the texture when the size matches.
    ///
    /// A canvas is the renderer's one offscreen buffer, and every effect that
    /// needs one is an operation on a canvas id — [`Target`](Self::Target)
    /// to draw into it, [`Snapshot`](Self::Snapshot) to fill it from what is
    /// already drawn, [`Blur`](Self::Blur) to filter it, and
    /// [`CanvasDraw`](Self::CanvasDraw) to composite it. A new effect is a
    /// new operation, not a new kind of buffer.
    ///
    /// Canvases hold **premultiplied** color: drawing straight-alpha
    /// primitives over transparent black with the normal over-blend leaves
    /// premultiplied pixels behind, and that is also the space a blur has to
    /// run in for translucent edges not to darken. `CanvasDraw` composites
    /// them accordingly.
    Canvas { id: u32, size: (f32, f32) },
    /// Aim every following draw at canvas `id` (or the frame for `0`), with
    /// coordinates relative to the canvas's top-left. A target that was
    /// never created drops the draws aimed at it.
    Target { id: u32 },
    /// Copy into canvas `id` the pixels of the current target under the
    /// canvas rect placed at `from`, cut to `clip` — what a backdrop effect
    /// samples. Reading the frame is allowed at any point: the frame is
    /// whatever has been drawn so far.
    Snapshot {
        id: u32,
        from: (f32, f32),
        clip: Rect,
    },
    /// Gaussian-blur canvas `id` in place; `radius` is the standard
    /// deviation in logical pixels (CSS `blur()` semantics), edges clamp.
    Blur { id: u32, radius: f32 },
    /// Composite canvas `id` (scaled to `rect`, at `alpha`) into the current
    /// target, scissored to `clip` and cut to `mask` — the canvas analogue of
    /// [`Image`](Self::Image).
    CanvasDraw {
        id: u32,
        rect: Rect,
        alpha: f32,
        clip: Rect,
        mask: ClipMask,
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
///
/// The canvas primitives ([`Primitive::Canvas`] and friends) split the list
/// further, into one render pass per stretch of draws into one target, with
/// the copies and filters between them recorded in the same order. A scene
/// without them is the single pass it always was.
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
/// from the embedded monospace font on the CPU — the table petal-ui's
/// `text_width` sums so a Petal panel script measures text the way this
/// renderer draws it. Ratios, not pixel widths, so one table serves every size.
pub fn ascii_advance_ratios() -> Vec<f64> {
    text::measure_embedded_advances(fonts::FontId::MONO, REGULAR_WEIGHT)
}

/// The same table for the embedded **proportional** UI face ([`fonts::FontId::UI`]).
///
/// A monospace table is fully described by its single ratio; this one is not —
/// every glyph differs, which is the whole reason `text_width` sums a
/// per-codepoint table instead of multiplying by a constant. A host that draws
/// `font: "ui"` must publish this alongside [`ascii_advance_ratios`], or
/// scripts will measure proportional text with monospace advances and every
/// centered or right-aligned run will land wrong.
pub fn ui_ascii_advance_ratios() -> Vec<f64> {
    text::measure_embedded_advances(fonts::FontId::UI, REGULAR_WEIGHT)
}

/// The same table for Inter **Bold** — the cut a `font: "ui"` run at
/// `weight >= 600` is actually shaped with.
///
/// A host that draws this face has to publish this alongside
/// [`ui_ascii_advance_ratios`]. Bold Inter is meaningfully wider than regular
/// Inter, so a bold UI run measured with the regular table comes out short, and
/// every centered or right-aligned bold label lands wrong while nothing about
/// the drawing looks broken.
pub fn ui_bold_ascii_advance_ratios() -> Vec<f64> {
    text::measure_embedded_advances(fonts::FontId::UI, 700)
}

/// Advance ratio for a codepoint outside a face's measured table — the same
/// monospace estimate `garden-script` falls back to, so a measurement here and
/// a script's `text_width` stay in step off the table too.
const FALLBACK_ADVANCE_RATIO: f64 = 0.6;

/// The advance width of `text` drawn at `size` in `style`, in logical pixels:
/// what the shaper will actually lay down, summed from the measured table of
/// the cut this style resolves to, plus its letter-spacing after each glyph
/// (the way [`TextStyle::spacing`] is drawn).
///
/// The host has already shaped every run it draws, so this is the width a
/// scene dump can report — and without it a scene-level comparison against a
/// reference layout can only compare origins, which is blind to exactly the
/// bugs that matter (a run measured in the wrong face is in the right place and
/// the wrong size).
pub fn measure_text(text: &str, size: f32, style: TextStyle) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let ratios = fonts::advance_ratios(style.font, style.weight, style.italic);
    let sum: f64 = text
        .chars()
        .map(|ch| {
            ratios
                .get(ch as usize)
                .copied()
                .unwrap_or(FALLBACK_ADVANCE_RATIO)
        })
        .sum();
    sum as f32 * size + style.spacing * text.chars().count() as f32
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
/// the quad/mesh/image/text pipelines and the filters, the current
/// render-target geometry (physical pixels + scale factor), and the offscreen
/// canvases the last scene created.
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
    filters: Filters,
    /// The multisampled color target the frame's passes draw into, resolved
    /// into the surface (or the capture texture) at the end of each. `None`
    /// when `samples == 1`, in which case passes draw to the target directly.
    /// Built on demand and rebuilt when the target size changes.
    samples: u32,
    msaa: Option<wgpu::TextureView>,
    msaa_size: (u32, u32),
    /// The current scene as a sequence of steps — draw runs, canvas
    /// creation, target switches, snapshots and filters — in scene order.
    /// Recording walks this so painter's order holds across primitive kinds
    /// and across targets — see [`Scene`].
    steps: Vec<Step>,
    /// Physical size of every target this frame draws into, by slot: slot 0
    /// is the frame, then one per canvas in creation order.
    targets: Vec<(u32, u32)>,
    /// The text batches (`Step::Draw` of kind `Text`) in order, each with
    /// the slot it draws into — what [`TextStack::prepare`] stages against.
    text_batches: Vec<(std::ops::Range<usize>, usize)>,
    /// Offscreen canvases, by id. Kept across frames so a panel that creates
    /// the same canvas every frame (they all do — the scene is rebuilt each
    /// frame) reuses its texture; a canvas the last scene did not create is
    /// dropped.
    canvases: HashMap<u32, CanvasTex>,
    /// Whether the scene reads the frame back (a [`Primitive::Snapshot`]
    /// taken while the frame is the target). The windowed renderer then
    /// draws into an intermediate texture — a surface texture cannot be
    /// copied from — and presents by copying that to the surface.
    frame_snapshot: bool,
    /// Whether the scene has any canvas step at all. With none, the frame is
    /// one pass and its MSAA attachment can be discarded after the resolve;
    /// with some, the frame's passes are interleaved with canvas work and the
    /// attachment has to survive between them.
    multipass: bool,
    /// The intermediate frame texture used when `frame_snapshot` — see
    /// [`Renderer::render`].
    frame: Option<FrameTex>,
}

/// An offscreen render target created by a [`Primitive::Canvas`].
struct CanvasTex {
    size_px: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Its own multisampled attachment, resolved into `texture` at the end
    /// of every pass that draws into it. `None` when not multisampling.
    msaa: Option<wgpu::TextureView>,
    /// Sampling bind group for the filters (`images` keeps its own).
    sample: wgpu::BindGroup,
    /// Globals/viewport slot — 1-based; slot 0 is the frame.
    slot: usize,
    /// The MSAA attachment no longer matches `texture` (a snapshot or a
    /// filter wrote the resolved texture directly), so the next pass into
    /// this canvas has to re-seed the attachment before drawing.
    msaa_stale: bool,
    /// Created by the scene being prepared; the rest are dropped.
    used: bool,
}

/// The intermediate frame texture of a scene that snapshots the frame.
struct FrameTex {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sample: wgpu::BindGroup,
}

/// Which pipeline draws a `Step::Draw`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BatchKind {
    Quad,
    Mesh,
    Image,
    Text,
}

/// One unit of the recording walk, in scene order.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// One maximal run of consecutive same-kind primitives into one target,
    /// as a half-open range of indices *within that kind's* staged list
    /// (each pipeline stages in scene order, so these address it directly).
    /// `slot` is `None` when the run was aimed at a target that does not
    /// exist — it is staged (so the indices stay aligned) but never drawn.
    Draw {
        kind: BatchKind,
        start: usize,
        end: usize,
        slot: Option<usize>,
    },
    /// Clear canvas `id` to transparent.
    Canvas { id: u32 },
    /// Copy a region of the current target into canvas `id`.
    Snapshot {
        id: u32,
        from: (f32, f32),
        clip: Rect,
        /// The target being read: `None` for the frame, else a canvas.
        source: Option<u32>,
    },
    /// Blur canvas `id` in place.
    Blur { id: u32, radius: f32 },
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
        let filters = Filters::new(&device, format, samples);
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
            filters,
            samples,
            msaa: None,
            msaa_size: (0, 0),
            steps: Vec::new(),
            targets: Vec::new(),
            text_batches: Vec::new(),
            canvases: HashMap::new(),
            frame_snapshot: false,
            multipass: false,
            frame: None,
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
    }

    /// A single-sampled color texture of `size` in the target format, usable
    /// as a pass attachment, a sampled texture, and both ends of a copy.
    fn create_color_texture(&self, label: &str, size: (u32, u32)) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// A multisampled attachment of `size`, or `None` when not multisampling.
    fn create_msaa(&self, label: &str, size: (u32, u32)) -> Option<wgpu::TextureView> {
        if self.samples == 1 {
            return None;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: self.samples,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Some(texture.create_view(&wgpu::TextureViewDescriptor::default()))
    }

    /// Make sure the frame's multisampled target matches the current size.
    /// Called from [`prepare_scene`](Self::prepare_scene), which both the
    /// present and the capture path run before recording, so neither can
    /// reach a pass with a stale attachment after a resize.
    fn ensure_msaa(&mut self) {
        if self.samples == 1 {
            return;
        }
        let size = (self.width.max(1), self.height.max(1));
        if self.msaa.is_some() && self.msaa_size == size {
            return;
        }
        self.msaa = self.create_msaa("garden msaa target", size);
        self.msaa_size = size;
    }

    /// The intermediate frame texture at the current size — see
    /// [`GpuCore::frame_snapshot`].
    fn ensure_frame_texture(&mut self) {
        let size = (self.width.max(1), self.height.max(1));
        if self.frame.as_ref().is_some_and(|f| f.size == size) {
            return;
        }
        let texture = self.create_color_texture("garden frame texture", size);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sample = self.filters.sample_bind_group(&self.device, &view);
        self.frame = Some(FrameTex {
            size,
            texture,
            view,
            sample,
        });
    }

    /// Physical size of a canvas of logical `size` at the current scale,
    /// clamped to what the device can allocate.
    fn canvas_physical_size(&self, size: (f32, f32)) -> (u32, u32) {
        let max = self.device.limits().max_texture_dimension_2d.max(1);
        let scale = self.scale_factor;
        let px = |v: f32| ((v.max(0.0) as f64 * scale).ceil() as u32).clamp(1, max);
        (px(size.0), px(size.1))
    }

    /// Stage `scene` for drawing at the current target size: upload quad,
    /// mesh and image instances, shape/upload text runs, allocate the
    /// canvases it creates, and build the step list recording follows.
    /// Shared by present and capture.
    fn prepare_scene(&mut self, scene: &Scene) {
        self.ensure_msaa();
        self.filters.begin_frame();
        self.filters.reserve_copy_params(&self.device, &self.queue);
        let scale_factor = self.scale_factor as f32;
        let physical = (self.width, self.height);

        // ── The walk: steps, targets, canvases ────────────────────────────
        for canvas in self.canvases.values_mut() {
            canvas.used = false;
        }
        self.steps.clear();
        self.targets.clear();
        self.targets.push(physical);
        self.frame_snapshot = false;
        self.multipass = false;

        // Next unused index for each kind, in `BatchKind` order.
        let mut next = [0usize; 4];
        // Current target: `Some(None)` is the frame, `Some(Some(id))` a
        // canvas, `None` an id that was never created (draws are dropped).
        let mut target: Option<Option<u32>> = Some(None);
        let mut target_slot: Option<usize> = Some(0);
        let mut new_canvases: Vec<(u32, (u32, u32), usize)> = Vec::new();
        for p in &scene.primitives {
            let kind = match p {
                Primitive::Quad { .. } => BatchKind::Quad,
                Primitive::Mesh { .. } => BatchKind::Mesh,
                Primitive::Image { .. } | Primitive::CanvasDraw { .. } => BatchKind::Image,
                Primitive::Text { .. } => BatchKind::Text,
                Primitive::Canvas { id, size } => {
                    self.multipass = true;
                    let slot = self.targets.len();
                    let size_px = self.canvas_physical_size(*size);
                    self.targets.push(size_px);
                    // A re-created id replaces the earlier canvas of that id
                    // for the rest of the walk, as the draw protocol says.
                    new_canvases.retain(|(other, _, _)| other != id);
                    new_canvases.push((*id, size_px, slot));
                    self.steps.push(Step::Canvas { id: *id });
                    continue;
                }
                Primitive::Target { id } => {
                    self.multipass = true;
                    if *id == 0 {
                        target = Some(None);
                        target_slot = Some(0);
                    } else {
                        match new_canvases.iter().find(|(other, _, _)| other == id) {
                            Some((_, _, slot)) => {
                                target = Some(Some(*id));
                                target_slot = Some(*slot);
                            }
                            None => {
                                target = None;
                                target_slot = None;
                            }
                        }
                    }
                    continue;
                }
                Primitive::Snapshot { id, from, clip } => {
                    self.multipass = true;
                    let Some(source) = target else { continue };
                    if source.is_none() {
                        self.frame_snapshot = true;
                    }
                    self.steps.push(Step::Snapshot {
                        id: *id,
                        from: *from,
                        clip: *clip,
                        source,
                    });
                    continue;
                }
                Primitive::Blur { id, radius } => {
                    self.multipass = true;
                    self.steps.push(Step::Blur {
                        id: *id,
                        radius: *radius,
                    });
                    continue;
                }
            };
            let counter = &mut next[kind as usize];
            let index = *counter;
            *counter += 1;
            match self.steps.last_mut() {
                Some(Step::Draw {
                    kind: last_kind,
                    end,
                    slot,
                    ..
                }) if *last_kind == kind && *slot == target_slot => *end = index + 1,
                _ => self.steps.push(Step::Draw {
                    kind,
                    start: index,
                    end: index + 1,
                    slot: target_slot,
                }),
            }
        }

        // ── Canvas textures ──────────────────────────────────────────────
        self.images.clear_canvases();
        for (id, size_px, slot) in new_canvases {
            let reuse = self
                .canvases
                .get(&id)
                .is_some_and(|c| c.size_px == size_px && !c.used);
            if !reuse {
                let texture = self.create_color_texture("garden canvas", size_px);
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let msaa = self.create_msaa("garden canvas msaa", size_px);
                let sample = self.filters.sample_bind_group(&self.device, &view);
                self.canvases.insert(
                    id,
                    CanvasTex {
                        size_px,
                        texture,
                        view,
                        msaa,
                        sample,
                        slot,
                        msaa_stale: false,
                        used: true,
                    },
                );
            }
            let canvas = self.canvases.get_mut(&id).expect("canvas just inserted");
            canvas.used = true;
            canvas.slot = slot;
            canvas.msaa_stale = false;
            self.images.set_canvas(&self.device, id, &canvas.view);
        }
        self.canvases.retain(|_, c| c.used);

        // ── Per-target globals ───────────────────────────────────────────
        for (slot, size) in self.targets.iter().enumerate() {
            let logical = (size.0 as f32 / scale_factor, size.1 as f32 / scale_factor);
            self.quads
                .set_target(&self.device, &self.queue, slot, logical, scale_factor);
            self.meshes
                .set_target(&self.device, &self.queue, slot, logical, scale_factor);
            self.images
                .set_target(&self.device, &self.queue, slot, logical, scale_factor);
        }

        // ── Per-kind staging ─────────────────────────────────────────────
        self.quads.prepare(
            &self.device,
            &self.queue,
            scene.primitives.iter().filter_map(|p| match p {
                Primitive::Quad { rect, color } => Some((rect, color)),
                _ => None,
            }),
        );

        self.meshes.prepare(
            &self.device,
            &self.queue,
            scene.primitives.iter().filter_map(|p| match p {
                Primitive::Mesh { vertices, clip } => Some((vertices.as_slice(), clip)),
                _ => None,
            }),
        );

        self.images.prepare(
            &self.device,
            &self.queue,
            scene.primitives.iter().filter_map(|p| match p {
                Primitive::Image {
                    rect,
                    source,
                    alpha,
                    clip,
                    mask,
                } => Some(ImageDraw {
                    rect,
                    source: ImageSource::File(source.clone()),
                    alpha: *alpha,
                    clip,
                    mask: *mask,
                }),
                Primitive::CanvasDraw {
                    id,
                    rect,
                    alpha,
                    clip,
                    mask,
                } => Some(ImageDraw {
                    rect,
                    source: ImageSource::Canvas(*id),
                    alpha: *alpha,
                    clip,
                    mask: *mask,
                }),
                _ => None,
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
                _ => None,
            })
            .collect();
        self.text_batches = self
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Draw {
                    kind: BatchKind::Text,
                    start,
                    end,
                    slot,
                } => Some((*start..*end, slot.unwrap_or(0))),
                _ => None,
            })
            .collect();
        self.text.prepare(
            &self.device,
            &self.queue,
            &self.targets,
            scale_factor,
            &texts,
            &self.text_batches,
        );
    }

    /// Record the scene staged by [`prepare_scene`](Self::prepare_scene):
    /// every pass into the frame (`frame_view`) and into the canvases, plus
    /// the snapshots and filters between them, in scene order.
    ///
    /// `frame_texture` is the texture behind `frame_view` when it can be
    /// copied from — what a snapshot of the frame reads. The windowed path
    /// passes its intermediate texture (a surface texture cannot be read);
    /// the capture path passes the capture texture.
    fn record_scene(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        frame_view: &wgpu::TextureView,
        frame_texture: Option<&wgpu::Texture>,
        bg: Color,
    ) {
        let GpuCore {
            device,
            queue,
            width,
            height,
            scale_factor,
            quads,
            meshes,
            images,
            text,
            filters,
            msaa,
            steps,
            canvases,
            multipass,
            ..
        } = self;
        let msaa: Option<&wgpu::TextureView> = msaa.as_ref();
        let frame_size = (*width, *height);
        let scale = *scale_factor as f32;
        let clear = {
            let [r, g, b, a] = bg.to_array();
            wgpu::Color {
                r: r as f64,
                g: g as f64,
                b: b as f64,
                a: a as f64,
            }
        };
        // With MSAA a pass draws into the multisampled texture and resolves
        // into the target on the way out. A single-pass frame never reads the
        // multisampled texture again, so it is discarded rather than stored;
        // a multipass frame comes back to it, so it has to survive.
        let msaa_store = if *multipass {
            wgpu::StoreOp::Store
        } else {
            wgpu::StoreOp::Discard
        };
        let mut frame_started = false;
        let mut text_batch = 0usize;

        let mut i = 0;
        while i < steps.len() {
            match steps[i] {
                Step::Draw { slot, .. } => {
                    // Gather the run of draws into this same target.
                    let mut j = i;
                    while j < steps.len()
                        && matches!(steps[j], Step::Draw { slot: s, .. } if s == slot)
                    {
                        j += 1;
                    }
                    let run = &steps[i..j];
                    i = j;
                    let Some(slot) = slot else {
                        // Aimed at a target that does not exist: skip, but
                        // keep the text batch counter aligned.
                        text_batch += run
                            .iter()
                            .filter(|s| {
                                matches!(
                                    s,
                                    Step::Draw {
                                        kind: BatchKind::Text,
                                        ..
                                    }
                                )
                            })
                            .count();
                        continue;
                    };

                    // Resolve the target: attachment, resolve target, size,
                    // and how to start the pass.
                    let (view, resolve, size, load, store, reseed) = if slot == 0 {
                        let load = if frame_started {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(clear)
                        };
                        frame_started = true;
                        match msaa {
                            Some(m) => (m, Some(frame_view), frame_size, load, msaa_store, None),
                            None => (
                                frame_view,
                                None,
                                frame_size,
                                load,
                                wgpu::StoreOp::Store,
                                None,
                            ),
                        }
                    } else {
                        let Some(canvas) = canvases.values_mut().find(|c| c.slot == slot) else {
                            text_batch += run
                                .iter()
                                .filter(|s| {
                                    matches!(
                                        s,
                                        Step::Draw {
                                            kind: BatchKind::Text,
                                            ..
                                        }
                                    )
                                })
                                .count();
                            continue;
                        };
                        match &canvas.msaa {
                            Some(m) => {
                                // The attachment is re-seeded from the
                                // resolved texture when a snapshot or filter
                                // has changed it. The canvas cannot be both
                                // the resolve target and a sampled texture
                                // of the same pass, so the copy goes through
                                // a scratch.
                                let reseed = if canvas.msaa_stale {
                                    canvas.msaa_stale = false;
                                    let scratch = filters.acquire_scratch(device, canvas.size_px);
                                    encoder.copy_texture_to_texture(
                                        canvas.texture.as_image_copy(),
                                        filters.scratch(scratch).texture().as_image_copy(),
                                        wgpu::Extent3d {
                                            width: canvas.size_px.0,
                                            height: canvas.size_px.1,
                                            depth_or_array_layers: 1,
                                        },
                                    );
                                    Some(scratch)
                                } else {
                                    None
                                };
                                let load = if reseed.is_some() {
                                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                                } else {
                                    wgpu::LoadOp::Load
                                };
                                (
                                    m,
                                    Some(&canvas.view),
                                    canvas.size_px,
                                    load,
                                    wgpu::StoreOp::Store,
                                    reseed,
                                )
                            }
                            None => (
                                &canvas.view,
                                None,
                                canvas.size_px,
                                wgpu::LoadOp::Load,
                                wgpu::StoreOp::Store,
                                None,
                            ),
                        }
                    };

                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("garden scene pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: resolve,
                            // The target holds sRGB-encoded bytes with no
                            // transfer function, so the clear color goes in
                            // as written — the space every pipeline blends in.
                            ops: wgpu::Operations { load, store },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    if let Some(scratch) = reseed {
                        filters.copy_into_pass(&mut pass, filters.scratch(scratch).bind_group());
                    }
                    // Walk the run in order so a quad drawn after a text run
                    // covers it, the way the primitive list says it should.
                    for step in run {
                        let Step::Draw {
                            kind, start, end, ..
                        } = *step
                        else {
                            unreachable!("run holds only draws");
                        };
                        match kind {
                            BatchKind::Quad => {
                                quads.render(&mut pass, slot, start as u32..end as u32)
                            }
                            BatchKind::Mesh => {
                                meshes.render(&mut pass, slot, size, scale, start..end)
                            }
                            BatchKind::Image => {
                                images.render(&mut pass, slot, size, scale, start..end)
                            }
                            BatchKind::Text => {
                                text.render_batch(text_batch, &mut pass);
                                text_batch += 1;
                            }
                        }
                    }
                }
                Step::Canvas { id } => {
                    i += 1;
                    let Some(canvas) = canvases.get_mut(&id) else {
                        continue;
                    };
                    canvas.msaa_stale = false;
                    let (view, resolve) = match &canvas.msaa {
                        Some(m) => (m, Some(&canvas.view)),
                        None => (&canvas.view, None),
                    };
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("garden canvas clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: resolve,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
                Step::Snapshot {
                    id,
                    from,
                    clip,
                    source,
                } => {
                    i += 1;
                    if source == Some(id) {
                        continue; // a canvas cannot snapshot itself
                    }
                    let (src_texture, src_size) = match source {
                        None => {
                            let Some(texture) = frame_texture else {
                                continue;
                            };
                            if !frame_started {
                                // Nothing has been drawn into the frame yet:
                                // what the snapshot sees is the cleared
                                // background.
                                frame_started = true;
                                let (view, resolve) = match msaa {
                                    Some(m) => (m, Some(frame_view)),
                                    None => (frame_view, None),
                                };
                                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: Some("garden frame clear"),
                                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                        view,
                                        depth_slice: None,
                                        resolve_target: resolve,
                                        ops: wgpu::Operations {
                                            load: wgpu::LoadOp::Clear(clear),
                                            store: msaa_store,
                                        },
                                    })],
                                    depth_stencil_attachment: None,
                                    timestamp_writes: None,
                                    occlusion_query_set: None,
                                    multiview_mask: None,
                                });
                            }
                            (texture, frame_size)
                        }
                        Some(src) => match canvases.get(&src) {
                            Some(c) => (&c.texture, c.size_px),
                            None => continue,
                        },
                    };
                    let Some(dest) = canvases.get(&id) else {
                        continue;
                    };
                    let dest_size = dest.size_px;
                    // The copied region, in physical pixels of the source:
                    // the canvas rect placed at `from`, cut to the clip and
                    // to the source's bounds.
                    let px = |v: f32| (v * scale).round() as i64;
                    let fx = px(from.0);
                    let fy = px(from.1);
                    let x0 = fx.max(px(clip.x)).max(0);
                    let y0 = fy.max(px(clip.y)).max(0);
                    let x1 = (fx + dest_size.0 as i64)
                        .min(px(clip.x + clip.w))
                        .min(src_size.0 as i64);
                    let y1 = (fy + dest_size.1 as i64)
                        .min(px(clip.y + clip.h))
                        .min(src_size.1 as i64);
                    if x1 <= x0 || y1 <= y0 {
                        continue;
                    }
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: src_texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: x0 as u32,
                                y: y0 as u32,
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyTextureInfo {
                            texture: &dest.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: (x0 - fx) as u32,
                                y: (y0 - fy) as u32,
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::Extent3d {
                            width: (x1 - x0) as u32,
                            height: (y1 - y0) as u32,
                            depth_or_array_layers: 1,
                        },
                    );
                    let dest = canvases.get_mut(&id).expect("dest canvas present");
                    dest.msaa_stale = dest.msaa.is_some();
                }
                Step::Blur { id, radius } => {
                    i += 1;
                    let Some(canvas) = canvases.get_mut(&id) else {
                        continue;
                    };
                    filters.blur(
                        device,
                        queue,
                        encoder,
                        &canvas.sample,
                        &canvas.view,
                        canvas.size_px,
                        radius * scale,
                    );
                    canvas.msaa_stale = canvas.msaa.is_some();
                }
            }
        }

        // A scene with no frame draws at all still has to clear the frame.
        if !frame_started {
            let (view, resolve) = match msaa {
                Some(m) => (m, Some(frame_view)),
                None => (frame_view, None),
            };
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("garden frame clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
    }

    /// Render `scene` into an offscreen texture at the current target size
    /// and read it back as tightly packed RGBA8 pixels (sRGB-encoded bytes,
    /// ready for PNG). Blocks until the GPU readback completes.
    fn capture(&mut self, scene: &Scene) -> Capture {
        let (width, height) = (self.width.max(1), self.height.max(1));
        self.prepare_scene(scene);

        let texture = self.create_color_texture("garden capture target", (width, height));
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
        self.record_scene(&mut encoder, &view, Some(&texture), scene.bg);
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
        // Prefer a **non**-sRGB format: the scene composites in the
        // gamma-encoded space (see [`Color`]), which means the target must
        // store what the shaders write rather than apply a transfer function
        // on the way in. A surface that only offers the sRGB variant is still
        // usable — configure it as sRGB but render through a view in its
        // non-sRGB twin, which is exactly what `view_formats` is for.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| "surface is incompatible with the GPU adapter".to_string())?;
        let view_format = format.remove_srgb_suffix();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: physical_size.width.max(1),
            height: physical_size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: match view_format == format {
                true => vec![],
                false => vec![view_format],
            },
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&context.device, &config);

        let core = GpuCore::new(
            context.device.clone(),
            context.queue.clone(),
            view_format,
            config.width,
            config.height,
            window.scale_factor(),
            supported_samples(&context.adapter, view_format),
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

        // Through the non-sRGB view (see `configure_for_window`), so the pass
        // writes sRGB-encoded bytes with no transfer function applied.
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.core.format),
            ..Default::default()
        });
        let mut encoder =
            self.core
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("garden frame"),
                });
        if self.core.frame_snapshot {
            // A surface texture cannot be copied from, and the scene wants to
            // read the frame back (a backdrop snapshot). Draw into an
            // intermediate texture instead and present by copying it over —
            // one full-screen quad, only on frames that ask for it.
            self.core.ensure_frame_texture();
            let frame_tex = self.core.frame.take().expect("frame texture just ensured");
            self.core.record_scene(
                &mut encoder,
                &frame_tex.view,
                Some(&frame_tex.texture),
                scene.bg,
            );
            self.core.filters.copy(
                &self.core.device,
                &self.core.queue,
                &mut encoder,
                &frame_tex.sample,
                &view,
            );
            self.core.frame = Some(frame_tex);
        } else {
            self.core.record_scene(&mut encoder, &view, None, scene.bg);
        }

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
                wgpu::TextureFormat::Rgba8Unorm,
                width.max(1),
                height.max(1),
                scale_factor,
                supported_samples(&context.adapter, wgpu::TextureFormat::Rgba8Unorm),
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
