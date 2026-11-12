//! Lecteur faible latence (image + son) pour carte de capture USB (UVC), ex. NZXT Signal HD60.
//!
//! - Un thread capture et décode les images ; seule la plus récente est gardée (pas de retard cumulé).
//! - L'image est envoyée telle quelle à la carte graphique, qui fait l'agrandissement (Lanczos) et l'affichage.
//! - Le son de la carte est renvoyé vers la sortie audio par défaut avec un tampon d'environ 25 ms.
//! - Un menu (Tab ou F1) permet de changer les réglages sans relancer le programme.

mod audio;
mod gpu;
mod settings;
mod sys;
mod ui;
#[cfg(test)]
mod shader_test;

use gpu::{Compute, Gfx};
use nokhwa::{
    pixel_format::{RgbAFormat, RgbFormat},
    query,
    utils::{
        ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
        Resolution,
    },
    Camera,
};
use settings::{Format, Settings};
use std::{
    env, mem, process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use ui::{Action, Info, Ui};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Fullscreen, Window, WindowId},
};

// ---------------------------------------------------------------------------
// Données partagées entre le thread de capture et la fenêtre
// ---------------------------------------------------------------------------

/// Réveille la boucle de la fenêtre (nouvelle image ou changement d'état).
enum UserEvent {
    Wake,
}

/// Image capturée : soit RGBA (4 octets par pixel), soit YUYV brut (2 octets par pixel, converti par la carte graphique).
struct Frame {
    w: usize,
    h: usize,
    yuyv: bool,
    px: Vec<u32>,
}

struct Shared {
    frame: Mutex<Option<Frame>>,
    pending: AtomicBool,
    pool: Mutex<Vec<Vec<u32>>>,
    fps: Mutex<f32>,
    /// Description du flux en cours, ex. « 1920x1080 YUYV ».
    stream: Mutex<String>,
    /// Message d'état (« Ouverture de la carte… »).
    status: Mutex<String>,
    /// Dernière erreur d'ouverture de la carte.
    error: Mutex<Option<String>>,
}

#[derive(Clone)]
struct CaptureParams {
    /// Index forcé par la ligne de commande (première ouverture seulement).
    index: Option<u32>,
    name: Option<String>,
    w: u32,
    h: u32,
    fps: u32,
    format: Option<FrameFormat>,
}

struct Capture {
    running: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

// ---------------------------------------------------------------------------
// Ligne de commande (les options modifient les réglages chargés)
// ---------------------------------------------------------------------------

struct Cli {
    list: bool,
    /// Saute l'écran d'accueil et lance le direct tout de suite.
    direct: bool,
    device: Option<u32>,
    audio: Option<usize>,
}

const AIDE: &str = "\
Usage : lecteur-hd60 [options]

Tout se règle aussi depuis le menu (Tab ou F1). Les options ci-dessous remplacent, pour cette
session, les réglages enregistrés.

  --list, -l          liste les périphériques vidéo et audio
  --direct, -g        saute l'écran d'accueil et lance le direct tout de suite
  --device, -d N     index vidéo (auto : nom contenant HD60 / NZXT / Signal)
  --audio, -a N       index de l'entrée audio (auto par défaut)
  --no-audio          désactive le son
  --width, -w N       largeur demandée   (1920)
  --height, -h N      hauteur demandée   (1080)
  --fps, -f N         images/s demandées (60)
  --format auto|mjpeg|yuyv|nv12   format de capture (auto : formats bruts d'abord)
  --fullscreen, -p    démarre en plein écran
  --gpu | --cpu       matériel qui agrandit et affiche l'image : carte graphique (défaut) ou processeur
                      (rendu logiciel, plus lent : les filtres légers sont alors choisis automatiquement)
  --upscale auto|lanczos|bicubic|bilinear|nearest   agrandissement (auto = lanczos)
  --sharpen 0-100     netteté du filtre bicubique (35)
  --present auto|vsync|immediate
                      auto : sans déchirement et latence minimale (Mailbox) ;
                      vsync : synchronisé avec l'écran ; immediate : sans synchronisation
  --no-config         ignore le fichier de réglages enregistré

Touches : Entrée lance le direct (accueil) · Tab / F1 réglages · F / F11 / Alt+Entrée plein écran
          Échap ferme les réglages, quitte le plein écran, revient à l'accueil, puis quitte · H accueil
          Q quitter · M sourdine · Haut/Bas volume · I infos";

fn parse_args(s: &mut Settings) -> Cli {
    let mut c = Cli { list: false, direct: false, device: None, audio: None };
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        let mut next = |name: &str| -> String {
            args.next().unwrap_or_else(|| {
                eprintln!("Valeur manquante pour {name}");
                process::exit(2);
            })
        };
        let num = |v: String, name: &str| -> u32 {
            v.parse().unwrap_or_else(|_| {
                eprintln!("Valeur invalide pour {name}");
                process::exit(2);
            })
        };
        match a.as_str() {
            "--list" | "-l" => c.list = true,
            "--direct" | "-g" => c.direct = true,
            "--gpu" => s.compute = Compute::Gpu,
            "--cpu" => {
                let from = s.compute;
                s.compute = Compute::Cpu;
                s.adapt_compute(from);
            }
            "--device" | "-d" => c.device = Some(num(next("--device"), "--device")),
            "--audio" | "-a" => c.audio = Some(num(next("--audio"), "--audio") as usize),
            "--no-audio" => s.audio_on = false,
            "--width" | "-w" => s.width = num(next("--width"), "--width"),
            "--height" | "-h" => s.height = num(next("--height"), "--height"),
            "--fps" | "-f" => s.fps = num(next("--fps"), "--fps"),
            "--format" => {
                let v = next("--format").to_lowercase();
                s.format = settings::parse_format(&v).unwrap_or_else(|| {
                    eprintln!("Format inconnu : {v} (auto, mjpeg, yuyv ou nv12)");
                    process::exit(2);
                });
            }
            "--upscale" => {
                let v = next("--upscale").to_lowercase();
                s.filter = settings::parse_filter(&v).unwrap_or_else(|| {
                    eprintln!("--upscale : auto, lanczos, bicubic, bilinear ou nearest (reçu : {v})");
                    process::exit(2);
                });
            }
            "--sharpen" => s.sharpen = (num(next("--sharpen"), "--sharpen").min(100)) as f32 / 100.0,
            "--present" => {
                let v = next("--present").to_lowercase();
                s.present = settings::parse_present(&v).unwrap_or_else(|| {
                    eprintln!("--present : auto, vsync ou immediate (reçu : {v})");
                    process::exit(2);
                });
            }
            "--fullscreen" | "-p" => s.fullscreen = true,
            "--no-config" => {}
            "--help" => {
                println!("{AIDE}");
                process::exit(0);
            }
            autre => {
                eprintln!("Option inconnue : {autre}\n\n{AIDE}");
                process::exit(2);
            }
        }
    }
    c
}

// ---------------------------------------------------------------------------
// Capture vidéo
// ---------------------------------------------------------------------------

fn list_video() -> Vec<(u32, String)> {
    match query(ApiBackend::Auto) {
        Ok(devs) => devs
            .iter()
            .filter_map(|d| match d.index() {
                CameraIndex::Index(i) => Some((*i, d.human_name())),
                _ => None,
            })
            .collect(),
        Err(e) => {
            eprintln!("Impossible de lister les périphériques vidéo : {e}");
            Vec::new()
        }
    }
}

/// Liste des demandes à essayer, de la meilleure à la plus tolérante.
/// En mode auto, les formats bruts (YUYV, NV12) passent avant MJPEG : pas de décodage JPEG, donc
/// moins de charge processeur et de latence, et une image sans compression.
fn candidates(w: u32, h: u32, fps: u32, pref: Option<FrameFormat>) -> Vec<(String, RequestedFormatType)> {
    let res = Resolution::new(w, h);
    let mut order = vec![FrameFormat::YUYV, FrameFormat::NV12, FrameFormat::MJPEG];
    if let Some(p) = pref {
        order.retain(|f| *f != p);
        order.insert(0, p);
    }
    let mut v: Vec<(String, RequestedFormatType)> = order
        .iter()
        .map(|&f| (format!("{f:?} {w}x{h} @ {fps}"), RequestedFormatType::Closest(CameraFormat::new(res, f, fps))))
        .collect();
    v.push(("fréquence d'images maximale".into(), RequestedFormatType::AbsoluteHighestFrameRate));
    v.push(("résolution maximale".into(), RequestedFormatType::AbsoluteHighestResolution));
    v.push(("choix automatique".into(), RequestedFormatType::None));
    v
}

/// Choisit le meilleur format réellement proposé par la carte : le premier qui donne exactement
/// la résolution et la fréquence demandées ; sinon le plus proche. Ouvre ensuite le flux.
fn open_camera(index: u32, w: u32, h: u32, fps: u32, pref: Option<FrameFormat>) -> Result<Camera, String> {
    let tries = candidates(w, h, fps, pref);
    let mut errs = Vec::new();
    let mut best: Option<(usize, (bool, u64, u32))> = None;

    for (i, (label, ty)) in tries.iter().enumerate() {
        let req = RequestedFormat::new::<RgbFormat>(*ty);
        match Camera::new(CameraIndex::Index(index), req) {
            Ok(cam) => {
                let f = cam.camera_format();
                let (rw, rh) = (f.resolution().width(), f.resolution().height());
                let exact = rw == w && rh == h;
                if exact && f.frame_rate() >= fps {
                    drop(cam);
                    return open_stream_of(index, *ty, label);
                }
                let score = (exact, (rw as u64 * rh as u64).min(w as u64 * h as u64), f.frame_rate());
                if best.map_or(true, |(_, s)| score > s) {
                    best = Some((i, score));
                }
            }
            Err(e) => errs.push(format!("  - {label} : {e:?}")),
        }
    }
    match best {
        Some((i, _)) => open_stream_of(index, tries[i].1, &tries[i].0),
        None => Err(errs.join("\n")),
    }
}

fn open_stream_of(index: u32, ty: RequestedFormatType, label: &str) -> Result<Camera, String> {
    let req = RequestedFormat::new::<RgbFormat>(ty);
    let mut cam = Camera::new(CameraIndex::Index(index), req).map_err(|e| format!("  - {label} : {e:?}"))?;
    cam.open_stream().map_err(|e| format!("  - {label} : ouverture du flux refusée ({e:?})"))?;
    Ok(cam)
}

fn is_capture_like(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("hd60") || n.contains("nzxt") || n.contains("signal") || n.contains("capture")
}

/// Index forcé, sinon périphérique du même nom, sinon la carte qui ressemble à une HD60, sinon le premier.
fn resolve_video(devs: &[(u32, String)], index: Option<u32>, name: &Option<String>) -> Option<u32> {
    if let Some(i) = index {
        return Some(i);
    }
    if let Some(n) = name {
        if let Some((i, _)) = devs.iter().find(|(_, d)| d == n) {
            return Some(*i);
        }
    }
    devs.iter().find(|(_, n)| is_capture_like(n)).or_else(|| devs.first()).map(|(i, _)| *i)
}

fn wake(proxy: &EventLoopProxy<UserEvent>) {
    let _ = proxy.send_event(UserEvent::Wake);
}

fn capture_thread(
    params: CaptureParams,
    sh: Arc<Shared>,
    running: Arc<AtomicBool>,
    proxy: EventLoopProxy<UserEvent>,
    previous: Option<JoinHandle<()>>,
) {
    sys::boost_thread();
    // Attend que l'ancienne capture ait libéré la carte.
    if let Some(h) = previous {
        let _ = h.join();
    }
    // Une image arrivée juste avant l'arrêt de l'ancienne capture ne doit pas s'afficher.
    if let Some(old) = sh.frame.lock().unwrap().take() {
        sh.pool.lock().unwrap().push(old.px);
    }
    *sh.status.lock().unwrap() = "Ouverture de la carte…".into();
    *sh.error.lock().unwrap() = None;
    wake(&proxy);

    let fail = |msg: String| {
        eprintln!("{msg}");
        *sh.error.lock().unwrap() = Some(msg);
        wake(&proxy);
    };

    let devs = list_video();
    let Some(index) = resolve_video(&devs, params.index, &params.name) else {
        fail("Aucun périphérique vidéo trouvé. Vérifiez le câble USB 3.x.".into());
        return;
    };
    if let Some((_, n)) = devs.iter().find(|(i, _)| *i == index) {
        eprintln!("Carte : {n}");
    }

    let mut cam = match open_camera(index, params.w, params.h, params.fps, params.format) {
        Ok(c) => c,
        Err(e) => {
            fail(format!(
                "Impossible d'ouvrir la carte. Essais effectués :\n{e}\n\n\
                 - Fermez NZXT CAM, OBS, Discord, le navigateur et l'appli Caméra de Windows.\n\
                 - Windows : Paramètres > Confidentialité et sécurité > Caméra : autorisez les applications de bureau.\n\
                 - Branchez la carte sur un port USB 3.x (bleu), sans hub.\n\
                 - Essayez un autre format ou une autre résolution dans ce menu."
            ));
            return;
        }
    };
    let f = cam.camera_format();
    eprintln!(
        "Flux vidéo : {}x{} @ {} i/s ({:?})",
        f.resolution().width(), f.resolution().height(), f.frame_rate(), f.format()
    );
    *sh.stream.lock().unwrap() = format!("{}x{} {:?}", f.resolution().width(), f.resolution().height(), f.format());
    *sh.status.lock().unwrap() = "Carte ouverte, en attente de la première image…".into();
    wake(&proxy);

    let mut count = 0u32;
    let mut t0 = Instant::now();

    while running.load(Ordering::SeqCst) {
        let buf = match cam.frame() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Erreur de capture : {e}");
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let res = buf.resolution();
        let (fw, fh) = (res.width() as usize, res.height() as usize);

        let mut px = sh.pool.lock().unwrap().pop().unwrap_or_default();
        // YUYV brut : on copie les octets tels quels (2 par pixel) et la carte graphique fait la conversion en RGB,
        // avec des couleurs interpolées. Les autres formats (NV12, MJPEG) sont décodés en RGBA ici.
        let raw = buf.buffer();
        let yuyv = buf.source_frame_format() == FrameFormat::YUYV && fw % 2 == 0 && raw.len() == fw * fh * 2;
        px.resize(if yuyv { fw * fh / 2 } else { fw * fh }, 0);
        // SAFETY : un Vec<u32> de n éléments est lisible comme 4n octets (alignement 1 <= 4).
        let bytes = unsafe { std::slice::from_raw_parts_mut(px.as_mut_ptr() as *mut u8, px.len() * 4) };
        if yuyv {
            bytes.copy_from_slice(raw);
        } else if buf.decode_image_to_buffer::<RgbAFormat>(bytes).is_err() {
            sh.pool.lock().unwrap().push(px);
            continue; // image corrompue : on passe à la suivante
        }

        // On remplace toujours l'image précédente : aucune file d'attente, donc aucun retard cumulé.
        let old = sh.frame.lock().unwrap().replace(Frame { w: fw, h: fh, yuyv, px });
        if let Some(o) = old {
            sh.pool.lock().unwrap().push(o.px);
        }
        if !sh.pending.swap(true, Ordering::SeqCst) {
            wake(&proxy);
        }

        count += 1;
        if t0.elapsed() >= Duration::from_secs(1) {
            *sh.fps.lock().unwrap() = count as f32 / t0.elapsed().as_secs_f32();
            count = 0;
            t0 = Instant::now();
        }
    }
    let _ = cam.stop_stream();
}

// ---------------------------------------------------------------------------
// Fenêtre, menu et affichage
// ---------------------------------------------------------------------------

/// Nom du matériel qui fait le rendu, tel qu'affiché : la carte graphique, ou le processeur (rendu logiciel
/// WARP, qui calcule sur les cœurs du processeur mais s'annonce « Microsoft Basic Render Driver »).
fn render_label(compute: Compute, adapter: &str) -> String {
    static CPU: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    match compute {
        Compute::Gpu => adapter.to_string(),
        Compute::Cpu => {
            let cpu = CPU.get_or_init(|| sys::cpu_name().unwrap_or_else(|| "Processeur".into()));
            format!("{cpu} (rendu logiciel)")
        }
    }
}

type CaptureKey =(Option<String>, u32, u32, u32, Format);

fn capture_key(s: &Settings) -> CaptureKey {
    (s.video_name.clone(), s.width, s.height, s.fps, s.format)
}

struct App {
    settings: Settings,
    sh: Arc<Shared>,
    ctl: Arc<audio::AudioControl>,
    audio: Option<(cpal::Stream, cpal::Stream)>,
    audio_status: String,
    cli_device: Option<u32>,
    cli_audio: Option<usize>,
    cli_direct: bool,
    /// Demandé par le bouton « Quitter » ; traité dans `about_to_wait`, qui a accès à la boucle.
    quit: bool,
    capture: Option<Capture>,
    applied: CaptureKey,
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    ui: Option<Ui>,
    mods: ModifiersState,
    title_t: Instant,
    src_size: (usize, usize),
    message: String,
    message_t: Instant,
    /// Prochain rendu demandé par le menu (animation) : jamais plus d'un toutes les 8 ms.
    repaint_at: Option<Instant>,
}

impl App {
    fn flash(&mut self, text: impl Into<String>) {
        self.message = text.into();
        self.message_t = Instant::now();
    }

    fn start_capture(&mut self) {
        let previous = self.capture.take().map(|c| {
            c.running.store(false, Ordering::SeqCst);
            c.handle
        });
        let running = Arc::new(AtomicBool::new(true));
        let s = &self.settings;
        let params = CaptureParams {
            index: self.cli_device.take(),
            name: s.video_name.clone(),
            w: s.width,
            h: s.height,
            fps: s.fps,
            format: s.format.frame_format(),
        };
        self.applied = capture_key(s);
        *self.sh.fps.lock().unwrap() = 0.0;
        let (sh, r, proxy) = (self.sh.clone(), running.clone(), self.proxy.clone());
        let handle = thread::spawn(move || capture_thread(params, sh, r, proxy, previous));
        self.capture = Some(Capture { running, handle });
    }

    fn at_home(&self) -> bool {
        self.ui.as_ref().is_some_and(|u| u.home)
    }

    /// Accueil -> direct : le son démarre et le panneau se range. La capture tournait déjà pour l'aperçu ;
    /// on la relance seulement si elle a échoué ou si ses réglages ont changé.
    fn start_live(&mut self) {
        let Some(ui) = self.ui.as_mut() else { return };
        ui.home = false;
        self.set_menu(false);
        let failed = self.sh.error.lock().unwrap().is_some();
        if failed || capture_key(&self.settings) != self.applied || self.capture.is_none() {
            self.start_capture();
        }
        self.start_audio();
        self.update_cursor();
    }

    /// Direct -> accueil : le son s'arrête, l'aperçu continue.
    fn go_home(&mut self) {
        self.audio = None;
        self.audio_status.clear();
        if let Some(ui) = self.ui.as_mut() {
            ui.home = true;
        }
        self.set_menu(false);
        self.refresh_lists();
        if let Some(w) = &self.window {
            w.set_title("Lecteur HD60");
            w.request_redraw();
        }
        self.update_cursor();
    }

    fn start_audio(&mut self) {
        self.audio = None; // libère les anciens flux avant d'en ouvrir de nouveaux
        self.audio_status.clear();
        if self.at_home() {
            return; // le son ne démarre qu'avec le direct
        }
        if !self.settings.audio_on {
            self.audio_status = "Son désactivé.".into();
            return;
        }
        let index = self.cli_audio.take().or_else(|| {
            let name = self.settings.audio_name.as_ref()?;
            audio::list_inputs().into_iter().find(|(_, n)| n == name).map(|(i, _)| i)
        });
        match audio::start(index, self.ctl.clone()) {
            Ok(s) => self.audio = Some(s),
            Err(e) => {
                eprintln!("Son désactivé : {e}");
                self.audio_status = format!("Son désactivé : {e}");
            }
        }
    }

    fn refresh_lists(&mut self) {
        if let Some(ui) = self.ui.as_mut() {
            ui.video_list = list_video().into_iter().map(|(_, n)| n).collect();
            ui.audio_list = audio::list_inputs().into_iter().map(|(_, n)| n).collect();
        }
    }

    fn set_menu(&mut self, open: bool) {
        let Some(ui) = self.ui.as_mut() else { return };
        if ui.open == open {
            return;
        }
        ui.open = open;
        if open {
            self.refresh_lists();
        }
        self.update_cursor();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn update_cursor(&self) {
        let menu = self.ui.as_ref().is_some_and(|u| u.wants_input());
        if let Some(w) = &self.window {
            w.set_cursor_visible(menu || w.fullscreen().is_none());
        }
    }

    fn set_fullscreen(&mut self, on: bool) {
        self.settings.fullscreen = on;
        if let Some(w) = &self.window {
            w.set_fullscreen(if on { Some(Fullscreen::Borderless(None)) } else { None });
        }
        self.update_cursor();
    }

    /// Recrée l'affichage sur le matériel choisi (carte graphique ou processeur). Si le nouveau matériel
    /// est inutilisable, on revient aux réglages `before`.
    fn rebuild_gfx(&mut self, before: &Settings) {
        let Some(window) = self.window.clone() else { return };
        // La surface de la fenêtre doit être libérée avant d'en créer une autre.
        self.gfx = None;
        let make = |s: &Settings| {
            pollster::block_on(Gfx::new(window.clone(), s.compute, s.present, s.filter, s.sharpen))
        };
        let gfx = match make(&self.settings) {
            Ok(g) => {
                let txt = match self.settings.compute {
                    Compute::Gpu => format!("Rendu sur la carte graphique : {}", g.adapter_name),
                    Compute::Cpu => "Rendu par le processeur : réglages d'affichage allégés".to_string(),
                };
                self.flash(txt);
                g
            }
            Err(e) => {
                eprintln!("Changement de matériel impossible : {e}");
                self.settings = before.clone();
                match make(&self.settings) {
                    Ok(g) => {
                        self.flash(format!("Changement impossible : {e}"));
                        g
                    }
                    Err(e2) => {
                        eprintln!("Affichage impossible : {e2}");
                        self.quit = true;
                        return;
                    }
                }
            }
        };
        eprintln!("Rendu : {}", gfx.adapter_name);
        if let Some(ui) = self.ui.as_mut() {
            ui.rebuild(&window, gfx.device(), gfx.surface_format());
        }
        self.gfx = Some(gfx);
        window.request_redraw();
    }

    /// Applique les réglages qui ont changé depuis `before`.
    fn apply_diff(&mut self, before: &Settings) {
        if self.settings.compute != before.compute {
            self.settings.adapt_compute(before.compute);
            self.rebuild_gfx(before);
        }
        let s = self.settings.clone();
        if let Some(g) = self.gfx.as_mut() {
            if s.filter != before.filter {
                g.set_filter(s.filter);
            }
            if s.sharpen != before.sharpen {
                g.set_sharpen(s.sharpen);
            }
            if s.present != before.present {
                g.set_present(s.present);
            }
        }
        if s.volume != before.volume {
            self.ctl.set_volume(s.volume);
        }
        if s.muted != before.muted {
            self.ctl.muted.store(s.muted, Ordering::Relaxed);
        }
        if s.audio_on != before.audio_on || s.audio_name != before.audio_name {
            self.start_audio();
        }
        let fs_now = self.window.as_ref().is_some_and(|w| w.fullscreen().is_some());
        if s.fullscreen != fs_now {
            self.set_fullscreen(s.fullscreen);
        }
    }

    fn handle_actions(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::StartLive => self.start_live(),
                Action::GoHome => self.go_home(),
                Action::Quit => self.quit = true,
                Action::ApplyCapture => {
                    self.start_capture();
                    self.flash("Capture relancée");
                }
                Action::RefreshDevices => {
                    self.refresh_lists();
                    self.flash("Périphériques actualisés");
                }
                Action::Save => match self.settings.save() {
                    Ok(p) => self.flash(format!("Réglages enregistrés : {}", p.display())),
                    Err(e) => self.flash(format!("Enregistrement impossible : {e}")),
                },
                Action::Reset => {
                    let old = mem::replace(&mut self.settings, Settings::default());
                    self.apply_diff(&old);
                    self.start_capture();
                    self.flash("Valeurs par défaut rétablies (non enregistrées)");
                }
            }
        }
    }

    fn redraw(&mut self) {
        self.sh.pending.store(false, Ordering::SeqCst);

        let new = self.sh.frame.lock().unwrap().take();
        if let (Some(f), Some(g)) = (new, self.gfx.as_mut()) {
            // SAFETY : lecture d'un Vec<u32> comme 4n octets (voir capture_thread).
            let bytes = unsafe { std::slice::from_raw_parts(f.px.as_ptr() as *const u8, f.px.len() * 4) };
            if f.yuyv {
                g.upload_yuyv(f.w as u32, f.h as u32, bytes);
            } else {
                g.upload(f.w as u32, f.h as u32, bytes);
            }
            self.src_size = (f.w, f.h);
            self.sh.pool.lock().unwrap().push(f.px);
        }
        if self.message_t.elapsed() > Duration::from_secs(4) {
            self.message.clear();
        }
        let error = self.sh.error.lock().unwrap().clone();

        let before = self.settings.clone();
        let mut actions: Vec<Action> = Vec::new();
        let mut repaint = false;
        {
            let App { gfx, ui, window, settings, sh, audio_status, applied, message, .. } = self;
            let (Some(gfx), Some(ui), Some(window)) = (gfx.as_mut(), ui.as_mut(), window.as_ref()) else { return };
            let has_frame = gfx.has_frame();
            // Le panneau s'ouvre et se range en glissant : l'image se place dans la place qu'il laisse.
            let ppp = window.scale_factor() as f32 * ui::zoom_for(settings.ui_scale, window.inner_size().height);
            let (inset, animating) = ui.tick(ppp);
            gfx.set_inset_right(inset);
            if animating {
                repaint = true;
            }
            let need_ui = ui.wants_input() || ui.panel_visible() || settings.show_info || !has_frame || !message.is_empty();
            if need_ui {
                let info = Info {
                    has_frame,
                    error,
                    status: sh.status.lock().unwrap().clone(),
                    stream: sh.stream.lock().unwrap().clone(),
                    fps: *sh.fps.lock().unwrap(),
                    gpu: render_label(settings.compute, &gfx.adapter_name),
                    present_label: gfx.present_label.to_string(),
                    filter_label: gfx.filter_label().to_string(),
                    audio_status: audio_status.clone(),
                    capture_dirty: capture_key(settings) != *applied,
                    message: message.clone(),
                };
                gfx.render_with(|dev, q, enc, view, size| {
                    let (cmds, r) = ui.paint(window, dev, q, enc, view, size, settings, &info, &mut actions);
                    repaint |= r;
                    cmds
                });
            } else {
                gfx.render_with(|_, _, _, _, _| Vec::new());
            }
        }
        self.apply_diff(&before);
        self.handle_actions(actions);
        // Le menu a pu être fermé par son bouton.
        self.update_cursor();
        if repaint {
            // Plafond de cadence : sans lui, le mode « Immédiat » (aucune synchronisation) redessinerait
            // en boucle à pleine vitesse et ferait monter la carte graphique à 90 %.
            self.repaint_at = Some(Instant::now() + Duration::from_millis(8));
        }
        self.update_title();
    }

    fn update_title(&mut self) {
        if self.title_t.elapsed() < Duration::from_millis(500) || self.src_size.0 == 0 {
            return;
        }
        self.title_t = Instant::now();
        if let Some(w) = &self.window {
            let fps = *self.sh.fps.lock().unwrap();
            let stream = self.sh.stream.lock().unwrap().clone();
            w.set_title(&format!("Lecteur HD60 — {stream} — {fps:.0} i/s"));
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // Icône de la fenêtre et de la barre des tâches : le logo, en 64x64 RGBA brut (assets/icon-64.rgba).
        let icon = winit::window::Icon::from_rgba(include_bytes!("../assets/icon-64.rgba").to_vec(), 64, 64).ok();
        let mut attrs = Window::default_attributes()
            .with_title("Lecteur HD60")
            .with_window_icon(icon)
            .with_inner_size(PhysicalSize::new(1280u32, 720u32));
        if self.settings.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Fenêtre impossible : {e}");
                el.exit();
                return;
            }
        };
        let make = |s: &Settings| {
            pollster::block_on(Gfx::new(window.clone(), s.compute, s.present, s.filter, s.sharpen))
        };
        let gfx = match make(&self.settings) {
            Ok(g) => g,
            Err(e) if self.settings.compute == Compute::Gpu => {
                // Pas de carte graphique utilisable : on essaie le rendu par le processeur avant d'abandonner.
                eprintln!("Affichage sur la carte graphique impossible : {e}");
                self.settings.compute = Compute::Cpu;
                self.settings.adapt_compute(Compute::Gpu);
                match make(&self.settings) {
                    Ok(g) => {
                        self.flash("Carte graphique indisponible : rendu par le processeur");
                        g
                    }
                    Err(e2) => {
                        eprintln!("Affichage sur le processeur impossible : {e2}");
                        el.exit();
                        return;
                    }
                }
            }
            Err(e) => {
                eprintln!("Affichage impossible : {e}");
                el.exit();
                return;
            }
        };
        eprintln!("Rendu : {}", gfx.adapter_name);
        let home = !(self.cli_direct || self.settings.skip_home);
        self.ui = Some(Ui::new(&window, gfx.device(), gfx.surface_format(), home));
        self.gfx = Some(gfx);
        self.window = Some(window);
        self.update_cursor();
        sys::boost_thread();

        self.ctl.set_volume(self.settings.volume);
        self.ctl.muted.store(self.settings.muted, Ordering::Relaxed);
        self.refresh_lists();
        // La capture démarre une fois la fenêtre prête (l'accueil affiche l'aperçu) ; le son attend le direct.
        self.start_capture();
        if !home {
            self.start_audio();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.quit {
            el.exit();
            return;
        }
        match self.repaint_at {
            Some(t) if Instant::now() >= t => {
                self.repaint_at = None;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
                el.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, _ev: UserEvent) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
        let Some(window) = self.window.clone() else { return };

        // Touches essentielles : traitées avant le menu, pour qu'il ne les « mange » pas.
        if let WindowEvent::KeyboardInput {
            event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Pressed, repeat: false, .. },
            ..
        } = &ev
        {
            let menu_open = self.ui.as_ref().is_some_and(|u| u.open);
            let home = self.at_home();
            let fs = window.fullscreen().is_some();
            match code {
                KeyCode::Tab | KeyCode::F1 => {
                    self.set_menu(!menu_open);
                    return;
                }
                KeyCode::Enter | KeyCode::NumpadEnter if home && !menu_open && !self.mods.alt_key() => {
                    self.start_live();
                    window.request_redraw();
                    return;
                }
                KeyCode::KeyH if !home && !menu_open => {
                    self.go_home();
                    return;
                }
                KeyCode::F11 => {
                    self.set_fullscreen(!fs);
                    return;
                }
                KeyCode::Enter | KeyCode::NumpadEnter if self.mods.alt_key() => {
                    self.set_fullscreen(!fs);
                    return;
                }
                KeyCode::Escape => {
                    if menu_open {
                        self.set_menu(false);
                    } else if fs {
                        self.set_fullscreen(false);
                    } else if !home {
                        self.go_home();
                    } else {
                        el.exit();
                    }
                    return;
                }
                _ => {}
            }
        }

        // Accueil ou réglages ouverts : egui reçoit les événements (souris, clavier). Pas l'événement de redessin
        // lui-même : egui y répond toujours « repaint », ce qui relançait un rendu à l'infini (carte graphique à 90 %).
        if self.ui.as_ref().is_some_and(|u| u.wants_input()) && !matches!(ev, WindowEvent::RedrawRequested) {
            if let Some(ui) = self.ui.as_mut() {
                let resp = ui.on_event(&window, &ev);
                if resp.repaint {
                    window.request_redraw();
                }
                if resp.consumed {
                    return;
                }
            }
        }

        match ev {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.resize(size);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Pressed, repeat, .. },
                ..
            } => {
                let menu_open = self.ui.as_ref().is_some_and(|u| u.open);
                if menu_open {
                    return;
                }
                match code {
                    KeyCode::KeyF if !repeat => {
                        let fs = window.fullscreen().is_some();
                        self.set_fullscreen(!fs);
                    }
                    KeyCode::KeyQ if !repeat => el.exit(),
                    // À l'accueil, il n'y a ni son ni infos à régler.
                    _ if self.at_home() => {}
                    KeyCode::KeyI if !repeat => {
                        self.settings.show_info = !self.settings.show_info;
                        window.request_redraw();
                    }
                    KeyCode::KeyM if !repeat => {
                        self.settings.muted = !self.settings.muted;
                        self.ctl.muted.store(self.settings.muted, Ordering::Relaxed);
                        let txt = if self.settings.muted { "Son coupé" } else { "Son rétabli" };
                        self.flash(txt);
                        window.request_redraw();
                    }
                    KeyCode::ArrowUp | KeyCode::ArrowDown => {
                        let d = if code == KeyCode::ArrowUp { 0.05 } else { -0.05 };
                        self.ctl.set_volume(self.ctl.volume() + d);
                        self.settings.volume = self.ctl.volume();
                        self.flash(format!("Volume {:.0} %", self.settings.volume * 100.0));
                        window.request_redraw();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let no_config = env::args().any(|a| a == "--no-config");
    let mut settings = if no_config { Settings::default() } else { Settings::load() };
    let cli = parse_args(&mut settings);
    sys::boost_process();

    #[cfg(target_os = "macos")]
    {
        nokhwa::nokhwa_initialize(|_| {});
        thread::sleep(Duration::from_millis(800));
    }

    if cli.list {
        println!("Vidéo :");
        for (i, n) in &list_video() {
            println!("  [{i}] {n}");
        }
        println!("Entrées audio :");
        for (i, n) in audio::list_inputs() {
            println!("  [{i}] {n}");
        }
        return;
    }

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().unwrap_or_else(|e| {
        eprintln!("Boucle d'événements impossible : {e}");
        process::exit(1);
    });
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let sh = Arc::new(Shared {
        frame: Mutex::new(None),
        pending: AtomicBool::new(false),
        pool: Mutex::new(Vec::new()),
        fps: Mutex::new(0.0),
        stream: Mutex::new(String::new()),
        status: Mutex::new(String::new()),
        error: Mutex::new(None),
    });

    let mut app = App {
        applied: capture_key(&settings),
        settings,
        sh,
        ctl: audio::AudioControl::new(),
        audio: None,
        audio_status: String::new(),
        cli_device: cli.device,
        cli_audio: cli.audio,
        cli_direct: cli.direct,
        quit: false,
        capture: None,
        proxy,
        window: None,
        gfx: None,
        ui: None,
        mods: ModifiersState::empty(),
        title_t: Instant::now(),
        src_size: (0, 0),
        message: String::new(),
        message_t: Instant::now(),
        repaint_at: None,
    };
    let _ = event_loop.run_app(&mut app);

    // Arrêt propre : on laisse 1,5 s au thread de capture pour libérer la carte.
    if let Some(c) = app.capture.take() {
        c.running.store(false, Ordering::SeqCst);
        let t = Instant::now();
        while !c.handle.is_finished() && t.elapsed() < Duration::from_millis(1500) {
            thread::sleep(Duration::from_millis(20));
        }
        if c.handle.is_finished() {
            let _ = c.handle.join();
        }
    }
    drop(app.audio.take());
    process::exit(0);
}
