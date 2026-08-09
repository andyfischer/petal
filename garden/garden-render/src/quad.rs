//! Instanced solid-quad pipeline.
//!
//! All quads in a scene are drawn with a single instanced draw call: one
//! per-instance vertex buffer of `{rect, color}` records, with a unit quad
//! expanded in the vertex shader (4-vertex triangle strip). The instance
//! buffer is rebuilt every frame but only reallocated when its capacity
//! grows.

use crate::globals::Globals;
use crate::{Color, Rect};

/// One quad instance as laid out in the GPU vertex buffer.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadInstance {
    pos: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

pub(crate) struct QuadPipeline {
    pipeline: wgpu::RenderPipeline,
    globals: Globals,
    instance_buffer: wgpu::Buffer,
    /// Capacity of `instance_buffer`, in instances.
    capacity: usize,
    /// Number of instances staged for the current frame.
    count: usize,
    /// CPU-side staging area, reused across frames.
    staging: Vec<QuadInstance>,
}

const INITIAL_CAPACITY: usize = 64;

impl QuadPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat, samples: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("garden quad shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });

        let globals = Globals::new(device, "garden quad");

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("garden quad pipeline layout"),
            bind_group_layouts: &[Some(globals.layout())],
            immediate_size: 0,
        });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                // pos
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                // size
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                // color
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 2,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("garden quad pipeline"),
            layout: Some(&pipeline_layout),
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
                    format: surface_format,
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

        let instance_buffer = Self::create_instance_buffer(device, INITIAL_CAPACITY);

        Self {
            pipeline,
            globals,
            instance_buffer,
            capacity: INITIAL_CAPACITY,
            count: 0,
            staging: Vec::with_capacity(INITIAL_CAPACITY),
        }
    }

    fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("garden quad instances"),
            size: (capacity * std::mem::size_of::<QuadInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Upload this frame's quads and the logical screen size.
    ///
    /// `quads` yields `(rect, color)` in logical pixels; the instance buffer
    /// is reallocated only when the number of quads exceeds its capacity.
    pub fn prepare<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        logical_size: (f32, f32),
        quads: impl Iterator<Item = (&'a Rect, &'a Color)>,
    ) {
        self.staging.clear();
        self.staging.extend(quads.map(|(rect, color)| QuadInstance {
            pos: [rect.x, rect.y],
            size: [rect.w, rect.h],
            // Linear space: the sRGB surface re-encodes on store.
            color: color.to_linear(),
        }));
        self.count = self.staging.len();

        if self.count > self.capacity {
            self.capacity = self.count.next_power_of_two();
            self.instance_buffer = Self::create_instance_buffer(device, self.capacity);
        }

        self.globals.write(queue, logical_size);
        if self.count > 0 {
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.staging),
            );
        }
    }

    /// Record one instanced draw call for everything staged by [`prepare`].
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        self.globals.bind(pass);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        pass.draw(0..4, 0..self.count as u32);
    }
}
