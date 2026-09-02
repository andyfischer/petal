//! Flat-shaded triangle-list pipeline.
//!
//! The general geometry primitive behind [`crate::Primitive::Mesh`]. Where the
//! [`QuadPipeline`](crate::quad) draws axis-aligned rectangles by instancing a
//! unit quad, this pipeline draws an arbitrary CPU-tessellated triangle list:
//! one growable vertex buffer holding every mesh's vertices, drawn in `clip`
//! groups so each can be scissored to its pane.
//!
//! Clipping is a real GPU scissor (a triangle can't be clipped to a rect by
//! intersection the way a quad can), so each `Mesh` primitive becomes one draw
//! call with its own scissor rect.

use crate::globals::Globals;
use crate::{Rect, Vertex};

/// One vertex as laid out in the GPU vertex buffer.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshVertex {
    pos: [f32; 2],
    color: [f32; 4],
    /// The rounded-clip mask, as `(x, y, w, h)` in logical pixels…
    mask: [f32; 4],
    /// …and its corner radius. Zero — what every vertex Garden itself emits
    /// carries — short-circuits the mask in the fragment shader.
    mask_radius: f32,
}

/// One scissored draw: a vertex sub-range of the shared buffer and the clip
/// rect (logical pixels) it is scissored to.
struct DrawGroup {
    clip: Rect,
    start: u32,
    count: u32,
}

pub(crate) struct MeshPipeline {
    pipeline: wgpu::RenderPipeline,
    globals: Globals,
    vertex_buffer: wgpu::Buffer,
    /// Capacity of `vertex_buffer`, in vertices.
    capacity: usize,
    /// CPU-side staging area, reused across frames.
    staging: Vec<MeshVertex>,
    /// One entry per `Mesh` primitive staged this frame.
    groups: Vec<DrawGroup>,
}

const INITIAL_CAPACITY: usize = 256;

impl MeshPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat, samples: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("garden mesh shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mesh.wgsl").into()),
        });

        let globals = Globals::new(device, "garden mesh");

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("garden mesh pipeline layout"),
            bind_group_layouts: &[Some(globals.layout())],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                // pos
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                // color
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 8,
                    shader_location: 1,
                },
                // mask rect
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 24,
                    shader_location: 2,
                },
                // mask corner radius
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 40,
                    shader_location: 3,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("garden mesh pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[vertex_layout],
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
                topology: wgpu::PrimitiveTopology::TriangleList,
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

        let vertex_buffer = Self::create_vertex_buffer(device, INITIAL_CAPACITY);

        Self {
            pipeline,
            globals,
            vertex_buffer,
            capacity: INITIAL_CAPACITY,
            staging: Vec::with_capacity(INITIAL_CAPACITY),
            groups: Vec::new(),
        }
    }

    fn create_vertex_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("garden mesh vertices"),
            size: (capacity * std::mem::size_of::<MeshVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Upload this frame's meshes.
    ///
    /// `meshes` yields `(vertices, clip)` per `Mesh` primitive in submission
    /// order; all vertices are concatenated into one buffer and each mesh is
    /// recorded as a scissored draw group. The buffer is reallocated only when
    /// the total vertex count exceeds its capacity.
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

    pub fn prepare<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        meshes: impl Iterator<Item = (&'a [Vertex], &'a Rect)>,
    ) {
        self.staging.clear();
        self.groups.clear();
        for (vertices, clip) in meshes {
            // Drop a malformed mesh (not whole triangles) rather than render
            // garbage; callers tessellate, so this should not happen. It still
            // gets a group: `render` addresses groups by *scene* index, so
            // dropping one here would silently shift every later mesh's
            // scissor onto the wrong geometry.
            let count = vertices.len() - vertices.len() % 3;
            let start = self.staging.len() as u32;
            self.staging
                .extend(vertices[..count].iter().map(|v| MeshVertex {
                    pos: [v.pos.0, v.pos.1],
                    // sRGB-encoded, straight through — see `Color`.
                    color: v.color.to_array(),
                    mask: [v.mask.rect.x, v.mask.rect.y, v.mask.rect.w, v.mask.rect.h],
                    mask_radius: v.mask.radius,
                }));
            self.groups.push(DrawGroup {
                clip: *clip,
                start,
                count: count as u32,
            });
        }

        if self.staging.len() > self.capacity {
            self.capacity = self.staging.len().next_power_of_two();
            self.vertex_buffer = Self::create_vertex_buffer(device, self.capacity);
        }

        if !self.staging.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.staging));
        }
    }

    /// Record one scissored draw per staged mesh. `physical_size` and
    /// `scale_factor` convert each group's logical-pixel clip rect to the
    /// integer physical-pixel scissor wgpu wants.
    /// `range` selects a half-open span of staged meshes, in the order
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
        let end = range.end.min(self.groups.len());
        if range.start >= end {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        self.globals.bind(pass, slot);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));

        for g in &self.groups[range.start..end] {
            let Some((x, y, w, h)) = scissor(g.clip, physical_size, scale_factor) else {
                continue; // clip fully off-target → nothing visible
            };
            pass.set_scissor_rect(x, y, w, h);
            pass.draw(g.start..g.start + g.count, 0..1);
        }

        // Leave the scissor covering the whole target so whatever batch follows
        // isn't accidentally clipped to the last mesh's rect.
        pass.set_scissor_rect(0, 0, physical_size.0, physical_size.1);
    }
}

/// Convert a logical-pixel clip rect to an integer physical-pixel scissor,
/// clamped to the render target. Rounds outward (floor origin, ceil far edge)
/// to match glyphon's text clipping. Returns `None` if the result is empty.
pub(crate) fn scissor(
    clip: Rect,
    physical: (u32, u32),
    scale: f32,
) -> Option<(u32, u32, u32, u32)> {
    let (pw, ph) = (physical.0 as f32, physical.1 as f32);
    let left = (clip.x * scale).floor().clamp(0.0, pw);
    let top = (clip.y * scale).floor().clamp(0.0, ph);
    let right = ((clip.x + clip.w) * scale).ceil().clamp(0.0, pw);
    let bottom = ((clip.y + clip.h) * scale).ceil().clamp(0.0, ph);
    let (w, h) = (right - left, bottom - top);
    (w >= 1.0 && h >= 1.0).then_some((left as u32, top as u32, w as u32, h as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scissor_clamps_to_target() {
        // A clip extending past the target is clamped to it.
        let clip = Rect::new(10.0, 10.0, 1000.0, 1000.0);
        let s = scissor(clip, (200, 100), 1.0).unwrap();
        assert_eq!(s, (10, 10, 190, 90));
    }

    #[test]
    fn scissor_rounds_outward_with_scale() {
        // Fractional logical coords at 2x scale round outward on both edges.
        let clip = Rect::new(5.25, 5.25, 10.5, 10.5);
        // left = floor(10.5)=10, top=10, right=ceil(31.5)=32, bottom=32.
        let s = scissor(clip, (400, 400), 2.0).unwrap();
        assert_eq!(s, (10, 10, 22, 22));
    }

    #[test]
    fn scissor_offscreen_is_none() {
        let clip = Rect::new(-50.0, -50.0, 40.0, 40.0);
        assert_eq!(scissor(clip, (200, 200), 1.0), None);
    }
}
