// Un seul triangle plein écran. Deux façons de calculer chaque pixel :
//  - fs_direct : plus proche voisin ou bilinéaire (une seule passe, très léger) ;
//  - fs_h puis fs_v : agrandissement/réduction séparable (Lanczos, bicubique). Les poids sont
//    précalculés sur le processeur et lus dans une petite texture (lut) : aucun calcul de sinus ici.

struct Params {
    src: vec2<f32>,   // taille de la texture d'entrée de cette passe (pixels)
    pad0: vec2<f32>,
    rect: vec4<f32>,  // zone d'affichage dans la fenêtre : x, y, largeur, hauteur (pixels)
    mode: f32,        // fs_direct : 0 plus proche, 1 bilinéaire
    taps: f32,        // nombre de points d'échantillonnage par pixel (passes séparables)
    rows: f32,        // hauteur de la texture de poids (lut)
    pad1: f32,
};

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> p: Params;
@group(0) @binding(3) var lut: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(pos[i], 0.0, 1.0);
}

@fragment
fn fs_direct(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let q = frag.xy - p.rect.xy;
    if (q.x < 0.0 || q.y < 0.0 || q.x >= p.rect.z || q.y >= p.rect.w) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);   // bandes noires
    }
    let uv = q / p.rect.zw;
    if (p.mode < 0.5) {
        let maxc = vec2<i32>(p.src) - vec2<i32>(1, 1);
        let c = clamp(vec2<i32>(floor(uv * p.src)), vec2<i32>(0, 0), maxc);
        return vec4<f32>(textureLoad(tex, c, 0).rgb, 1.0);
    }
    return vec4<f32>(textureSampleLevel(tex, samp, uv, 0.0).rgb, 1.0);
}

// Passe horizontale : image source -> texture intermédiaire (largeur d'affichage, hauteur source).
@fragment
fn fs_h(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let x = i32(frag.x);
    let y = i32(frag.y);
    let rows = i32(p.rows);
    let taps = i32(p.taps);
    let start = i32(textureLoad(lut, vec2<i32>(x, rows - 1), 0).x);
    let maxx = i32(p.src.x) - 1;
    var acc = vec3<f32>(0.0);
    for (var k = 0; k < taps; k++) {
        let w = textureLoad(lut, vec2<i32>(x, k / 4), 0)[k % 4];
        let sx = clamp(start + k, 0, maxx);
        acc += w * textureLoad(tex, vec2<i32>(sx, y), 0).rgb;
    }
    return vec4<f32>(acc, 1.0);
}

// Passe verticale : texture intermédiaire -> fenêtre (dans la zone d'affichage, via le viewport).
@fragment
fn fs_v(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let x = i32(frag.x - p.rect.x);
    let y = i32(frag.y - p.rect.y);
    let rows = i32(p.rows);
    let taps = i32(p.taps);
    let start = i32(textureLoad(lut, vec2<i32>(y, rows - 1), 0).x);
    let maxy = i32(p.src.y) - 1;
    var acc = vec3<f32>(0.0);
    for (var k = 0; k < taps; k++) {
        let w = textureLoad(lut, vec2<i32>(y, k / 4), 0)[k % 4];
        let sy = clamp(start + k, 0, maxy);
        acc += w * textureLoad(tex, vec2<i32>(x, sy), 0).rgb;
    }
    return vec4<f32>(clamp(acc, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

// Conversion YUYV (4:2:2) -> RGB, faite par la carte graphique. La texture d'entrée est large de la moitié de
// l'image : chaque texel contient Y0, U, Y1, V (deux pixels qui partagent leurs couleurs).
// Les couleurs sont placées sur les pixels pairs : les pixels impairs en prennent la moyenne avec le voisin de droite,
// au lieu de répéter la même valeur (ce qui donnait des bords de couleur en blocs de deux pixels).
// Plage limitée (16-235). p.mode : 0 = BT.709 (HD), 1 = BT.601 (définitions inférieures à 720 lignes).
@fragment
fn fs_yuyv(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let x = i32(frag.x);
    let y = i32(frag.y);
    let k = x >> 1;
    let maxk = i32(p.src.x) / 2 - 1;
    let t0 = textureLoad(tex, vec2<i32>(k, y), 0);
    var luma: f32;
    var cb: f32;
    var cr: f32;
    if ((x & 1) == 0) {
        luma = t0.r;
        cb = t0.g;
        cr = t0.a;
    } else {
        let t1 = textureLoad(tex, vec2<i32>(min(k + 1, maxk), y), 0);
        luma = t0.b;
        cb = 0.5 * (t0.g + t1.g);
        cr = 0.5 * (t0.a + t1.a);
    }
    let yl = (luma - 16.0 / 255.0) * (255.0 / 219.0);
    let u = (cb - 128.0 / 255.0) * (255.0 / 224.0);
    let v = (cr - 128.0 / 255.0) * (255.0 / 224.0);
    var rgb: vec3<f32>;
    if (p.mode < 0.5) {
        rgb = vec3<f32>(yl + 1.5748 * v, yl - 0.1873 * u - 0.4681 * v, yl + 1.8556 * u);
    } else {
        rgb = vec3<f32>(yl + 1.402 * v, yl - 0.344136 * u - 0.714136 * v, yl + 1.772 * u);
    }
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
