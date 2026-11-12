//! Affichage par la carte graphique (wgpu : DirectX 12 / Vulkan / Metal).
//! L'image capturée est envoyée telle quelle dans une texture ; le shader fait l'agrandissement.

use std::sync::Arc;
use winit::{dpi::PhysicalSize, window::Window};

#[derive(Clone, Copy, PartialEq)]
pub enum Filter {
    Auto,
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos,
}

impl Filter {
    pub const ALL: [Filter; 5] = [Filter::Auto, Filter::Lanczos, Filter::Bicubic, Filter::Bilinear, Filter::Nearest];
    pub fn menu_label(self) -> &'static str {
        match self {
            Filter::Auto => "Auto (Lanczos)",
            Filter::Lanczos => "Lanczos 3 (le plus net)",
            Filter::Bicubic => "Bicubique (netteté réglable)",
            Filter::Bilinear => "Bilinéaire (doux)",
            Filter::Nearest => "Plus proche voisin (pixels visibles)",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Filter::Nearest => "plus proche",
            Filter::Bilinear => "bilinéaire",
            Filter::Bicubic => "bicubique",
            Filter::Auto | Filter::Lanczos => "Lanczos",
        }
    }
}

/// Matériel qui fait l'agrandissement et l'affichage.
#[derive(Clone, Copy, PartialEq)]
pub enum Compute {
    /// Carte graphique (DirectX 12 / Vulkan).
    Gpu,
    /// Processeur : rendu logiciel (WARP sous Windows). Plus lent, mais sans carte graphique compatible.
    Cpu,
}

impl Compute {
    pub const ALL: [Compute; 2] = [Compute::Gpu, Compute::Cpu];
    pub fn label(self) -> &'static str {
        match self {
            Compute::Gpu => "Carte graphique",
            Compute::Cpu => "Processeur",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Present {
    /// Mailbox si disponible (sans déchirement, latence minimale), sinon Fifo.
    Auto,
    /// Synchronisé avec l'écran, file d'attente classique.
    Vsync,
    /// Affiche tout de suite, sans synchronisation (déchirement possible, latence minimale).
    Immediate,
}

impl Present {
    pub const ALL: [Present; 3] = [Present::Auto, Present::Vsync, Present::Immediate];
    pub fn menu_label(self) -> &'static str {
        match self {
            Present::Auto => "Auto (sans déchirement, latence minimale)",
            Present::Vsync => "Vsync (synchronisé avec l'écran)",
            Present::Immediate => "Immédiat (latence minimale, déchirement possible)",
        }
    }
}

fn choose_present(modes: &[wgpu::PresentMode], present: Present) -> (wgpu::PresentMode, &'static str) {
    let has = |m: wgpu::PresentMode| modes.contains(&m);
    match present {
        Present::Auto if has(wgpu::PresentMode::Mailbox) => (wgpu::PresentMode::Mailbox, "Mailbox"),
        Present::Immediate if has(wgpu::PresentMode::Immediate) => (wgpu::PresentMode::Immediate, "Immediate"),
        Present::Immediate if has(wgpu::PresentMode::AutoNoVsync) => (wgpu::PresentMode::AutoNoVsync, "Immediate"),
        _ => (wgpu::PresentMode::Fifo, "Fifo (vsync)"),
    }
}


// ---------------------------------------------------------------------------
// Poids du filtre (calculés sur le processeur, lus par le shader dans une petite texture)
// ---------------------------------------------------------------------------

fn lanczos3(x: f32) -> f32 {
    let ax = x.abs();
    if ax >= 3.0 {
        return 0.0;
    }
    let sinc = |t: f32| {
        if t.abs() < 1e-6 {
            1.0
        } else {
            let p = std::f32::consts::PI * t;
            p.sin() / p
        }
    };
    sinc(ax) * sinc(ax / 3.0)
}

fn cubic(x: f32, a: f32) -> f32 {
    let ax = x.abs();
    if ax <= 1.0 {
        (a + 2.0) * ax * ax * ax - (a + 3.0) * ax * ax + 1.0
    } else if ax < 2.0 {
        a * ax * ax * ax - 5.0 * a * ax * ax + 8.0 * a * ax - 4.0 * a
    } else {
        0.0
    }
}

struct Lut {
    taps: u32,
    rows: u32,
    width: u32,
    data: Vec<f32>,
}

/// Table de poids pour passer de `src_len` à `dst_len` pixels le long d'un axe.
/// Texel (x, r) : poids des points 4r..4r+3 du pixel de sortie x ; dernière rangée : indice du premier point.
/// Si l'image est réduite, le noyau est élargi pour garder une réduction propre.
fn build_lut(src_len: u32, dst_len: u32, filter: Filter, sharp: f32) -> Lut {
    let scale = dst_len as f32 / src_len as f32;
    let s = scale.min(1.0);
    let support = if filter == Filter::Bicubic { 2.0 } else { 3.0 };
    let half = ((support / s).ceil() as i32).clamp(1, 12);
    let taps = (2 * half) as u32;
    let rows = taps.div_ceil(4) + 1;
    let a = -0.5 - 0.5 * sharp.clamp(0.0, 1.0);
    let mut data = vec![0f32; (dst_len * rows * 4) as usize];
    let mut w = vec![0f32; taps as usize];
    for d in 0..dst_len {
        let c = (d as f32 + 0.5) / scale - 0.5;
        let start = c.floor() as i32 - half + 1;
        let mut sum = 0.0;
        for (k, wk) in w.iter_mut().enumerate() {
            let x = (c - (start + k as i32) as f32) * s;
            *wk = if filter == Filter::Bicubic { cubic(x, a) } else { lanczos3(x) };
            sum += *wk;
        }
        for (k, wk) in w.iter().enumerate() {
            let (row, comp) = (k as u32 / 4, k as u32 % 4);
            data[(((row * dst_len + d) * 4) + comp) as usize] = wk / sum;
        }
        data[((((rows - 1) * dst_len) + d) * 4) as usize] = start as f32;
    }
    Lut { taps, rows, width: dst_len, data }
}

// ---------------------------------------------------------------------------
// Moteur de rendu (indépendant de la fenêtre : testable sans écran)
// ---------------------------------------------------------------------------

struct SrcTexture {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    w: u32,
    h: u32,
}

/// Image YUYV brute envoyée telle quelle à la carte graphique (deux pixels par texel).
struct YuvTexture {
    tex: wgpu::Texture,
    #[allow(dead_code)]
    view: wgpu::TextureView,
    w: u32,
    h: u32,
}

/// Ressources dépendant de la taille de la source, de la zone d'affichage et du filtre.
struct Prepared {
    key: (u32, u32, u32, u32, u32, u32), // source w/h, zone w/h, filtre, netteté
    ox: u32,
    oy: u32,
    tw: u32,
    th: u32,
    direct: bool,
    bind_direct: wgpu::BindGroup,
    // Passes séparables
    inter_view: Option<wgpu::TextureView>,
    bind_h: Option<wgpu::BindGroup>,
    bind_v: Option<wgpu::BindGroup>,
    // Ces textures doivent rester en vie tant que les bind groups existent.
    _keep: Vec<wgpu::Texture>,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    p_direct: wgpu::RenderPipeline,
    p_h: wgpu::RenderPipeline,
    p_v: wgpu::RenderPipeline,
    p_yuyv: wgpu::RenderPipeline,
    ubuf: [wgpu::Buffer; 4],
    yuv: Option<YuvTexture>,
    yuv_bind: Option<wgpu::BindGroup>,
    /// Une image YUYV attend sa conversion en RGB (faite au prochain `draw`).
    yuv_pending: bool,
    dummy_lut: wgpu::TextureView,
    _dummy_tex: wgpu::Texture,
    src: Option<SrcTexture>,
    prepared: Option<Prepared>,
    filter: Filter,
    sharp: f32,
    /// Largeur (pixels) réservée à droite de la fenêtre pour le panneau : l'image est placée dans le reste.
    inset_right: u32,
}

/// Ouvre une passe de rendu qui efface la cible en noir.
fn begin<'e>(enc: &'e mut wgpu::CommandEncoder, view: &wgpu::TextureView) -> wgpu::RenderPass<'e> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    })
}

const INTER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

fn params_bytes(p: [f32; 12]) -> [u8; 48] {
    let mut b = [0u8; 48];
    for (i, v) in p.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
    }
    b
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat, filter: Filter, sharp: f32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("agrandissement"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let entry = |binding: u32, ty: wgpu::BindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
            entries: &[
                entry(0, wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                }),
                entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(2, wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                }),
                entry(3, wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                }),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let make = |label: &str, fs: &str, target: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: target, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let p_direct = make("direct", "fs_direct", format);
        let p_h = make("horizontal", "fs_h", INTER_FORMAT);
        let p_v = make("vertical", "fs_v", format);
        let p_yuyv = make("yuyv", "fs_yuyv", wgpu::TextureFormat::Rgba8Unorm);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mk_ubuf = |label: &str| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 48,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let ubuf = [mk_ubuf("p0"), mk_ubuf("p1"), mk_ubuf("p2"), mk_ubuf("p3")];
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("poids vides"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_lut = dummy.create_view(&Default::default());
        Self {
            device, queue, bgl, sampler, p_direct, p_h, p_v, p_yuyv, ubuf, dummy_lut, _dummy_tex: dummy,
            yuv: None, yuv_bind: None, yuv_pending: false,
            src: None, prepared: None, filter, sharp, inset_right: 0,
        }
    }

    pub fn set_filter(&mut self, f: Filter) {
        self.filter = f;
    }

    pub fn set_sharpen(&mut self, v: f32) {
        self.sharp = v;
    }

    pub fn has_frame(&self) -> bool {
        self.src.is_some()
    }

    /// Réserve `px` pixels à droite de la fenêtre (panneau) ; l'image est réduite pour tenir dans le reste.
    pub fn set_inset_right(&mut self, px: u32) {
        self.inset_right = px;
    }

    /// Envoie une image RGBA (4 octets par pixel, `w * h * 4` octets) vers la carte graphique.
    /// Prépare la texture RGBA de la source (recréée si la taille change).
    fn ensure_src(&mut self, w: u32, h: u32) {
        if self.src.as_ref().map_or(true, |s| s.w != w || s.h != h) {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("capture"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                // RENDER_ATTACHMENT : la conversion YUYV -> RGB écrit directement dans cette texture.
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.src = Some(SrcTexture { tex, view, w, h });
            self.prepared = None;
        }
    }

    /// Envoie une image YUYV 4:2:2 brute (`w * h * 2` octets, `w` pair) : la carte graphique la convertit en RGB
    /// (couleurs interpolées, BT.709 à partir de 720 lignes) au prochain `draw`. Le processeur ne fait que copier.
    pub fn upload_yuyv(&mut self, w: u32, h: u32, raw: &[u8]) {
        debug_assert!(w % 2 == 0 && raw.len() == (w * h * 2) as usize);
        self.ensure_src(w, h);
        if self.yuv.as_ref().map_or(true, |t| t.w != w || t.h != h) {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("yuyv"),
                size: wgpu::Extent3d { width: w / 2, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let matrix = if h >= 720 { 0.0 } else { 1.0 };
            self.queue.write_buffer(
                &self.ubuf[3],
                0,
                &params_bytes([w as f32, h as f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, matrix, 0.0, 0.0, 0.0]),
            );
            self.yuv_bind = Some(self.bind(&view, &self.ubuf[3], &self.dummy_lut));
            self.yuv = Some(YuvTexture { tex, view, w, h });
        }
        let t = self.yuv.as_ref().unwrap();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &t.tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            raw,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 2), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w / 2, height: h, depth_or_array_layers: 1 },
        );
        self.yuv_pending = true;
    }

    pub fn upload(&mut self, w: u32, h: u32, rgba: &[u8]) {
        self.ensure_src(w, h);
        self.yuv_pending = false;
        let s = self.src.as_ref().unwrap();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &s.tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    fn make_lut_texture(&self, lut: &Lut) -> (wgpu::Texture, wgpu::TextureView) {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("poids"),
            size: wgpu::Extent3d { width: lut.width, height: lut.rows, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes: Vec<u8> = lut.data.iter().flat_map(|v| v.to_ne_bytes()).collect();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lut.width * 16), rows_per_image: Some(lut.rows) },
            wgpu::Extent3d { width: lut.width, height: lut.rows, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&Default::default());
        (tex, view)
    }

    fn bind(&self, input: &wgpu::TextureView, ubuf: &wgpu::Buffer, lut: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(lut) },
            ],
        })
    }

    /// Prépare (ou réutilise) tout ce qui dépend de la taille de la fenêtre, de la source et du filtre.
    /// Les poids ne sont recalculés que lorsque l'un de ces éléments change, jamais à chaque image.
    fn prepare(&mut self, cw: u32, ch: u32) {
        let Some(src) = self.src.as_ref() else { return };
        let (sw, sh) = (src.w, src.h);
        // L'image se place dans la zone libre : la fenêtre moins le panneau de droite.
        let sw_zone = cw.saturating_sub(self.inset_right).max(1);
        let k = (sw_zone as f32 / sw as f32).min(ch as f32 / sh as f32);
        let tw = ((sw as f32 * k).round() as u32).clamp(1, sw_zone);
        let th = ((sh as f32 * k).round() as u32).clamp(1, ch);
        let (ox, oy) = ((sw_zone - tw) / 2, (ch - th) / 2);
        let filter_id = match self.filter {
            Filter::Nearest => 0,
            Filter::Bilinear => 1,
            Filter::Bicubic => 2,
            Filter::Auto | Filter::Lanczos => 3,
        };
        let key = (sw, sh, tw, th, filter_id, self.sharp.to_bits());
        if self.prepared.as_ref().is_some_and(|p| p.key == key && (p.ox, p.oy) == (ox, oy)) {
            return;
        }
        // Taille identique : copie exacte, sans filtre.
        let exact = tw == sw && th == sh;
        let direct = exact || filter_id <= 1;
        let src_view = &src.view;

        if direct {
            let mode = if exact || filter_id == 0 { 0.0 } else { 1.0 };
            self.queue.write_buffer(
                &self.ubuf[0],
                0,
                &params_bytes([sw as f32, sh as f32, 0.0, 0.0, ox as f32, oy as f32, tw as f32, th as f32, mode, 0.0, 0.0, 0.0]),
            );
            let bind_direct = self.bind(src_view, &self.ubuf[0], &self.dummy_lut);
            self.prepared = Some(Prepared {
                key, ox, oy, tw, th, direct: true, bind_direct,
                inter_view: None, bind_h: None, bind_v: None, _keep: Vec::new(),
            });
            return;
        }

        let lh = build_lut(sw, tw, self.filter, self.sharp);
        let lv = build_lut(sh, th, self.filter, self.sharp);
        let (th_tex, th_view) = self.make_lut_texture(&lh);
        let (tv_tex, tv_view) = self.make_lut_texture(&lv);
        let inter = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("intermédiaire"),
            size: wgpu::Extent3d { width: tw, height: sh, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: INTER_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let inter_view = inter.create_view(&Default::default());
        self.queue.write_buffer(
            &self.ubuf[1],
            0,
            &params_bytes([sw as f32, sh as f32, 0.0, 0.0, 0.0, 0.0, tw as f32, sh as f32, 0.0, lh.taps as f32, lh.rows as f32, 0.0]),
        );
        self.queue.write_buffer(
            &self.ubuf[2],
            0,
            &params_bytes([tw as f32, sh as f32, 0.0, 0.0, ox as f32, oy as f32, tw as f32, th as f32, 0.0, lv.taps as f32, lv.rows as f32, 0.0]),
        );
        let bind_direct = self.bind(src_view, &self.ubuf[0], &self.dummy_lut);
        let bind_h = self.bind(src_view, &self.ubuf[1], &th_view);
        let bind_v = self.bind(&inter_view, &self.ubuf[2], &tv_view);
        self.prepared = Some(Prepared {
            key, ox, oy, tw, th, direct: false, bind_direct,
            inter_view: Some(inter_view), bind_h: Some(bind_h), bind_v: Some(bind_v),
            _keep: vec![th_tex, tv_tex, inter],
        });
    }

    /// Dessine l'image courante (proportions conservées, bandes noires) sur `target`.
    pub fn draw(&mut self, enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, size: (u32, u32)) {
        // Image YUYV reçue : conversion en RGB dans la texture de la source (une fois par image).
        if self.yuv_pending {
            if let (Some(src), Some(bind)) = (self.src.as_ref(), self.yuv_bind.as_ref()) {
                let mut pass = begin(enc, &src.view);
                pass.set_pipeline(&self.p_yuyv);
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
            }
            self.yuv_pending = false;
        }
        self.prepare(size.0.max(1), size.1.max(1));
        let Some(p) = self.prepared.as_ref() else {
            let _ = begin(enc, target); // efface en noir (aucune image reçue)
            return;
        };
        if p.direct {
            let mut pass = begin(enc, target);
            pass.set_pipeline(&self.p_direct);
            pass.set_bind_group(0, &p.bind_direct, &[]);
            pass.draw(0..3, 0..1);
            return;
        }
        // Passe 1 : horizontale, vers la texture intermédiaire.
        {
            let mut pass = begin(enc, p.inter_view.as_ref().unwrap());
            pass.set_pipeline(&self.p_h);
            pass.set_bind_group(0, p.bind_h.as_ref().unwrap(), &[]);
            pass.draw(0..3, 0..1);
        }
        // Passe 2 : verticale, vers la fenêtre, limitée à la zone d'affichage (le reste reste noir).
        {
            let mut pass = begin(enc, target);
            pass.set_viewport(p.ox as f32, p.oy as f32, p.tw as f32, p.th as f32, 0.0, 1.0);
            pass.set_pipeline(&self.p_v);
            pass.set_bind_group(0, p.bind_v.as_ref().unwrap(), &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

// ---------------------------------------------------------------------------
// Fenêtre : surface d'affichage + moteur de rendu
// ---------------------------------------------------------------------------

pub struct Gfx {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    present_modes: Vec<wgpu::PresentMode>,
    renderer: Renderer,
    pub adapter_name: String,
    pub present_label: &'static str,
    filter: Filter,
}

impl Gfx {
    pub async fn new(
        window: Arc<Window>,
        compute: Compute,
        present: Present,
        filter: Filter,
        sharp: f32,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).map_err(|e| format!("surface : {e}"))?;
        let cpu = compute == Compute::Cpu;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                // Le processeur passe par l'adaptateur logiciel (WARP sous Windows).
                force_fallback_adapter: cpu,
            })
            .await
            .ok_or(if cpu {
                "aucun rendu logiciel (processeur) disponible"
            } else {
                "aucune carte graphique compatible trouvée"
            })?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("lecteur-hd60"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
            .map_err(|e| format!("périphérique graphique : {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        // Format non-sRGB : les valeurs de l'image sont affichées telles quelles, sans conversion.
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let (mode, present_label) = choose_present(&caps.present_modes, present);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: mode,
            desired_maximum_frame_latency: 1,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let renderer = Renderer::new(device, queue, format, filter, sharp);
        Ok(Self { surface, config, present_modes: caps.present_modes.clone(), renderer, adapter_name, present_label, filter })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.renderer.device, &self.config);
    }

    pub fn filter_label(&self) -> &'static str {
        self.filter.label()
    }

    pub fn set_filter(&mut self, f: Filter) {
        self.filter = f;
        self.renderer.set_filter(f);
    }

    pub fn set_sharpen(&mut self, v: f32) {
        self.renderer.set_sharpen(v);
    }

    /// Change le mode de présentation à chaud (reconfigure la surface).
    pub fn set_present(&mut self, p: Present) {
        let (mode, label) = choose_present(&self.present_modes, p);
        if mode != self.config.present_mode {
            self.config.present_mode = mode;
            self.surface.configure(&self.renderer.device, &self.config);
        }
        self.present_label = label;
    }

    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    pub fn has_frame(&self) -> bool {
        self.renderer.has_frame()
    }

    pub fn set_inset_right(&mut self, px: u32) {
        self.renderer.set_inset_right(px);
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.renderer.device
    }

    pub fn upload(&mut self, w: u32, h: u32, rgba: &[u8]) {
        self.renderer.upload(w, h, rgba);
    }

    pub fn upload_yuyv(&mut self, w: u32, h: u32, raw: &[u8]) {
        self.renderer.upload_yuyv(w, h, raw);
    }

    /// Dessine l'image courante puis appelle `overlay` pour dessiner par-dessus (menu).
    /// `overlay` renvoie des commandes à envoyer avant les nôtres.
    pub fn render_with<F>(&mut self, overlay: F)
    where
        F: FnOnce(&wgpu::Device, &wgpu::Queue, &mut wgpu::CommandEncoder, &wgpu::TextureView, [u32; 2]) -> Vec<wgpu::CommandBuffer>,
    {
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Timeout) => {
                self.surface.configure(&self.renderer.device, &self.config);
                return;
            }
            Err(_) => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = self.renderer.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("enc") });
        self.renderer.draw(&mut enc, &view, (self.config.width, self.config.height));
        let mut cmds = overlay(
            &self.renderer.device,
            &self.renderer.queue,
            &mut enc,
            &view,
            [self.config.width, self.config.height],
        );
        cmds.push(enc.finish());
        self.renderer.queue.submit(cmds);
        frame.present();
    }
}
