//! NV12 → RGBA on the GPU with wgpu.
//!
//! Runs on the decoder thread, on the same `Device` and `Queue` as the Slint
//! renderer (wgpu's are thread-safe). Each frame's Y and UV planes are
//! uploaded to two textures and a shader converts them into an RGBA texture,
//! which the UI shows as a `slint::Image`. The UI thread only redraws.
//!
//! Submissions to one queue run in order on the GPU, so the UI samples either
//! the previous frame or the new one, never half of one.

use slint::wgpu_30::wgpu;

/// One decoded frame in NV12: a full-size Y plane and a half-size plane of
/// interleaved U and V. Rows may be padded (`*_stride` bytes apart).
pub struct Nv12<'a> {
    pub width: u32,
    pub height: u32,
    pub y: &'a [u8],
    pub y_stride: usize,
    pub uv: &'a [u8],
    pub uv_stride: usize,
    pub color: ColorInfo,
}

/// How the YUV values map to RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorInfo {
    /// BT.709 (HD) rather than BT.601 (SD) coefficients.
    pub bt709: bool,
    /// Full range (0-255) rather than limited (16-235 / 16-240).
    pub full_range: bool,
}

/// Uniform for the shader: three matrix rows and the offset, as vec4s.
pub fn color_uniform(info: ColorInfo) -> [[f32; 4]; 4] {
    let (kr, kb) = if info.bt709 {
        (0.2126, 0.0722)
    } else {
        (0.299, 0.114)
    };
    let kg = 1.0 - kr - kb;
    let (y_scale, c_scale, y_off) = if info.full_range {
        (1.0, 1.0, 0.0)
    } else {
        (255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0)
    };
    let c_off = 128.0 / 255.0;
    [
        [y_scale, 0.0, 2.0 * (1.0 - kr) * c_scale, 0.0],
        [
            y_scale,
            -2.0 * kb * (1.0 - kb) / kg * c_scale,
            -2.0 * kr * (1.0 - kr) / kg * c_scale,
            0.0,
        ],
        [y_scale, 2.0 * (1.0 - kb) * c_scale, 0.0, 0.0],
        [y_off, c_off, c_off, 0.0],
    ]
}

struct Targets {
    width: u32,
    height: u32,
    y: wgpu::Texture,
    uv: wgpu::Texture,
    out: wgpu::Texture,
    out_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

pub struct VideoRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    color: Option<ColorInfo>,
    targets: Option<Targets>,
}

/// Format of the texture the UI shows (the one Slint can import).
const OUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl VideoRenderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nv12"),
            source: wgpu::ShaderSource::Wgsl(include_str!("nv12.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nv12"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nv12"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nv12"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OUT_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nv12"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nv12 color"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            device,
            queue,
            pipeline,
            layout,
            sampler,
            uniform,
            color: None,
            targets: None,
        }
    }

    /// Uploads and converts one frame. Returns the output texture when it is
    /// new (first frame, or the video size changed), so the UI can switch to
    /// it; otherwise the UI only needs to redraw.
    pub fn render(&mut self, frame: &Nv12<'_>) -> Option<wgpu::Texture> {
        let mut new_texture = None;
        if self
            .targets
            .as_ref()
            .is_none_or(|t| (t.width, t.height) != (frame.width, frame.height))
        {
            let targets = self.create_targets(frame.width, frame.height);
            new_texture = Some(targets.out.clone());
            self.targets = Some(targets);
        }
        if self.color != Some(frame.color) {
            let uniform = color_uniform(frame.color);
            let bytes: Vec<u8> = uniform
                .iter()
                .flatten()
                .flat_map(|f| f.to_le_bytes())
                .collect();
            self.queue.write_buffer(&self.uniform, 0, &bytes);
            self.color = Some(frame.color);
        }
        let t = self.targets.as_ref().unwrap();

        let (cw, ch) = chroma_size(frame.width, frame.height);
        write_plane(
            &self.queue,
            &t.y,
            frame.y,
            frame.y_stride,
            frame.width,
            frame.height,
        );
        write_plane(&self.queue, &t.uv, frame.uv, frame.uv_stride, cw, ch);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nv12"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nv12"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.out_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &t.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        new_texture
    }

    fn create_targets(&self, width: u32, height: u32) -> Targets {
        let (cw, ch) = chroma_size(width, height);
        let texture = |label, w, h, format, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let plane_usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        let y = texture(
            "video y",
            width,
            height,
            wgpu::TextureFormat::R8Unorm,
            plane_usage,
        );
        let uv = texture(
            "video uv",
            cw,
            ch,
            wgpu::TextureFormat::Rg8Unorm,
            plane_usage,
        );
        // Slint needs TEXTURE_BINDING and RENDER_ATTACHMENT to import it.
        let out = texture(
            "video rgba",
            width,
            height,
            OUT_FORMAT,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let y_view = y.create_view(&Default::default());
        let uv_view = uv.create_view(&Default::default());
        let out_view = out.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nv12"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&y_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&uv_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        });
        Targets {
            width,
            height,
            y,
            uv,
            out,
            out_view,
            bind_group,
        }
    }
}

fn chroma_size(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(2), height.div_ceil(2))
}

/// Uploads `height` rows of a plane, `stride` bytes apart.
fn write_plane(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride as u32),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the shader computes, on the CPU.
    fn convert(info: ColorInfo, y: u8, u: u8, v: u8) -> [u8; 3] {
        let m = color_uniform(info);
        let yuv = [
            y as f32 / 255.0 - m[3][0],
            u as f32 / 255.0 - m[3][1],
            v as f32 / 255.0 - m[3][2],
        ];
        let row = |r: [f32; 4]| {
            let x = r[0] * yuv[0] + r[1] * yuv[1] + r[2] * yuv[2];
            (x.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        [row(m[0]), row(m[1]), row(m[2])]
    }

    #[test]
    fn limited_range_bt709() {
        let info = ColorInfo {
            bt709: true,
            full_range: false,
        };
        assert_eq!(convert(info, 16, 128, 128), [0, 0, 0]);
        assert_eq!(convert(info, 235, 128, 128), [255, 255, 255]);
        // BT.709 red: Y=63, Cb=102, Cr=240.
        let red = convert(info, 63, 102, 240);
        assert!(red[0] >= 253 && red[1] <= 2 && red[2] <= 2, "{red:?}");
    }

    #[test]
    fn full_range_bt601() {
        let info = ColorInfo {
            bt709: false,
            full_range: true,
        };
        assert_eq!(convert(info, 0, 128, 128), [0, 0, 0]);
        assert_eq!(convert(info, 255, 128, 128), [255, 255, 255]);
        // BT.601 full-range green: Y=150, Cb=44, Cr=21.
        let green = convert(info, 150, 44, 21);
        assert!(
            green[0] <= 2 && green[1] >= 253 && green[2] <= 2,
            "{green:?}"
        );
    }

    #[test]
    fn chroma_rounds_up() {
        assert_eq!(chroma_size(1920, 1080), (960, 540));
        assert_eq!(chroma_size(641, 361), (321, 181));
    }
}
