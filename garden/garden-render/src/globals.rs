//! The per-target uniform block shared by every scene pipeline.
//!
//! The [`QuadPipeline`](crate::quad), [`MeshPipeline`](crate::mesh) and
//! [`ImagePipeline`](crate::image) bind exactly one uniform at
//! `@group(0) @binding(0)`: the logical size of the target their vertex
//! shaders divide by to map pixels to clip space, plus the scale factor the
//! clip mask feathers by. The buffer, its bind-group layout, and the bind
//! group are byte-for-byte identical between them, so that plumbing lives
//! here once instead of in each pipeline.
//!
//! There is one *slot* per render target a frame draws into — slot 0 is the
//! frame, and each offscreen canvas gets its own — because a uniform written
//! with `queue.write_buffer` lands before the command buffer runs, so a single
//! buffer rewritten between passes would leave every pass seeing the last
//! value. A slot is a separate buffer + bind group, chosen per draw.

/// Uniforms shared by all primitives in a pipeline (16-byte aligned for WGSL).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    screen_size: [f32; 2],
    /// Physical pixels per logical pixel. The vertex stage doesn't need it —
    /// it works in logical units throughout — but the fragment stage does: a
    /// [`crate::ClipMask`]'s antialiased edge has to feather across one
    /// *device* pixel, or a rounded crop is soft on a Retina display and hard
    /// on a 1x one.
    scale: f32,
    _pad: f32,
}

struct Slot {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// The globals uniform buffers plus the bind groups that expose them — always
/// group 0, binding 0. A pipeline holds one of these, uses
/// [`layout`](Self::layout) when building its pipeline layout, has every
/// target's slot [`write`](Self::write)n each frame, and
/// [`bind`](Self::bind)s the right slot before drawing into that target.
pub(crate) struct Globals {
    label: String,
    layout: wgpu::BindGroupLayout,
    slots: Vec<Slot>,
}

impl Globals {
    /// Create the layout and the frame's slot. `label` is a short pipeline
    /// name (e.g. `"garden quad"`) used to prefix the wgpu debug labels.
    pub fn new(device: &wgpu::Device, label: &str) -> Globals {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{label} bind group layout")),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let mut globals = Globals {
            label: label.to_string(),
            layout,
            slots: Vec::new(),
        };
        globals.ensure_slot(device, 0);
        globals
    }

    /// The bind-group layout, for building the owning pipeline's layout.
    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    fn ensure_slot(&mut self, device: &wgpu::Device, slot: usize) {
        while self.slots.len() <= slot {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("{} globals {}", self.label, self.slots.len())),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("{} bind group {}", self.label, self.slots.len())),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.slots.push(Slot { buffer, bind_group });
        }
    }

    /// Upload the logical size and scale factor of target `slot` for this
    /// frame (the size floored to 1×1 so the shader never divides by zero),
    /// growing the slot pool as needed.
    pub fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: usize,
        logical_size: (f32, f32),
        scale: f32,
    ) {
        self.ensure_slot(device, slot);
        let uniforms = Uniforms {
            screen_size: [logical_size.0.max(1.0), logical_size.1.max(1.0)],
            scale: scale.max(f32::MIN_POSITIVE),
            _pad: 0.0,
        };
        queue.write_buffer(&self.slots[slot].buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Bind target `slot`'s globals at group 0 for the current pass. An
    /// unwritten slot falls back to the frame's.
    pub fn bind(&self, pass: &mut wgpu::RenderPass<'_>, slot: usize) {
        let slot = self.slots.get(slot).unwrap_or(&self.slots[0]);
        pass.set_bind_group(0, &slot.bind_group, &[]);
    }
}
