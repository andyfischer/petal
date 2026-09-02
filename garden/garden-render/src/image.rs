//! Cached PNG textures, offscreen-canvas textures, and the textured-quad
//! pipeline that draws either.
//!
//! A bitmap and a canvas are the same draw — one instanced quad sampling one
//! texture — with one difference: a PNG's pixels are *straight* alpha, while
//! a canvas holds *premultiplied* pixels (everything drawn into a transparent
//! target with the scene's over-blend lands premultiplied). So there are two
//! pipelines over the same shader: the straight one composites with
//! `ALPHA_BLENDING`, the premultiplied one with `PREMULTIPLIED_ALPHA_BLENDING`
//! and scales all four channels by opacity × mask coverage. A canvas drawn
//! through the straight pipeline would double-multiply its alpha and every
//! translucent edge would darken.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

use crate::globals::Globals;
use crate::{mesh::scissor, ClipMask, Rect};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ImageInstance {
    pos: [f32; 2],
    size: [f32; 2],
    alpha: f32,
    /// The rounded-rect mask, in logical pixels, and its corner radius —
    /// the same (rect, radius) pair a mesh vertex carries, evaluated by the
    /// same SDF. A zero radius means "no mask" and costs one compare.
    mask: [f32; 4],
    mask_radius: f32,
}

struct Texture {
    bind_group: wgpu::BindGroup,
}

/// Where an image draw's pixels come from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ImageSource {
    /// A PNG on disk (straight alpha), cached by path.
    File(String),
    /// An offscreen canvas (premultiplied), by id; its bind group is
    /// published per frame with [`ImagePipeline::set_canvas`].
    Canvas(u32),
}

/// One image draw as staged for this frame.
pub(crate) struct ImageDraw<'a> {
    pub rect: &'a Rect,
    pub source: ImageSource,
    pub alpha: f32,
    pub clip: &'a Rect,
    pub mask: ClipMask,
}

struct Draw {
    source: ImageSource,
    clip: Rect,
}

pub(crate) struct ImagePipeline {
    /// Straight-alpha bitmaps.
    pipeline: wgpu::RenderPipeline,
    /// Premultiplied canvases.
    pipeline_premul: wgpu::RenderPipeline,
    globals: Globals,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    staging: Vec<ImageInstance>,
    textures: HashMap<String, Option<Texture>>,
    /// This frame's canvases, by id — rebuilt each frame by the renderer's
    /// scene walk, since a canvas texture is reallocated when its size
    /// changes.
    canvases: HashMap<u32, wgpu::BindGroup>,
    draws: Vec<Draw>,
}

impl ImagePipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("garden image shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
        });
        let globals = Globals::new(device, "garden image");
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("garden image texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("garden image pipeline layout"),
            bind_group_layouts: &[Some(globals.layout()), Some(&texture_layout)],
            immediate_size: 0,
        });
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ImageInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 16,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 20,
                    shader_location: 3,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 36,
                    shader_location: 4,
                },
            ],
        };
        let make = |label: &str, entry: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: std::slice::from_ref(&instance_layout),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                cache: None,
                multiview_mask: None,
            })
        };
        let pipeline = make(
            "garden image pipeline",
            "fs_main",
            wgpu::BlendState::ALPHA_BLENDING,
        );
        let pipeline_premul = make(
            "garden canvas pipeline",
            "fs_premul",
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("garden image sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("garden image instance"),
            size: std::mem::size_of::<ImageInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            pipeline_premul,
            globals,
            texture_layout,
            sampler,
            instance_buffer,
            instance_capacity: 1,
            staging: Vec::new(),
            textures: HashMap::new(),
            canvases: HashMap::new(),
            draws: Vec::new(),
        }
    }

    /// Publish target `slot`'s logical size and scale factor for this frame
    /// (slot 0 is the frame; each offscreen canvas has its own).
    pub fn set_target(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: usize,
        logical_size: (f32, f32),
        scale: f32,
    ) {
        self.globals.write(device, queue, slot, logical_size, scale);
    }

    /// Forget last frame's canvases. Called at the start of each scene walk;
    /// the walk re-publishes every canvas it creates.
    pub fn clear_canvases(&mut self) {
        self.canvases.clear();
    }

    /// Make canvas `id` drawable this frame through `view`.
    pub fn set_canvas(&mut self, device: &wgpu::Device, id: u32, view: &wgpu::TextureView) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("garden canvas sample"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.canvases.insert(id, bind_group);
    }

    pub fn prepare<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        images: impl Iterator<Item = ImageDraw<'a>>,
    ) {
        self.draws.clear();
        self.staging.clear();
        for draw in images {
            if let ImageSource::File(source) = &draw.source {
                if !self.textures.contains_key(source) {
                    let loaded =
                        load_texture(device, queue, &self.texture_layout, &self.sampler, source)
                            .map_err(|err| eprintln!("[garden image] {source}: {err}"))
                            .ok();
                    self.textures.insert(source.to_string(), loaded);
                }
            }
            // An image whose file is missing or unreadable still gets a slot:
            // `render` addresses draws by *scene* index, so skipping one here
            // would shift every later image onto another image's instance.
            // It is dropped at draw time instead, where its absent texture
            // simply produces no draw call.
            self.draws.push(Draw {
                source: draw.source,
                clip: *draw.clip,
            });
            let mask = draw.mask;
            self.staging.push(ImageInstance {
                pos: [draw.rect.x, draw.rect.y],
                size: [draw.rect.w, draw.rect.h],
                alpha: draw.alpha.clamp(0.0, 1.0),
                mask: [mask.rect.x, mask.rect.y, mask.rect.w, mask.rect.h],
                mask_radius: match mask.is_none() {
                    true => 0.0,
                    false => mask.radius,
                },
            });
        }
        if self.staging.len() > self.instance_capacity {
            self.instance_capacity = self.staging.len().next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("garden image instances"),
                size: (self.instance_capacity * std::mem::size_of::<ImageInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !self.staging.is_empty() {
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.staging),
            );
        }
    }

    /// `range` selects a half-open span of staged images, in the order
    /// [`prepare`] received them (scene order), so the scene can be drawn as
    /// interleaved per-kind batches and keep painter's order across kinds.
    pub fn render(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        slot: usize,
        physical_size: (u32, u32),
        scale_factor: f32,
        range: std::ops::Range<usize>,
    ) {
        let end = range.end.min(self.draws.len());
        if range.start >= end {
            return;
        }
        // The two pipelines share the vertex buffer and globals, so switching
        // between a bitmap and a canvas mid-batch is one `set_pipeline`.
        let mut bound: Option<bool> = None;
        for (index, draw) in self.draws.iter().enumerate().take(end).skip(range.start) {
            let (bind_group, premul) = match &draw.source {
                ImageSource::File(source) => {
                    match self.textures.get(source).and_then(Option::as_ref) {
                        Some(texture) => (&texture.bind_group, false),
                        None => continue,
                    }
                }
                ImageSource::Canvas(id) => match self.canvases.get(id) {
                    Some(bind_group) => (bind_group, true),
                    None => continue,
                },
            };
            let Some((x, y, w, h)) = scissor(draw.clip, physical_size, scale_factor) else {
                continue;
            };
            if bound != Some(premul) {
                pass.set_pipeline(match premul {
                    true => &self.pipeline_premul,
                    false => &self.pipeline,
                });
                self.globals.bind(pass, slot);
                pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
                bound = Some(premul);
            }
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(1, bind_group, &[]);
            pass.draw(0..4, index as u32..index as u32 + 1);
        }
        pass.set_scissor_rect(0, 0, physical_size.0, physical_size.1);
    }
}

fn load_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    source: &str,
) -> Result<Texture, String> {
    let decoder = png::Decoder::new(BufReader::new(
        File::open(source).map_err(|err| err.to_string())?,
    ));
    let mut reader = decoder.read_info().map_err(|err| err.to_string())?;
    let mut bytes = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut bytes)
        .map_err(|err| err.to_string())?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => bytes[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => bytes[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect(),
        other => {
            return Err(format!(
                "unsupported PNG color type {other:?}; use RGB or RGBA"
            ))
        }
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(source),
        size: wgpu::Extent3d {
            width: info.width,
            height: info.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // No transfer function: the PNG's bytes are already sRGB-encoded and
        // the scene composites in that space (see [`crate::Color`]), so the
        // sampler must hand the shader the bytes as stored. An `…Srgb` texture
        // would linearize them and the image would land brighter than every
        // shape drawn beside it.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * info.width),
            rows_per_image: Some(info.height),
        },
        wgpu::Extent3d {
            width: info.width,
            height: info.height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(source),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    Ok(Texture { bind_group })
}
