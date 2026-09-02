//! Offscreen-canvas filters: the separable Gaussian blur, and the plain
//! texture copy that it, the frame presentation, and the MSAA re-seed are
//! built from.
//!
//! A [`Blur`](crate::Primitive::Blur) runs as: halve the canvas down until
//! its sigma is at most [`MAX_SIGMA`] texels (each halving is a bilinear
//! copy, which averages 2×2 exactly), blur horizontally into a scratch, blur
//! vertically into another, and resample back up into the canvas. Working
//! at the reduced resolution is what keeps a 40 px wash behind a bar to a
//! handful of taps; the upsample is bilinear, and a blurred image has no
//! detail left for that to lose.
//!
//! Every pass reads one texture and writes another, so the module keeps a
//! pool of scratch textures (by size, reused across frames) and a pool of
//! parameter buffers (one per blur pass in the frame — a uniform written with
//! `write_buffer` lands before the command buffer runs, so passes cannot
//! share one).

use bytemuck::Zeroable;

/// The largest sigma, in texels of the texture being sampled, that is run
/// directly. Above it the source is halved first.
const MAX_SIGMA: f32 = 4.0;

/// How many halvings at most (1/8 resolution). A sigma past
/// `MAX_SIGMA * 8` physical pixels runs with more taps instead.
const MAX_HALVINGS: u32 = 3;

/// Taps each side of the centre at most; 3σ at [`MAX_SIGMA`] is 12.
const MAX_TAPS: i32 = 16;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    step: [f32; 2],
    sigma: f32,
    taps: i32,
}

struct ParamSlot {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// A pooled intermediate texture: sampled by one pass, drawn into by the
/// next.
pub(crate) struct Scratch {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    /// Claimed by this frame; released by [`Filters::begin_frame`].
    in_use: bool,
}

impl Scratch {
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

pub(crate) struct Filters {
    format: wgpu::TextureFormat,
    blur: wgpu::RenderPipeline,
    /// Single-sampled copy: downsample, upsample, present.
    copy: wgpu::RenderPipeline,
    /// The copy into a multisampled attachment, for re-seeding a canvas's
    /// MSAA target from its resolved texture after a filter changed it.
    copy_msaa: Option<wgpu::RenderPipeline>,
    params_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: Vec<ParamSlot>,
    params_used: usize,
    scratch: Vec<Scratch>,
}

impl Filters {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> Filters {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("garden filter shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blur.wgsl").into()),
        });
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("garden filter params layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("garden filter texture layout"),
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
            label: Some("garden filter pipeline layout"),
            bind_group_layouts: &[Some(&params_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let make = |label: &str, entry: &str, count: u32| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        // A filter *replaces* its destination; nothing here
                        // composites.
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count,
                    ..Default::default()
                },
                cache: None,
                multiview_mask: None,
            })
        };
        let blur = make("garden blur pipeline", "fs_blur", 1);
        let copy = make("garden copy pipeline", "fs_copy", 1);
        let copy_msaa =
            (samples > 1).then(|| make("garden copy pipeline (msaa)", "fs_copy", samples));
        // Clamp to edge: a blur near the canvas border smears the border
        // pixels outward rather than fading to transparent, which is what a
        // backdrop wants — a bar at the pane edge must not show a fringe.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("garden filter sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Filters {
            format,
            blur,
            copy,
            copy_msaa,
            params_layout,
            texture_layout,
            sampler,
            params: Vec::new(),
            params_used: 0,
            scratch: Vec::new(),
        }
    }

    /// Release every scratch texture and parameter slot claimed by the
    /// previous frame.
    pub fn begin_frame(&mut self) {
        self.params_used = 0;
        for s in &mut self.scratch {
            s.in_use = false;
        }
    }

    /// A bind group through which a filter pass can sample `view`.
    pub fn sample_bind_group(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("garden filter source"),
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
        })
    }

    /// Claim a free scratch texture of `size` (allocating one if the pool
    /// has none), returning its index. Released at the next
    /// [`begin_frame`](Self::begin_frame).
    pub fn acquire_scratch(&mut self, device: &wgpu::Device, size: (u32, u32)) -> usize {
        let size = (size.0.max(1), size.1.max(1));
        if let Some(i) = self
            .scratch
            .iter()
            .position(|s| !s.in_use && s.size == size)
        {
            self.scratch[i].in_use = true;
            return i;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("garden filter scratch"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
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
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.sample_bind_group(device, &view);
        self.scratch.push(Scratch {
            size,
            texture,
            view,
            bind_group,
            in_use: true,
        });
        self.scratch.len() - 1
    }

    pub fn scratch(&self, index: usize) -> &Scratch {
        &self.scratch[index]
    }

    /// Next free parameter slot, written with `params`.
    fn params_slot(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, params: Params) -> usize {
        if self.params_used == self.params.len() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("garden filter params"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("garden filter params"),
                layout: &self.params_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            self.params.push(ParamSlot { buffer, bind_group });
        }
        let index = self.params_used;
        self.params_used += 1;
        queue.write_buffer(&self.params[index].buffer, 0, bytemuck::bytes_of(&params));
        index
    }

    /// Record one full-target pass drawing `pipeline` from `source` into
    /// `target`, with parameter slot `params`.
    fn pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        params: usize,
        source: &wgpu::BindGroup,
        target: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("garden filter pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Every texel is overwritten by the full-screen triangle.
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.params[params].bind_group, &[]);
        pass.set_bind_group(1, source, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Copy (resample) `source` into `target`, single-sampled.
    pub fn copy(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::BindGroup,
        target: &wgpu::TextureView,
    ) {
        let params = self.params_slot(device, queue, Params::zeroed());
        self.pass(encoder, &self.copy, params, source, target);
    }

    /// Draw `source` over the whole of an already-begun **multisampled**
    /// pass — how a canvas's MSAA attachment is re-seeded from its resolved
    /// texture after a filter changed it. `None` when the renderer is not
    /// multisampling (there is nothing to re-seed then).
    pub fn copy_into_pass(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        source: &wgpu::BindGroup,
    ) -> bool {
        let Some(pipeline) = &self.copy_msaa else {
            return false;
        };
        // Any written params slot will do: `fs_copy` reads none. Slot 0 is
        // guaranteed by `reserve_copy_params`.
        let Some(slot) = self.params.first() else {
            return false;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &slot.bind_group, &[]);
        pass.set_bind_group(1, source, &[]);
        pass.draw(0..3, 0..1);
        true
    }

    /// Make sure a params slot exists for [`copy_into_pass`](Self::copy_into_pass),
    /// which runs inside a pass and cannot allocate one.
    ///
    /// The reservation must not *claim* the slot: `fs_copy` reads no
    /// parameters, so slot 0 stays free for a real pass to write. Restoring
    /// the mark rather than zeroing it keeps that true wherever this is
    /// called from.
    pub fn reserve_copy_params(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.params.is_empty() {
            let used = self.params_used;
            self.params_slot(device, queue, Params::zeroed());
            self.params_used = used;
        }
    }

    /// Gaussian-blur `canvas` (of physical `size`, sampled through
    /// `canvas_sample`) in place with standard deviation `sigma` physical
    /// pixels. A sigma under a quarter pixel is a no-op.
    #[allow(clippy::too_many_arguments)]
    pub fn blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        canvas_sample: &wgpu::BindGroup,
        canvas_view: &wgpu::TextureView,
        size: (u32, u32),
        sigma: f32,
    ) {
        if sigma < 0.25 || size.0 == 0 || size.1 == 0 {
            return;
        }
        // Halve until the sigma fits the direct kernel.
        let mut halvings = 0;
        let mut sigma_at = sigma;
        let mut work = size;
        while sigma_at > MAX_SIGMA && halvings < MAX_HALVINGS && work.0 > 8 && work.1 > 8 {
            halvings += 1;
            sigma_at /= 2.0;
            work = ((work.0 / 2).max(1), (work.1 / 2).max(1));
        }
        let taps = ((sigma_at * 3.0).ceil() as i32).clamp(1, MAX_TAPS);

        // Downsample chain: canvas → half → quarter → …
        let mut src_index: Option<usize> = None;
        let mut cur = size;
        for _ in 0..halvings {
            cur = ((cur.0 / 2).max(1), (cur.1 / 2).max(1));
            let dst = self.acquire_scratch(device, cur);
            let params = self.params_slot(device, queue, Params::zeroed());
            let src_bg = match src_index {
                Some(i) => &self.scratch[i].bind_group,
                None => canvas_sample,
            };
            // Borrow juggling: `pass` needs `&self` while `dst` is an index.
            self.pass(encoder, &self.copy, params, src_bg, &self.scratch[dst].view);
            src_index = Some(dst);
        }

        // Horizontal into scratch A, vertical into scratch B (or, with no
        // halving, straight back into the canvas).
        let a = self.acquire_scratch(device, work);
        let h = self.params_slot(
            device,
            queue,
            Params {
                step: [1.0 / work.0 as f32, 0.0],
                sigma: sigma_at,
                taps,
            },
        );
        let src_bg = match src_index {
            Some(i) => &self.scratch[i].bind_group,
            None => canvas_sample,
        };
        self.pass(encoder, &self.blur, h, src_bg, &self.scratch[a].view);

        let v = self.params_slot(
            device,
            queue,
            Params {
                step: [0.0, 1.0 / work.1 as f32],
                sigma: sigma_at,
                taps,
            },
        );
        if halvings == 0 {
            self.pass(
                encoder,
                &self.blur,
                v,
                &self.scratch[a].bind_group,
                canvas_view,
            );
        } else {
            let b = self.acquire_scratch(device, work);
            self.pass(
                encoder,
                &self.blur,
                v,
                &self.scratch[a].bind_group,
                &self.scratch[b].view,
            );
            // Back up to the canvas in one bilinear step: the image is
            // smooth now, so the intermediate levels add nothing.
            let up = self.params_slot(device, queue, Params::zeroed());
            self.pass(
                encoder,
                &self.copy,
                up,
                &self.scratch[b].bind_group,
                canvas_view,
            );
        }
    }
}
