//! Teste le moteur de rendu sans fenêtre : rendu dans une texture, relecture des pixels.
//! Lancer avec : cargo test --release -- --nocapture

use crate::gpu::{Filter, Renderer};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Ctx {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Ctx> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()?;
    Some(Ctx { device, queue })
}

/// Rend `src` (sw x sh, RGBA) dans une cible ow x oh avec le filtre donné ; renvoie les pixels RGBA.
fn render(c: &Ctx, filter: Filter, sw: u32, sh: u32, src: &[u8], ow: u32, oh: u32) -> Vec<u8> {
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), FORMAT, filter, 0.35);
    r.upload(sw, sh, src);
    let target = c.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: ow, height: oh, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let row = (ow * 4).div_ceil(256) * 256;
    let out = c.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * oh) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = c.device.create_command_encoder(&Default::default());
    r.draw(&mut enc, &view, (ow, oh));
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(oh) } },
        wgpu::Extent3d { width: ow, height: oh, depth_or_array_layers: 1 },
    );
    c.queue.submit(Some(enc.finish()));
    let slice = out.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    c.device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range();
    let mut res = Vec::with_capacity((ow * oh * 4) as usize);
    for y in 0..oh {
        res.extend_from_slice(&data[(y * row) as usize..(y * row + ow * 4) as usize]);
    }
    res
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

fn close(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= tol)
}

const FILTERS: [(Filter, &str); 4] =
    [(Filter::Nearest, "nearest"), (Filter::Bilinear, "bilinear"), (Filter::Bicubic, "bicubic"), (Filter::Lanczos, "lanczos")];

#[test]
fn rendu_tous_les_filtres() {
    let Some(c) = gpu() else {
        eprintln!("(pas de GPU/pilote Vulkan disponible : test ignoré)");
        return;
    };
    let solid: Vec<u8> = (0..16).flat_map(|_| [200u8, 100, 50, 255]).collect(); // 4x4 unie

    for (f, name) in FILTERS {
        // Image unie 4x4 -> 16x16 : couleur conservée partout.
        let o = render(&c, f, 4, 4, &solid, 16, 16);
        for y in 0..16 {
            for x in 0..16 {
                assert!(close(px(&o, 16, x, y), [200, 100, 50], 1), "{name} agrandi ({x},{y}) = {:?}", px(&o, 16, x, y));
            }
        }
        // Bandes noires : image 4x4 dans une cible 16x8 -> zone 8x8 centrée, 4 colonnes noires de chaque côté.
        let o = render(&c, f, 4, 4, &solid, 16, 8);
        assert_eq!(px(&o, 16, 1, 4), [0, 0, 0], "{name} bande noire gauche");
        assert_eq!(px(&o, 16, 14, 4), [0, 0, 0], "{name} bande noire droite");
        assert!(close(px(&o, 16, 8, 4), [200, 100, 50], 1), "{name} centre");
        // Réduction 16x16 -> 4x4 : couleur conservée.
        let big: Vec<u8> = (0..256).flat_map(|_| [10u8, 200, 90, 255]).collect();
        let o = render(&c, f, 16, 16, &big, 4, 4);
        for y in 0..4 {
            for x in 0..4 {
                assert!(close(px(&o, 4, x, y), [10, 200, 90], 1), "{name} réduit ({x},{y}) = {:?}", px(&o, 4, x, y));
            }
        }
    }

    // Plus proche voisin : chaque pixel source devient un bloc 4x4 identique.
    let mut pat = Vec::new();
    for i in 0..16u8 {
        pat.extend_from_slice(&[i * 16, 255 - i * 16, i * 8, 255]);
    }
    let o = render(&c, Filter::Nearest, 4, 4, &pat, 16, 16);
    for y in 0..16u32 {
        for x in 0..16u32 {
            let i = ((y / 4) * 4 + x / 4) as usize * 4;
            assert_eq!(px(&o, 16, x, y), [pat[i], pat[i + 1], pat[i + 2]], "plus proche ({x},{y})");
        }
    }

    // Dégradé horizontal : croissant avec tous les filtres, et l'image n'est pas retournée.
    let mut ramp = Vec::new();
    for _y in 0..8 {
        for x in 0..8u32 {
            let v = (x * 255 / 7) as u8;
            ramp.extend_from_slice(&[v, v, v, 255]);
        }
    }
    for (f, name) in FILTERS.iter().skip(1) {
        let o = render(&c, *f, 8, 8, &ramp, 32, 32);
        let mut prev = -1i32;
        for x in 6..26 {
            let v = px(&o, 32, x, 16)[0] as i32;
            assert!(v >= prev - 1, "{name} dégradé non croissant en x={x} ({v} < {prev})");
            prev = v;
        }
        assert!(px(&o, 32, 26, 16)[0] > px(&o, 32, 6, 16)[0] + 100, "{name} dégradé inversé ou plat");
    }

    // Dégradé vertical : le haut doit rester en haut (pas de retournement vertical).
    let mut vr = Vec::new();
    for y in 0..8u32 {
        for _x in 0..8 {
            let v = (y * 255 / 7) as u8;
            vr.extend_from_slice(&[v, v, v, 255]);
        }
    }
    for (f, name) in FILTERS {
        let o = render(&c, f, 8, 8, &vr, 32, 32);
        assert!(px(&o, 32, 16, 26)[0] > px(&o, 32, 16, 6)[0] + 100, "{name} vertical retourné ou plat");
    }
    eprintln!("rendu : tous les tests passent");
}

/// Le filtre Lanczos doit être net : un contour franc reste raide (plus qu'en bilinéaire).
#[test]
fn lanczos_plus_net_que_bilineaire() {
    let Some(c) = gpu() else {
        eprintln!("(pas de GPU/pilote Vulkan disponible : test ignoré)");
        return;
    };
    let mut edge = Vec::new();
    for _y in 0..8 {
        for x in 0..8u32 {
            let v = if x < 4 { 20u8 } else { 230 };
            edge.extend_from_slice(&[v, v, v, 255]);
        }
    }
    // Pente maximale du contour (écart le plus fort entre deux pixels voisins) : plus elle est forte, plus c'est net.
    let slope = |f: Filter| {
        let o = render(&c, f, 8, 8, &edge, 128, 128);
        (1..128).map(|x| (px(&o, 128, x, 64)[0] as i32 - px(&o, 128, x - 1, 64)[0] as i32).abs()).max().unwrap()
    };
    let (sl, sb) = (slope(Filter::Lanczos), slope(Filter::Bilinear));
    eprintln!("pente maximale du contour : lanczos {sl}, bilinéaire {sb}");
    assert!(sl > sb, "Lanczos doit avoir un contour plus raide que le bilinéaire ({sl} contre {sb})");
}

// ---------------------------------------------------------------------------
// YUYV -> RGB sur la carte graphique
// ---------------------------------------------------------------------------

/// Référence processeur : couleurs 4:2:2 interpolées sur les pixels impairs, plage limitée, BT.709 ou BT.601.
fn yuyv_reference(w: usize, h: usize, raw: &[u8], bt709: bool) -> Vec<[u8; 3]> {
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let k = x / 2;
            let at = |k: usize| &raw[(y * w + k * 2) * 2..(y * w + k * 2) * 2 + 4]; // Y0 U Y1 V
            let t0 = at(k);
            let (luma, cb, cr) = if x % 2 == 0 {
                (t0[0] as f32, t0[1] as f32, t0[3] as f32)
            } else {
                let t1 = at((k + 1).min(w / 2 - 1));
                (t0[2] as f32, (t0[1] as f32 + t1[1] as f32) / 2.0, (t0[3] as f32 + t1[3] as f32) / 2.0)
            };
            let yl = (luma - 16.0) * (255.0 / 219.0) / 255.0;
            let u = (cb - 128.0) * (255.0 / 224.0) / 255.0;
            let v = (cr - 128.0) * (255.0 / 224.0) / 255.0;
            let (r, g, b) = if bt709 {
                (yl + 1.5748 * v, yl - 0.1873 * u - 0.4681 * v, yl + 1.8556 * u)
            } else {
                (yl + 1.402 * v, yl - 0.344136 * u - 0.714136 * v, yl + 1.772 * u)
            };
            let q = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            out.push([q(r), q(g), q(b)]);
        }
    }
    out
}

fn yuyv_render(c: &Ctx, w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
    let mut r = Renderer::new(c.device.clone(), c.queue.clone(), FORMAT, Filter::Nearest, 0.35);
    r.upload_yuyv(w, h, raw);
    let target = c.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let row = (w * 4).div_ceil(256) * 256;
    let out = c.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = c.device.create_command_encoder(&Default::default());
    r.draw(&mut enc, &view, (w, h)); // taille identique : copie exacte de l'image convertie
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &out, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    c.queue.submit(Some(enc.finish()));
    let slice = out.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    c.device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range();
    let mut res = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        res.extend_from_slice(&data[(y * row) as usize..(y * row + w * 4) as usize]);
    }
    res
}

#[test]
fn yuyv_converti_par_la_carte_graphique() {
    let Some(c) = gpu() else {
        eprintln!("(pas de GPU/pilote Vulkan disponible : test ignoré)");
        return;
    };
    // Deux tailles : 8x720 (BT.709) et 8x480 (BT.601), avec des valeurs variées pour exercer l'interpolation.
    for (w, h, bt709) in [(8usize, 720usize, true), (8, 480, false)] {
        let mut raw = vec![0u8; w * h * 2];
        for (i, b) in raw.iter_mut().enumerate() {
            *b = match i % 4 {
                1 | 3 => (40 + (i * 37) % 180) as u8,         // U, V
                _ => (16 + (i * 53) % 219) as u8,             // Y (plage limitée)
            };
        }
        let got = yuyv_render(&c, w as u32, h as u32, &raw);
        let want = yuyv_reference(w, h, &raw, bt709);
        let mut worst = 0;
        for y in 0..h {
            for x in 0..w {
                let g = px(&got, w as u32, x as u32, y as u32);
                let e = want[y * w + x];
                for k in 0..3 {
                    worst = worst.max((g[k] as i32 - e[k] as i32).abs());
                }
            }
        }
        eprintln!("yuyv {w}x{h} (bt709={bt709}) : écart maximal avec la référence = {worst}/255");
        assert!(worst <= 2, "conversion YUYV incorrecte (écart {worst})");
    }
}

#[test]
fn yuyv_couleurs_interpolees_sur_les_bords() {
    let Some(c) = gpu() else {
        eprintln!("(pas de GPU/pilote Vulkan disponible : test ignoré)");
        return;
    };
    // 4 pixels : jaune (Y=210 U=16 V=146) puis gris (Y=126 U=128 V=128). Pixel impair du jaune : couleur intermédiaire.
    let (w, h) = (4usize, 720usize);
    let mut raw = vec![0u8; w * h * 2];
    for y in 0..h {
        let r = y * w * 2;
        raw[r..r + 8].copy_from_slice(&[210, 16, 210, 146, 126, 128, 126, 128]);
    }
    let got = yuyv_render(&c, w as u32, h as u32, &raw);
    let p1 = px(&got, w as u32, 1, 0); // pixel impair du couple jaune
    let p0 = px(&got, w as u32, 0, 0);
    // Sans interpolation (duplication des couleurs), p0 et p1 auraient le même bleu : ici p1 est moins jaune.
    eprintln!("pixel 0 = {p0:?}, pixel 1 = {p1:?}");
    assert!(p1[2] > p0[2] + 8, "le pixel impair doit reprendre une partie de la couleur du voisin (p0 {p0:?}, p1 {p1:?})");
}
