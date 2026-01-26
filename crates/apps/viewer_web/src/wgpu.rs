#[cfg(target_arch = "wasm32")]
mod imp {
    use ::wgpu::util::DeviceExt;
    use std::borrow::Cow;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    #[derive(Debug)]
    pub struct WgpuContext {
        pub _instance: &'static ::wgpu::Instance,
        pub surface: ::wgpu::Surface<'static>,
        pub device: ::wgpu::Device,
        pub queue: ::wgpu::Queue,
        pub config: ::wgpu::SurfaceConfiguration,
        pub _canvas: web_sys::HtmlCanvasElement,
        pub stars_pipeline: ::wgpu::RenderPipeline,
        pub stars_count: u32,
        pub pipeline: ::wgpu::RenderPipeline,
        pub graticule_pipeline: ::wgpu::RenderPipeline,
        pub cities_pipeline: ::wgpu::RenderPipeline,
        pub uniform_buffer: ::wgpu::Buffer,
        pub uniform_bind_group: ::wgpu::BindGroup,
        pub depth_view: ::wgpu::TextureView,
        pub vertex_buffer: ::wgpu::Buffer,
        pub index_buffer: ::wgpu::Buffer,
        pub index_count: u32,
        pub graticule_vertex_buffer: ::wgpu::Buffer,
        pub graticule_vertex_count: u32,
        pub cities_vertex_buffer: ::wgpu::Buffer,
        pub cities_vertex_count: u32,
    }

    const GLOBE_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> VsOut {
    return VsOut(
        globals.view_proj * vec4<f32>(position, 1.0),
        normal,
    );
}

@fragment
fn fs_main(fs_in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(fs_in.normal);
    let l = normalize(globals.light_dir);
    let ndotl = max(dot(n, l), 0.0);

    // Simple globe-ish color ramp.
    let base = vec3<f32>(0.10, 0.55, 0.85);
    let shade = 0.25 + 0.75 * ndotl;
    return vec4<f32>(base * shade, 1.0);
}
"#;

    const GRATICULE_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return globals.view_proj * vec4<f32>(position, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Light blue overlay lines.
    return vec4<f32>(0.65, 0.85, 1.0, 1.0);
}
"#;

    const STARS_SHADER: &str = r#"
fn hash_u32(x_in: u32) -> u32 {
    // 32-bit integer mix (non-linear) to avoid visible correlation patterns.
    var x = x_in;
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    x ^= x >> 16u;
    return x;
}

fn hash01(x: u32) -> f32 {
    return f32(hash_u32(x)) / 4294967295.0;
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) a: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Deterministic pseudo-random star positions in clip space.
    // Use different salts per component to avoid structure.
    let rx = hash01(vid ^ 0x68bc21ebu);
    let ry = hash01(vid ^ 0x02e5be93u);
    let rb = hash01(vid ^ 0x9e3779b9u);

    let x = rx * 2.0 - 1.0;
    let y = ry * 2.0 - 1.0;
    // Slightly vary brightness; keep faint stars common.
    // Keep overall brightness conservative to avoid a "snow" look.
    let a = 0.03 + 0.22 * rb * rb;

    return VsOut(vec4<f32>(x, y, 0.9999, 1.0), a);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, in.a);
}
"#;

    const CITIES_SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    _pad: f32,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> VsOut {
    return VsOut(globals.view_proj * vec4<f32>(position, 1.0));
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    // Bright city markers.
    return vec4<f32>(1.0, 0.25, 0.25, 0.95);
}
"#;

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct Vertex {
        position: [f32; 3],
        normal: [f32; 3],
    }

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct LineVertex {
        position: [f32; 3],
    }

    #[repr(C)]
    #[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    pub struct CityVertex {
        pub position: [f32; 3],
        pub _pad: f32,
    }

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct Globals {
        view_proj: [[f32; 4]; 4],
        light_dir: [f32; 3],
        _pad: f32,
    }

    fn create_depth_view(
        device: &::wgpu::Device,
        config: &::wgpu::SurfaceConfiguration,
    ) -> ::wgpu::TextureView {
        let tex = device.create_texture(&::wgpu::TextureDescriptor {
            label: Some("atlas-depth"),
            size: ::wgpu::Extent3d {
                width: config.width.max(1),
                height: config.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: ::wgpu::TextureDimension::D2,
            format: ::wgpu::TextureFormat::Depth24Plus,
            usage: ::wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        tex.create_view(&::wgpu::TextureViewDescriptor::default())
    }

    fn generate_sphere_mesh(lat_segments: u32, lon_segments: u32) -> (Vec<Vertex>, Vec<u16>) {
        let lat_segments = lat_segments.max(3);
        let lon_segments = lon_segments.max(3);

        let mut vertices = Vec::with_capacity(((lat_segments + 1) * (lon_segments + 1)) as usize);
        for lat in 0..=lat_segments {
            let v = lat as f32 / lat_segments as f32;
            let theta = v * std::f32::consts::PI;
            let sin_t = theta.sin();
            let cos_t = theta.cos();

            for lon in 0..=lon_segments {
                let u = lon as f32 / lon_segments as f32;
                let phi = u * std::f32::consts::TAU;
                let sin_p = phi.sin();
                let cos_p = phi.cos();

                let x = sin_t * cos_p;
                let y = cos_t;
                let z = sin_t * sin_p;
                vertices.push(Vertex {
                    position: [x, y, z],
                    normal: [x, y, z],
                });
            }
        }

        let stride = lon_segments + 1;
        let mut indices = Vec::with_capacity((lat_segments * lon_segments * 6) as usize);
        for lat in 0..lat_segments {
            for lon in 0..lon_segments {
                let i0 = lat * stride + lon;
                let i1 = i0 + 1;
                let i2 = i0 + stride;
                let i3 = i2 + 1;

                indices.push(i0 as u16);
                indices.push(i2 as u16);
                indices.push(i1 as u16);
                indices.push(i1 as u16);
                indices.push(i2 as u16);
                indices.push(i3 as u16);
            }
        }

        (vertices, indices)
    }

    fn lat_lon_to_unit(lat_rad: f32, lon_rad: f32) -> [f32; 3] {
        let cos_lat = lat_rad.cos();
        let sin_lat = lat_rad.sin();
        let cos_lon = lon_rad.cos();
        let sin_lon = lon_rad.sin();
        [cos_lat * cos_lon, sin_lat, cos_lat * sin_lon]
    }

    fn generate_graticule_lines() -> Vec<LineVertex> {
        // Simple graticule: meridians every 15°, parallels every 15°.
        // Built as a LineList: (p0,p1),(p1,p2),...
        let mut verts = Vec::new();

        let meridian_step_deg: i32 = 15;
        let parallel_step_deg: i32 = 15;
        let samples: i32 = 128;

        // Meridians: lon fixed, lat varies -90..90.
        for lon_deg in (0..360).step_by(meridian_step_deg as usize) {
            let lon = (lon_deg as f32).to_radians();
            let mut prev = None;
            for i in 0..=samples {
                let t = i as f32 / samples as f32;
                let lat = (-90.0 + 180.0 * t) as f32;
                let p = lat_lon_to_unit(lat.to_radians(), lon);
                if let Some(prev_p) = prev {
                    verts.push(LineVertex { position: prev_p });
                    verts.push(LineVertex { position: p });
                }
                prev = Some(p);
            }
        }

        // Parallels: lat fixed, lon varies -180..180.
        for lat_deg in (-75..=75).step_by(parallel_step_deg as usize) {
            let lat = (lat_deg as f32).to_radians();
            let mut prev = None;
            for i in 0..=samples {
                let t = i as f32 / samples as f32;
                let lon = (-180.0 + 360.0 * t) as f32;
                let p = lat_lon_to_unit(lat, lon.to_radians());
                if let Some(prev_p) = prev {
                    verts.push(LineVertex { position: prev_p });
                    verts.push(LineVertex { position: p });
                }
                prev = Some(p);
            }
        }

        verts
    }

    pub async fn init_wgpu_from_canvas_id(canvas_id: &str) -> Result<WgpuContext, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("window missing"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("document missing"))?;
        let canvas_elem = document
            .get_element_by_id(canvas_id)
            .ok_or_else(|| JsValue::from_str("canvas missing"))?
            .dyn_into::<web_sys::HtmlCanvasElement>()?;

        let width = canvas_elem.width();
        let height = canvas_elem.height();

        // IMPORTANT: `wgpu::Surface` must not outlive its `wgpu::Instance`.
        // To avoid UB, we leak the instance for the lifetime of the app.
        //
        // Prefer WebGPU when available, but allow WebGL as a fallback.
        let instance: &'static ::wgpu::Instance = Box::leak(Box::new(::wgpu::Instance::new(
            &::wgpu::InstanceDescriptor {
                backends: ::wgpu::Backends::BROWSER_WEBGPU | ::wgpu::Backends::GL,
                ..Default::default()
            },
        )));

        let surface = instance
            .create_surface(::wgpu::SurfaceTarget::Canvas(canvas_elem.clone()))
            .map_err(|e| JsValue::from_str(&format!("surface error: {e}")))?;

        let adapter = instance
            .request_adapter(&::wgpu::RequestAdapterOptions {
                power_preference: ::wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("adapter error: {e}")))?;

        let (device, queue) = adapter
            .request_device(&::wgpu::DeviceDescriptor {
                label: Some("atlas-wgpu-device"),
                required_features: ::wgpu::Features::empty(),
                required_limits: ::wgpu::Limits::downlevel_webgl2_defaults(),
                ..Default::default()
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("device error: {e}")))?;

        // `adapter`, `device`, `queue` created above.

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .cloned()
            .find(|f| f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);

        let config = ::wgpu::SurfaceConfiguration {
            usage: ::wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            desired_maximum_frame_latency: 2,
            present_mode: ::wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let depth_view = create_depth_view(&device, &config);

        let shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-globe-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(GLOBE_SHADER)),
        });

        let graticule_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-graticule-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(GRATICULE_SHADER)),
        });

        let stars_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-stars-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(STARS_SHADER)),
        });

        let cities_shader = device.create_shader_module(::wgpu::ShaderModuleDescriptor {
            label: Some("atlas-cities-shader"),
            source: ::wgpu::ShaderSource::Wgsl(Cow::Borrowed(CITIES_SHADER)),
        });

        let uniform_buffer = device.create_buffer(&::wgpu::BufferDescriptor {
            label: Some("atlas-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: ::wgpu::BufferUsages::STORAGE | ::wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&::wgpu::BindGroupLayoutDescriptor {
                label: Some("atlas-globals-bgl"),
                entries: &[::wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ::wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: ::wgpu::BindingType::Buffer {
                        ty: ::wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let uniform_bind_group = device.create_bind_group(&::wgpu::BindGroupDescriptor {
            label: Some("atlas-globals-bg"),
            layout: &uniform_bind_group_layout,
            entries: &[::wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&::wgpu::PipelineLayoutDescriptor {
            label: Some("atlas-globe-pipeline-layout"),
            bind_group_layouts: &[&uniform_bind_group_layout],
            immediate_size: 0,
        });

        let stars_pipeline_layout =
            device.create_pipeline_layout(&::wgpu::PipelineLayoutDescriptor {
                label: Some("atlas-stars-pipeline-layout"),
                bind_group_layouts: &[],
                immediate_size: 0,
            });

        // Starfield background: generated procedurally via vertex_index.
        let stars_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-stars-pipeline"),
            layout: Some(&stars_pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &stars_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &stars_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::PointList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-globe-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[::wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as ::wgpu::BufferAddress,
                    step_mode: ::wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        ::wgpu::VertexAttribute {
                            format: ::wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::REPLACE),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                // Disable culling for now. If winding ends up opposite what we expect
                // (common when generating sphere indices), culling will make the globe
                // completely disappear and you'll only see the clear color.
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(::wgpu::DepthStencilState {
                format: ::wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: true,
                depth_compare: ::wgpu::CompareFunction::Less,
                stencil: ::wgpu::StencilState::default(),
                bias: ::wgpu::DepthBiasState::default(),
            }),
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let graticule_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-graticule-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &graticule_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[::wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineVertex>() as ::wgpu::BufferAddress,
                    step_mode: ::wgpu::VertexStepMode::Vertex,
                    attributes: &[::wgpu::VertexAttribute {
                        format: ::wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &graticule_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::LineList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            // Keep this pipeline depthless; depth bias / depth state can be a source of
            // backend-specific issues on WebGL.
            depth_stencil: None,
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let cities_pipeline = device.create_render_pipeline(&::wgpu::RenderPipelineDescriptor {
            label: Some("atlas-cities-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: ::wgpu::VertexState {
                module: &cities_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[::wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CityVertex>() as ::wgpu::BufferAddress,
                    step_mode: ::wgpu::VertexStepMode::Vertex,
                    attributes: &[::wgpu::VertexAttribute {
                        format: ::wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
            },
            fragment: Some(::wgpu::FragmentState {
                module: &cities_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(::wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: ::wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: ::wgpu::PrimitiveState {
                topology: ::wgpu::PrimitiveTopology::PointList,
                strip_index_format: None,
                front_face: ::wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: ::wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(::wgpu::DepthStencilState {
                format: ::wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: false,
                depth_compare: ::wgpu::CompareFunction::LessEqual,
                stencil: ::wgpu::StencilState::default(),
                bias: ::wgpu::DepthBiasState::default(),
            }),
            multisample: ::wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let (vertices, indices) = generate_sphere_mesh(64, 128);
        let vertex_buffer = device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
            label: Some("atlas-globe-vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: ::wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
            label: Some("atlas-globe-indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: ::wgpu::BufferUsages::INDEX,
        });

        let graticule_vertices = generate_graticule_lines();
        let graticule_vertex_buffer =
            device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                label: Some("atlas-graticule-vertices"),
                contents: bytemuck::cast_slice(&graticule_vertices),
                usage: ::wgpu::BufferUsages::VERTEX,
            });

        let cities_vertex_buffer = device.create_buffer_init(&::wgpu::util::BufferInitDescriptor {
            label: Some("atlas-cities-vertices"),
            contents: bytemuck::bytes_of(&CityVertex {
                position: [0.0, 0.0, 0.0],
                _pad: 0.0,
            }),
            usage: ::wgpu::BufferUsages::VERTEX | ::wgpu::BufferUsages::COPY_DST,
        });

        // Initialize uniforms so the first render doesn't read uninitialized memory.
        let globals = Globals {
            view_proj: [[0.0; 4]; 4],
            light_dir: [0.4, 0.7, 0.2],
            _pad: 0.0,
        };
        queue.write_buffer(&uniform_buffer, 0, bytemuck::bytes_of(&globals));

        Ok(WgpuContext {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            _canvas: canvas_elem,
            stars_pipeline,
            stars_count: 1200,
            pipeline,
            graticule_pipeline,
            cities_pipeline,
            uniform_buffer,
            uniform_bind_group,
            depth_view,
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            graticule_vertex_buffer,
            graticule_vertex_count: graticule_vertices.len() as u32,
            cities_vertex_buffer,
            cities_vertex_count: 0,
        })
    }

    pub fn set_cities_points(ctx: &mut WgpuContext, points: &[CityVertex]) {
        if points.is_empty() {
            ctx.cities_vertex_count = 0;
            return;
        }

        ctx.cities_vertex_buffer =
            ctx.device
                .create_buffer_init(&::wgpu::util::BufferInitDescriptor {
                    label: Some("atlas-cities-vertices"),
                    contents: bytemuck::cast_slice(points),
                    usage: ::wgpu::BufferUsages::VERTEX,
                });
        ctx.cities_vertex_count = points.len() as u32;
    }

    pub fn resize_wgpu(ctx: &mut WgpuContext, width: u32, height: u32) {
        ctx.config.width = width.max(1);
        ctx.config.height = height.max(1);
        ctx.surface.configure(&ctx.device, &ctx.config);
        ctx.depth_view = create_depth_view(&ctx.device, &ctx.config);
    }

    pub fn render_mesh(
        ctx: &WgpuContext,
        view_proj: [[f32; 4]; 4],
        show_graticule: bool,
        show_cities: bool,
    ) -> Result<(), JsValue> {
        let frame = ctx
            .surface
            .get_current_texture()
            .map_err(|e| JsValue::from_str(&format!("surface acquire failed: {e}")))?;
        let view = frame
            .texture
            .create_view(&::wgpu::TextureViewDescriptor::default());

        let globals = Globals {
            view_proj,
            light_dir: [0.4, 0.7, 0.2],
            _pad: 0.0,
        };
        ctx.queue
            .write_buffer(&ctx.uniform_buffer, 0, bytemuck::bytes_of(&globals));

        let mut encoder = ctx
            .device
            .create_command_encoder(&::wgpu::CommandEncoderDescriptor {
                label: Some("atlas-mesh-encoder"),
            });

        // Pass 1: clear to deep space and draw stars (no depth attachment).
        {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-stars-pass"),
                color_attachments: &[Some(::wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: ::wgpu::Operations {
                        load: ::wgpu::LoadOp::Clear(::wgpu::Color {
                            r: 0.004,
                            g: 0.008,
                            b: 0.016,
                            a: 1.0,
                        }),
                        store: ::wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.stars_pipeline);
            rpass.draw(0..ctx.stars_count, 0..1);
        }

        // Pass 2: draw globe with depth, preserving the starfield color.
        {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-globe-pass"),
                color_attachments: &[Some(::wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: ::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(::wgpu::RenderPassDepthStencilAttachment {
                    view: &ctx.depth_view,
                    depth_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Clear(1.0),
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
            rpass.set_index_buffer(ctx.index_buffer.slice(..), ::wgpu::IndexFormat::Uint16);
            rpass.draw_indexed(0..ctx.index_count, 0, 0..1);
        }

        // Pass 3 (optional): graticule overlay (depthless, alpha blended).
        if show_graticule {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-graticule-pass"),
                color_attachments: &[Some(::wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: ::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.graticule_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_vertex_buffer(0, ctx.graticule_vertex_buffer.slice(..));
            rpass.draw(0..ctx.graticule_vertex_count, 0..1);
        }

        // Pass 4 (optional): city markers (depth-tested points).
        if show_cities && ctx.cities_vertex_count > 0 {
            let mut rpass = encoder.begin_render_pass(&::wgpu::RenderPassDescriptor {
                label: Some("atlas-cities-pass"),
                color_attachments: &[Some(::wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: ::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(::wgpu::RenderPassDepthStencilAttachment {
                    view: &ctx.depth_view,
                    depth_ops: Some(::wgpu::Operations {
                        load: ::wgpu::LoadOp::Load,
                        store: ::wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });

            rpass.set_pipeline(&ctx.cities_pipeline);
            rpass.set_bind_group(0, &ctx.uniform_bind_group, &[]);
            rpass.set_vertex_buffer(0, ctx.cities_vertex_buffer.slice(..));
            rpass.draw(0..ctx.cities_vertex_count, 0..1);
        }

        ctx.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use wasm_bindgen::prelude::JsValue;

    #[derive(Debug, Default)]
    pub struct WgpuContext;

    pub async fn init_wgpu_from_canvas_id(_canvas_id: &str) -> Result<WgpuContext, JsValue> {
        Err(JsValue::from_str(
            "wgpu initialization is only available on wasm32 targets",
        ))
    }

    pub fn resize_wgpu(_ctx: &mut WgpuContext, _width: u32, _height: u32) {}

    #[derive(Debug, Copy, Clone)]
    #[allow(dead_code)]
    pub struct CityVertex {
        pub position: [f32; 3],
        pub _pad: f32,
    }

    pub fn set_cities_points(_ctx: &mut WgpuContext, _points: &[CityVertex]) {}

    pub fn render_mesh(
        _ctx: &WgpuContext,
        _view_proj: [[f32; 4]; 4],
        _show_graticule: bool,
        _show_cities: bool,
    ) -> Result<(), JsValue> {
        Err(JsValue::from_str(
            "wgpu rendering is only available on wasm32 targets",
        ))
    }
}

pub use imp::{
    CityVertex, WgpuContext, init_wgpu_from_canvas_id, render_mesh, resize_wgpu, set_cities_points,
};
