//! Cached PNG textures and the textured-quad pipeline.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

use crate::globals::Globals;
use crate::{mesh::scissor, Rect};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ImageInstance {
    pos: [f32; 2],
    size: [f32; 2],
    alpha: f32,
}

struct Texture {
    bind_group: wgpu::BindGroup,
}

struct Draw {
    source: String,
    clip: Rect,
}

pub(crate) struct ImagePipeline {
    pipeline: wgpu::RenderPipeline,
    globals: Globals,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    staging: Vec<ImageInstance>,
    textures: HashMap<String, Option<Texture>>,
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
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("garden image pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[instance_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
        });
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
            globals,
            texture_layout,
            sampler,
            instance_buffer,
            instance_capacity: 1,
            staging: Vec::new(),
            textures: HashMap::new(),
            draws: Vec::new(),
        }
    }

    pub fn prepare<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        logical_size: (f32, f32),
        scale: f32,
        images: impl Iterator<Item = (&'a Rect, &'a str, f32, &'a Rect)>,
    ) {
        self.draws.clear();
        self.staging.clear();
        for (rect, source, alpha, clip) in images {
            if !self.textures.contains_key(source) {
                let loaded =
                    load_texture(device, queue, &self.texture_layout, &self.sampler, source)
                        .map_err(|err| eprintln!("[garden image] {source}: {err}"))
                        .ok();
                self.textures.insert(source.to_string(), loaded);
            }
            // An image whose file is missing or unreadable still gets a slot:
            // `render` addresses draws by *scene* index, so skipping one here
            // would shift every later image onto another image's instance.
            // It is dropped at draw time instead, where its absent texture
            // simply produces no draw call.
            self.draws.push(Draw {
                source: source.to_string(),
                clip: *clip,
            });
            self.staging.push(ImageInstance {
                pos: [rect.x, rect.y],
                size: [rect.w, rect.h],
                alpha: alpha.clamp(0.0, 1.0),
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
        self.globals.write(queue, logical_size, scale);
    }

    /// `range` selects a half-open span of staged images, in the order
    /// [`prepare`] received them (scene order), so the scene can be drawn as
    /// interleaved per-kind batches and keep painter's order across kinds.
    pub fn render(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        physical_size: (u32, u32),
        scale_factor: f32,
        range: std::ops::Range<usize>,
    ) {
        let end = range.end.min(self.draws.len());
        if range.start >= end {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        self.globals.bind(pass);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        for (index, draw) in self.draws.iter().enumerate().take(end).skip(range.start) {
            let Some(texture) = self.textures.get(&draw.source).and_then(Option::as_ref) else {
                continue;
            };
            let Some((x, y, w, h)) = scissor(draw.clip, physical_size, scale_factor) else {
                continue;
            };
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(1, &texture.bind_group, &[]);
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
