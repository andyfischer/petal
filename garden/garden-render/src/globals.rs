//! The per-frame uniform block shared by every scene pipeline.
//!
//! Both the [`QuadPipeline`](crate::quad) and the [`MeshPipeline`](crate::mesh)
//! bind exactly one uniform at `@group(0) @binding(0)`: the logical screen size
//! their vertex shaders divide by to map pixels to clip space. The buffer, its
//! bind-group layout, and the bind group are byte-for-byte identical between the
//! two, so that plumbing lives here once instead of in each pipeline.

/// Uniforms shared by all primitives in a pipeline (16-byte aligned for WGSL).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    screen_size: [f32; 2],
    _pad: [f32; 2],
}

/// The globals uniform buffer plus the bind group that exposes it — always
/// group 0, binding 0, vertex stage. A pipeline holds one of these, uses
/// [`layout`](Self::layout) when building its pipeline layout, calls
/// [`write`](Self::write) each frame, and [`bind`](Self::bind)s it before
/// drawing.
pub(crate) struct Globals {
    buffer: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
}

impl Globals {
    /// Create the buffer and bind group. `label` is a short pipeline name
    /// (e.g. `"garden quad"`) used to prefix the wgpu debug labels.
    pub fn new(device: &wgpu::Device, label: &str) -> Globals {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{label} globals")),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{label} bind group layout")),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label} bind group")),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        Globals {
            buffer,
            layout,
            bind_group,
        }
    }

    /// The bind-group layout, for building the owning pipeline's layout.
    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Upload the logical screen size for this frame (floored to 1×1 so the
    /// shader never divides by zero).
    pub fn write(&self, queue: &wgpu::Queue, logical_size: (f32, f32)) {
        let uniforms = Uniforms {
            screen_size: [logical_size.0.max(1.0), logical_size.1.max(1.0)],
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Bind the globals at group 0 for the current pass.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.bind_group, &[]);
    }
}
