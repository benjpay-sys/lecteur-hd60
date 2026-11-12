//! Interface : un panneau clair à droite (accueil, puis réglages à onglets) et l'image à gauche.
//! L'image occupe toute la place laissée libre par le panneau ; sans signal, une mire de barres la remplace.
//! Dessiné avec egui par-dessus l'image (rendu par la carte graphique ou le processeur).

use crate::{
    audio,
    gpu::{Compute, Filter, Present},
    settings::{Format, Profile, Settings},
};
use egui::{
    pos2, vec2, Align2, Color32, Context, CornerRadius, FontFamily, FontId, Id, Margin, Rect, RichText, Sense,
    Stroke,
};
use std::{path::PathBuf, sync::Arc, time::Instant};
use winit::{event::WindowEvent, window::Window};

pub enum Action {
    /// Depuis l'accueil : passe en plein direct (le son démarre, le panneau se range).
    StartLive,
    /// Depuis le direct : revient à l'accueil (le son s'arrête, l'aperçu continue).
    GoHome,
    Quit,
    ApplyCapture,
    Save,
    Reset,
    RefreshDevices,
}

/// Informations affichées dans le panneau et les pastilles.
pub struct Info {
    pub has_frame: bool,
    pub error: Option<String>,
    pub status: String,
    pub stream: String,
    pub fps: f32,
    pub gpu: String,
    pub present_label: String,
    pub filter_label: String,
    pub audio_status: String,
    pub capture_dirty: bool,
    pub message: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Image,
    Sound,
    Capture,
    General,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Image, Tab::Sound, Tab::Capture, Tab::General];
    fn label(self) -> &'static str {
        match self {
            Tab::Image => "Image",
            Tab::Sound => "Son",
            Tab::Capture => "Capture",
            Tab::General => "Général",
        }
    }
}

pub struct Ui {
    ctx: Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    /// Réglages affichés dans le panneau (sinon, la page d'accueil).
    pub open: bool,
    /// Accueil : l'aperçu tourne, le son est coupé. Sinon : direct.
    pub home: bool,
    pub tab: Tab,
    pub video_list: Vec<String>,
    pub audio_list: Vec<String>,
    /// Ouverture du panneau, de 0 (rangé) à 1 (ouvert).
    frac: f32,
    last: Instant,
    /// Un message avec des boutons est affiché sur l'image : l'interface doit recevoir la souris.
    card: bool,
}

const RESOLUTIONS: [(u32, u32); 5] = [(1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3840, 2160)];
const FRAMERATES: [u32; 6] = [24, 30, 50, 60, 120, 144];

/// Largeur du panneau, en points.
const PANEL_W: f32 = 344.0;
const MARGIN_X: f32 = 20.0;

// Palette : gris froid très clair et un bleu outremer. Le bleu ne sert qu'à l'action principale et à la
// sélection ; vert, orange et rouge ne servent qu'aux voyants. Le fond de l'image est un gris presque noir.
const PANEL: Color32 = Color32::from_rgb(0xF6, 0xF7, 0xF9);
const FIELD: Color32 = Color32::from_rgb(0xE6, 0xE9, 0xEF);
const HOVER: Color32 = Color32::from_rgb(0xED, 0xEF, 0xF4);
const LINE: Color32 = Color32::from_rgb(0xD9, 0xDD, 0xE5);
const TEXT: Color32 = Color32::from_rgb(0x14, 0x17, 0x1C);
const MUTED: Color32 = Color32::from_rgb(0x5B, 0x64, 0x72);
const ACCENT: Color32 = Color32::from_rgb(0x22, 0x43, 0xD4);
const ACCENT_DARK: Color32 = Color32::from_rgb(0x1A, 0x35, 0xB0);
const ACCENT_SOFT: Color32 = Color32::from_rgb(0xE4, 0xEA, 0xFC);
const OK: Color32 = Color32::from_rgb(0x1C, 0x8A, 0x4B);
const WARN: Color32 = Color32::from_rgb(0xA8, 0x62, 0x0A);
const BAD: Color32 = Color32::from_rgb(0xC2, 0x30, 0x3A);
const STAGE: Color32 = Color32::from_rgb(0x0D, 0x0F, 0x13);

/// Police des titres et des valeurs : Bahnschrift (style DIN) si Windows la fournit.
fn titre(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("titre".into()))
}

fn muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(13.0).color(MUTED)
}

/// Polices et style de l'interface. À appeler une fois après la création du contexte egui.
/// Segoe UI et Bahnschrift sont lues dans le dossier des polices de Windows ; sans elles, egui garde ses polices.
pub fn setup(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    let dir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts");
    let mut load = |key: &str, file: &str| -> bool {
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                fonts.font_data.insert(key.to_string(), Arc::new(egui::FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    let has_segoe = load("segoeui", "segoeui.ttf");
    let has_bahn = load("bahnschrift", "bahnschrift.ttf");
    if has_segoe {
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoeui".into());
    }
    let mut family = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if has_bahn {
        family.insert(0, "bahnschrift".into());
    }
    fonts.families.insert(FontFamily::Name("titre".into()), family);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut(|st| {
        use egui::TextStyle::*;
        st.text_styles.insert(Heading, titre(22.0));
        st.text_styles.insert(Body, FontId::new(14.5, FontFamily::Proportional));
        st.text_styles.insert(Button, FontId::new(14.5, FontFamily::Proportional));
        st.text_styles.insert(Small, FontId::new(12.5, FontFamily::Proportional));
        st.text_styles.insert(Monospace, FontId::new(13.5, FontFamily::Monospace));
        st.spacing.item_spacing = vec2(10.0, 8.0);
        st.spacing.button_padding = vec2(12.0, 6.0);
        st.spacing.interact_size.y = 30.0;
        // Pas de fondu à l'apparition des zones : le panneau a sa propre animation de glissement.
        st.animation_time = 0.0;

        let v = &mut st.visuals;
        *v = egui::Visuals::light();
        v.panel_fill = PANEL;
        v.window_fill = Color32::WHITE;
        v.extreme_bg_color = Color32::WHITE;
        v.faint_bg_color = HOVER;
        v.window_stroke = Stroke::new(1.0_f32, LINE);
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.selection.bg_fill = ACCENT_SOFT;
        v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        v.hyperlink_color = ACCENT;

        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.inactive.bg_fill = Color32::from_rgb(0xDF, 0xE3, 0xEA);
        w.inactive.weak_bg_fill = Color32::WHITE;
        w.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xAE, 0xB6, 0xC4));
        w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.hovered.bg_fill = Color32::from_rgb(0xC4, 0xCB, 0xD8);
        w.hovered.weak_bg_fill = HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0_f32, MUTED);
        w.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.active.bg_fill = ACCENT_SOFT;
        w.active.weak_bg_fill = ACCENT_SOFT;
        w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
        w.active.fg_stroke = Stroke::new(1.0_f32, TEXT);
        w.open.bg_fill = Color32::WHITE;
        w.open.weak_bg_fill = Color32::WHITE;
        w.open.fg_stroke = Stroke::new(1.0_f32, TEXT);
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = CornerRadius::same(6);
        }
    });
}

/// Facteur d'agrandissement de l'interface. Il se réduit si la fenêtre est basse, pour que le panneau tienne.
pub fn zoom_for(ui_scale: f32, height_px: u32) -> f32 {
    ui_scale.min(height_px as f32 / 680.0).max(0.85)
}

impl Ui {
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat, home: bool) -> Self {
        let ctx = Context::default();
        setup(&ctx);
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let renderer = egui_wgpu::Renderer::new(device, format, None, 1, false);
        Self {
            ctx,
            state,
            renderer,
            open: false,
            home,
            tab: Tab::Image,
            video_list: Vec::new(),
            audio_list: Vec::new(),
            frac: if home { 1.0 } else { 0.0 },
            last: Instant::now(),
            card: false,
        }
    }

    /// Recrée l'interface pour un nouveau périphérique graphique (changement carte graphique / processeur) :
    /// le moteur de dessin et la texture des polices sont liés à l'ancien périphérique.
    pub fn rebuild(&mut self, window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) {
        let mut new = Ui::new(window, device, format, self.home);
        new.open = self.open;
        new.tab = self.tab;
        new.frac = self.frac;
        new.video_list = std::mem::take(&mut self.video_list);
        new.audio_list = std::mem::take(&mut self.audio_list);
        *self = new;
    }

    /// L'interface reçoit les événements (souris, clavier) : panneau affiché ou message à boutons sur l'image.
    pub fn wants_input(&self) -> bool {
        self.open || self.home || self.card
    }

    /// Le panneau est au moins en partie visible.
    pub fn panel_visible(&self) -> bool {
        self.frac > 0.0
    }

    fn eased(&self) -> f32 {
        1.0 - (1.0 - self.frac).powi(3)
    }

    /// Fait avancer l'animation d'ouverture du panneau. Renvoie la largeur (pixels) que le panneau prend
    /// à droite de la fenêtre, et `true` tant que l'animation continue. `ppp` : pixels par point egui.
    pub fn tick(&mut self, ppp: f32) -> (u32, bool) {
        let target = if self.open || self.home { 1.0 } else { 0.0 };
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.05);
        self.last = now;
        let step = dt / 0.18;
        self.frac = if self.frac < target { (self.frac + step).min(target) } else { (self.frac - step).max(target) };
        ((self.eased() * PANEL_W * ppp).round() as u32, self.frac != target)
    }

    pub fn on_event(&mut self, window: &Window, ev: &WindowEvent) -> egui_winit::EventResponse {
        self.state.on_window_event(window, ev)
    }

    /// Construit l'interface et la dessine sur `view`. Renvoie les commandes à envoyer et
    /// `true` si egui demande de redessiner tout de suite (animation en cours).
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: [u32; 2],
        settings: &mut Settings,
        info: &Info,
        actions: &mut Vec<Action>,
    ) -> (Vec<wgpu::CommandBuffer>, bool) {
        let eased = self.eased();
        let home = self.home;
        self.card = !home && !info.has_frame;
        let Ui { ctx, state, renderer, open, tab, video_list, audio_list, .. } = self;
        ctx.set_zoom_factor(zoom_for(settings.ui_scale, size[1]));
        let raw = state.take_egui_input(window);
        let out = ctx.run(raw, |ctx| {
            draw(ctx, open, home, tab, eased, video_list, audio_list, settings, info, actions)
        });
        state.handle_platform_output(window, out.platform_output);

        let ppp = out.pixels_per_point;
        let jobs = ctx.tessellate(out.shapes, ppp);
        for (id, delta) in &out.textures_delta.set {
            renderer.update_texture(device, queue, *id, delta);
        }
        let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: ppp };
        let cmds = renderer.update_buffers(device, queue, enc, &jobs, &sd);
        {
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("interface"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut pass = pass.forget_lifetime();
            renderer.render(&mut pass, &jobs, &sd);
        }
        for id in &out.textures_delta.free {
            renderer.free_texture(id);
        }
        let repaint = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay.is_zero());
        (cmds, repaint)
    }
}

// ---------------------------------------------------------------------------
// Composants
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Level {
    Ok,
    Warn,
    Bad,
}

impl Level {
    fn color(self) -> Color32 {
        match self {
            Level::Ok => OK,
            Level::Warn => WARN,
            Level::Bad => BAD,
        }
    }
}

/// Ligne de matériel : voyant, nom de l'élément, valeur détectée (coupée si trop longue).
fn status_row(ui: &mut egui::Ui, label: &str, value: &str, level: Level) {
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.5, level.color());
        ui.allocate_ui_with_layout(vec2(58.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(58.0);
            ui.label(muted(label));
        });
        ui.add(egui::Label::new(RichText::new(value).font(titre(14.5))).truncate()).on_hover_text(value);
    });
}

/// Choix entre deux ou trois options, sur toute la largeur. Renvoie `true` si la valeur a changé.
fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let w = ui.available_width();
    let (track, _) = ui.allocate_exact_size(vec2(w, 38.0), Sense::hover());
    ui.painter().rect_filled(track, CornerRadius::same(10), FIELD);
    let n = options.len() as f32;
    let seg_w = (w - 6.0) / n;
    let mut changed = false;
    for (i, (v, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(track.left() + 3.0 + seg_w * i as f32, track.top() + 3.0), vec2(seg_w, 32.0));
        let resp = ui.interact(r, ui.id().with(("seg", i)), Sense::click());
        let selected = *value == *v;
        if selected {
            ui.painter().rect_filled(r, CornerRadius::same(8), Color32::WHITE);
            ui.painter().rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(8), HOVER);
        }
        let color = if selected { TEXT } else { MUTED };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, titre(14.5), color);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, *label));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !selected {
            *value = *v;
            changed = true;
        }
    }
    changed
}

/// Ligne de liste à choix unique : toute la ligne se clique.
fn radio_row(ui: &mut egui::Ui, name: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 33.0), Sense::click());
    let fill = if selected {
        ACCENT_SOFT
    } else if resp.hovered() {
        HOVER
    } else {
        Color32::TRANSPARENT
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(8), fill);
    let c = pos2(rect.left() + 18.0, rect.center().y);
    p.circle_stroke(c, 7.5, Stroke::new(1.5_f32, if selected { ACCENT } else { MUTED }));
    if selected {
        p.circle_filled(c, 3.8, ACCENT);
    }
    p.text(pos2(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, name, titre(15.5), TEXT);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, name));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Bouton principal : plein bleu, toute la largeur. Un seul par écran.
fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    let fill = if resp.is_pointer_button_down_on() || resp.hovered() { ACCENT_DARK } else { ACCENT };
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, titre(18.0), Color32::WHITE);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Bouton sans cadre, pour les actions secondaires.
fn link(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(color)).frame(false)).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Onglets : le texte choisi est foncé et souligné en bleu.
fn tabs(ui: &mut egui::Ui, current: &mut Tab) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for t in Tab::ALL {
            let galley = ui.painter().layout_no_wrap(t.label().to_string(), titre(15.0), TEXT);
            let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 24.0, 34.0), Sense::click());
            let selected = *current == t;
            let color = if selected { TEXT } else if resp.hovered() { TEXT } else { MUTED };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, t.label(), titre(15.0), color);
            if selected {
                let y = rect.bottom();
                ui.painter().line_segment(
                    [pos2(rect.left() + 12.0, y), pos2(rect.right() - 12.0, y)],
                    Stroke::new(2.5_f32, ACCENT),
                );
            }
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, t.label()));
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                *current = t;
            }
        }
    });
    let y = ui.cursor().top();
    ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0_f32, LINE));
    ui.add_space(10.0);
}

/// Libellé discret au-dessus d'un contrôle.
fn field(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(muted(label));
    ui.add_space(-2.0);
    add(ui);
    ui.add_space(8.0);
}

// ---------------------------------------------------------------------------
// Image : mire sans signal, message, pastilles
// ---------------------------------------------------------------------------

/// Mire de barres (SMPTE) à la place de l'image tant qu'aucun signal n'arrive.
fn test_pattern(painter: &egui::Painter, stage: Rect) {
    painter.rect_filled(stage, CornerRadius::ZERO, STAGE);
    let k = (stage.width() / 16.0).min(stage.height() / 9.0);
    if k <= 1.0 {
        return;
    }
    let r = Rect::from_center_size(stage.center(), vec2(16.0 * k, 9.0 * k));
    let c = |r: u8, g: u8, b: u8| Color32::from_rgb(r, g, b);
    let gray = c(191, 191, 191);
    let black = c(16, 16, 16);
    let bars = [gray, c(191, 191, 0), c(0, 191, 191), c(0, 191, 0), c(191, 0, 191), c(191, 0, 0), c(0, 0, 191)];
    let castle = [c(0, 0, 191), black, c(191, 0, 191), black, c(0, 191, 191), black, gray];
    let bottom: [(f32, Color32); 7] = [
        (1.25, c(0, 33, 76)),
        (1.25, Color32::WHITE),
        (1.25, c(50, 0, 106)),
        (1.25, black),
        (0.667, c(8, 8, 8)),
        (0.667, black),
        (0.666, c(24, 24, 24)),
    ];
    let band = |y0: f32, y1: f32, cols: &[(f32, Color32)]| {
        let unit = r.width() / 7.0;
        let mut x = r.left();
        for (w, col) in cols {
            let x1 = (x + w * unit).min(r.right());
            painter.rect_filled(Rect::from_min_max(pos2(x, y0), pos2(x1 + 0.5, y1)), CornerRadius::ZERO, *col);
            x += w * unit;
        }
    };
    let (h1, h2) = (r.top() + r.height() * 0.67, r.top() + r.height() * 0.75);
    band(r.top(), h1, &bars.map(|col| (1.0, col)));
    band(h1, h2, &castle.map(|col| (1.0, col)));
    band(h2, r.bottom(), &bottom);
}

/// Pastille posée sur l'image : fond blanc, bord fin.
fn hud(ctx: &Context, id: &str, pos: egui::Pos2, pivot: Align2, add: impl FnOnce(&mut egui::Ui)) {
    egui::Area::new(Id::new(id))
        .fixed_pos(pos)
        .pivot(pivot)
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_white_alpha(238))
                .stroke(Stroke::new(1.0_f32, LINE))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::symmetric(14, 7))
                .show(ui, add);
        });
}

#[allow(clippy::too_many_arguments)]
fn draw_stage(ctx: &Context, stage: Rect, home: bool, open: &mut bool, s: &Settings, info: &Info, actions: &mut Vec<Action>) {
    if !info.has_frame {
        test_pattern(&ctx.layer_painter(egui::LayerId::background()), stage);
        egui::Area::new(Id::new("signal"))
            .fixed_pos(stage.center())
            .pivot(Align2::CENTER_CENTER)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::WHITE)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(20))
                    .shadow(egui::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(70) })
                    .show(ui, |ui| {
                        ui.set_max_width((stage.width() - 60.0).clamp(220.0, 440.0));
                        match &info.error {
                            Some(e) => {
                                ui.label(RichText::new("Carte non ouverte").font(titre(20.0)).color(BAD));
                                egui::ScrollArea::vertical().max_height((stage.height() - 180.0).max(80.0)).show(ui, |ui| {
                                    ui.label(RichText::new(e).size(13.0));
                                });
                            }
                            None => {
                                ui.label(RichText::new("Pas de signal").font(titre(20.0)));
                                ui.label(muted(if info.status.is_empty() { "En attente de l'image." } else { &info.status }));
                            }
                        }
                        ui.horizontal(|ui| {
                            if info.error.is_some() && link(ui, "Réessayer", ACCENT).clicked() {
                                actions.push(Action::ApplyCapture);
                            }
                            if !home {
                                if link(ui, "Réglages", ACCENT).clicked() {
                                    *open = true;
                                }
                                if link(ui, "Accueil", ACCENT).clicked() {
                                    actions.push(Action::GoHome);
                                }
                            }
                        });
                    });
            });
    }

    // Pastille d'état : voyant rouge en direct, gris sur l'aperçu (touche I pour la masquer).
    if s.show_info && info.has_frame {
        hud(ctx, "stats", pos2(stage.left() + 14.0, stage.bottom() - 14.0), Align2::LEFT_BOTTOM, |ui| {
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.5, if home { MUTED } else { BAD });
                ui.label(RichText::new(if home { "Aperçu" } else { "Direct" }).font(titre(14.0)).color(TEXT));
                ui.add_space(4.0);
                ui.label(RichText::new(&info.stream).font(titre(14.0)).color(TEXT));
                ui.label(muted(format!("{:.0} i/s", info.fps)));
                ui.label(muted(&info.filter_label));
                ui.label(muted(&info.present_label));
            });
        });
    }
    if !info.message.is_empty() {
        hud(ctx, "message", pos2(stage.center().x, stage.top() + 16.0), Align2::CENTER_TOP, |ui| {
            ui.label(RichText::new(&info.message).color(TEXT));
        });
    }
}

// ---------------------------------------------------------------------------
// Panneau
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn draw(
    ctx: &Context,
    open: &mut bool,
    home: bool,
    tab: &mut Tab,
    frac: f32,
    video_list: &[String],
    audio_list: &[String],
    s: &mut Settings,
    info: &Info,
    actions: &mut Vec<Action>,
) {
    let screen = ctx.screen_rect();
    let shown = PANEL_W * frac;
    let stage = Rect::from_min_max(screen.min, pos2(screen.right() - shown, screen.bottom()));
    draw_stage(ctx, stage, home, open, s, info, actions);
    if frac <= 0.0 {
        return;
    }

    let rect = Rect::from_min_size(pos2(screen.right() - shown, screen.top()), vec2(PANEL_W, screen.height()));
    egui::Area::new(Id::new("panneau"))
        .fixed_pos(rect.min)
        .order(egui::Order::Middle)
        .constrain(false)
        .show(ctx, |ui| {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, PANEL);
            ui.painter().vline(rect.left(), rect.y_range(), Stroke::new(1.0_f32, LINE));
            let inner = Rect::from_min_max(
                pos2(rect.left() + MARGIN_X, rect.top() + 18.0),
                pos2(rect.right() - MARGIN_X, rect.bottom() - 12.0),
            );
            // Raccourcis en bas du panneau, sur toutes les pages ; le contenu occupe le reste.
            let hints = shortcut_items(home, *open);
            let foot_h = 22.0 + hints.len().div_ceil(2) as f32 * 26.0;
            let content = Rect::from_min_max(inner.min, pos2(inner.right(), inner.bottom() - foot_h));
            let foot = Rect::from_min_max(pos2(inner.left(), inner.bottom() - foot_h + 12.0), inner.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                ui.set_width(content.width());
                if *open {
                    settings_page(ui, open, home, tab, video_list, audio_list, s, info, actions);
                } else {
                    home_page(ui, open, video_list, audio_list, s, info, actions);
                }
            });
            ui.painter().hline(inner.x_range(), foot.top() - 6.0, Stroke::new(1.0_f32, LINE));
            ui.scope_builder(egui::UiBuilder::new().max_rect(foot), |ui| {
                ui.set_width(foot.width());
                shortcuts(ui, &hints);
            });
        });
}

/// Raccourcis utiles dans la situation actuelle (certaines touches ne marchent qu'en direct).
fn shortcut_items(home: bool, open: bool) -> Vec<(&'static str, &'static str)> {
    let mut v = Vec::new();
    if home && !open {
        v.push(("Entrée", "Lancer le direct"));
    }
    v.push(("Tab", if open { "Fermer les réglages" } else { "Réglages" }));
    v.push(("F", "Plein écran"));
    if !home {
        v.push(("M", "Sourdine"));
        v.push(("↑ ↓", "Volume"));
        v.push(("I", "Infos à l'écran"));
        v.push(("H", "Accueil"));
    }
    v.push((
        "Échap",
        if open {
            "Fermer"
        } else if home {
            "Quitter"
        } else {
            "Accueil"
        },
    ));
    v
}

/// Une touche dessinée comme une touche de clavier, suivie de son action.
fn key_hint(ui: &mut egui::Ui, key: &str, what: &str, w: f32) {
    ui.allocate_ui_with_layout(vec2(w, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(w);
        ui.spacing_mut().item_spacing.x = 7.0;
        let galley = ui.painter().layout_no_wrap(key.to_string(), titre(12.5), TEXT);
        let size = vec2((galley.size().x + 14.0).max(24.0), 21.0);
        let (r, _) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(5), Color32::WHITE);
        ui.painter().rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0_f32, Color32::from_rgb(0xAE, 0xB6, 0xC4)), egui::StrokeKind::Inside);
        ui.painter().galley(r.center() - galley.size() / 2.0, galley, TEXT);
        ui.add(egui::Label::new(RichText::new(what).size(12.5).color(MUTED)).truncate());
    });
}

fn shortcuts(ui: &mut egui::Ui, items: &[(&str, &str)]) {
    let col = ui.available_width() / 2.0;
    ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
    for pair in items.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (key, what) in pair {
                key_hint(ui, key, what, col);
            }
        });
    }
}

/// Bouton secondaire plein largeur : icône de curseurs, nom clair et rappel de la touche.
fn settings_button(ui: &mut egui::Ui, label: &str, key: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(10), if hovered { HOVER } else { Color32::WHITE });
    p.rect_stroke(
        rect,
        CornerRadius::same(10),
        Stroke::new(1.0_f32, if hovered { MUTED } else { Color32::from_rgb(0xAE, 0xB6, 0xC4) }),
        egui::StrokeKind::Inside,
    );
    // Icône : trois curseurs.
    let (x0, cy) = (rect.left() + 16.0, rect.center().y);
    for (dy, knob) in [(-6.0_f32, 4.0_f32), (0.0, 11.0), (6.0, 7.0)] {
        let y = cy + dy;
        p.line_segment([pos2(x0, y), pos2(x0 + 17.0, y)], Stroke::new(1.6_f32, TEXT));
        p.circle_filled(pos2(x0 + knob, y), 3.0, Color32::WHITE);
        p.circle_stroke(pos2(x0 + knob, y), 3.0, Stroke::new(1.6_f32, TEXT));
    }
    p.text(pos2(rect.left() + 44.0, cy), Align2::LEFT_CENTER, label, titre(16.5), TEXT);
    let galley = p.layout_no_wrap(key.to_string(), titre(12.5), MUTED);
    let kr = Rect::from_center_size(
        pos2(rect.right() - 14.0 - (galley.size().x + 12.0) / 2.0, cy),
        vec2(galley.size().x + 12.0, 21.0),
    );
    p.rect_filled(kr, CornerRadius::same(5), PANEL);
    p.rect_stroke(kr, CornerRadius::same(5), Stroke::new(1.0_f32, LINE), egui::StrokeKind::Inside);
    p.galley(kr.center() - galley.size() / 2.0, galley, MUTED);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Carte vidéo que le lancement utiliserait : celle choisie, sinon la première qui ressemble à une HD60.
fn detected_video(list: &[String], pick: &Option<String>) -> Option<(String, Level)> {
    if let Some(n) = pick {
        if list.contains(n) {
            return Some((n.clone(), Level::Ok));
        }
    }
    if let Some(n) = list.iter().find(|n| crate::is_capture_like(n)) {
        return Some((n.clone(), Level::Ok));
    }
    list.first().map(|n| (n.clone(), Level::Warn))
}

fn detected_audio(list: &[String], s: &Settings) -> (String, Level) {
    if !s.audio_on {
        return ("Désactivé".into(), Level::Warn);
    }
    if let Some(n) = &s.audio_name {
        return if list.contains(n) { (n.clone(), Level::Ok) } else { (format!("{n} (introuvable)"), Level::Bad) };
    }
    match audio::guess_input(list) {
        Some(n) => (n.clone(), Level::Ok),
        None => ("Aucune entrée trouvée".into(), Level::Warn),
    }
}

#[allow(clippy::too_many_arguments)]
fn home_page(
    ui: &mut egui::Ui,
    open: &mut bool,
    video_list: &[String],
    audio_list: &[String],
    s: &mut Settings,
    info: &Info,
    actions: &mut Vec<Action>,
) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Lecteur HD60").font(titre(28.0)).color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                if link(ui, "Actualiser", ACCENT).clicked() {
                    actions.push(Action::RefreshDevices);
                }
            });
        });
        ui.add_space(6.0);

        // ----- Matériel détecté -----
        let video = detected_video(video_list, &s.video_name);
        let (audio_txt, audio_level) = detected_audio(audio_list, s);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            match &video {
                Some((n, l)) => status_row(ui, "Carte", n, *l),
                None => status_row(ui, "Carte", "Aucune carte détectée", Level::Bad),
            }
            status_row(ui, "Son", &audio_txt, audio_level);
            let level = if s.compute == Compute::Cpu { Level::Warn } else { Level::Ok };
            status_row(ui, "Rendu", &info.gpu, level);
        });
        if video.is_none() {
            ui.label(RichText::new("Branchez la carte sur un port USB 3.x et fermez NZXT CAM, OBS et Discord.").size(13.0).color(WARN));
        }
        ui.add_space(8.0);

        // ----- Traitement de l'image -----
        ui.label(RichText::new("Traitement de l'image").font(titre(16.0)));
        let items: Vec<(Compute, &str)> = Compute::ALL.iter().map(|c| (*c, c.label())).collect();
        segmented(ui, &mut s.compute, &items);
        if s.compute == Compute::Cpu {
            ui.label(RichText::new("Rendu logiciel, plus lent : les filtres légers sont choisis pour vous.").size(13.0).color(WARN));
        }
        ui.add_space(8.0);

        // ----- Profil -----
        ui.label(RichText::new("Profil d'affichage").font(titre(16.0)));
        let current = Profile::current(s);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for p in Profile::ALL {
                if radio_row(ui, p.name(), current == Some(p)).clicked() {
                    p.apply(s);
                }
            }
        });
        match current {
            Some(p) => ui.label(muted(p.description(s.compute))),
            None => ui.label(muted("Réglages personnalisés. Choisissez un profil pour les remplacer.")),
        };
        ui.add_space(8.0);

        // ----- Actions -----
        if primary_button(ui, "Lancer le direct").clicked() {
            actions.push(Action::StartLive);
        }
        if settings_button(ui, "Réglages", "Tab").clicked() {
            *open = true;
        }
        ui.add_space(2.0);
        ui.checkbox(&mut s.skip_home, "Lancer directement au démarrage");
    });
}

#[allow(clippy::too_many_arguments)]
fn settings_page(
    ui: &mut egui::Ui,
    open: &mut bool,
    home: bool,
    tab: &mut Tab,
    video_list: &[String],
    audio_list: &[String],
    s: &mut Settings,
    info: &Info,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Réglages").font(titre(24.0)).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if link(ui, if home { "Retour" } else { "Fermer" }, ACCENT).clicked() {
                *open = false;
            }
        });
    });
    ui.add_space(4.0);
    tabs(ui, tab);

    let w = ui.available_width();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(w);
        ui.spacing_mut().slider_width = (w - 76.0).max(80.0);
        ui.spacing_mut().combo_width = w;
        match *tab {
            Tab::Image => {
                field(ui, "Traitement de l'image", |ui| {
                    let items: Vec<(Compute, &str)> = Compute::ALL.iter().map(|c| (*c, c.label())).collect();
                    segmented(ui, &mut s.compute, &items);
                });
                if s.compute == Compute::Cpu {
                    ui.label(RichText::new("Rendu par le processeur : préférez Bilinéaire ou Bicubique. Lanczos et la 4K sont lents.").size(13.0).color(WARN));
                    ui.add_space(6.0);
                }
                field(ui, "Filtre d'agrandissement", |ui| {
                    egui::ComboBox::from_id_salt("filtre").selected_text(s.filter.menu_label()).width(w).truncate().show_ui(ui, |ui| {
                        for f in Filter::ALL {
                            ui.selectable_value(&mut s.filter, f, f.menu_label());
                        }
                    });
                });
                field(ui, "Netteté (filtre bicubique)", |ui| {
                    let mut sp = s.sharpen * 100.0;
                    ui.add_enabled(s.filter == Filter::Bicubic, egui::Slider::new(&mut sp, 0.0..=100.0).suffix(" %"));
                    s.sharpen = sp / 100.0;
                });
                field(ui, "Affichage à l'écran", |ui| {
                    egui::ComboBox::from_id_salt("present").selected_text(s.present.menu_label()).width(w).truncate().show_ui(ui, |ui| {
                        for p in Present::ALL {
                            ui.selectable_value(&mut s.present, p, p.menu_label());
                        }
                    });
                });
                ui.checkbox(&mut s.fullscreen, "Plein écran (F, F11 ou Alt+Entrée)");
                ui.checkbox(&mut s.show_info, "Pastille d'infos sur l'image (I)");
                ui.add_space(6.0);
                field(ui, "Taille de l'interface", |ui| {
                    ui.add(egui::Slider::new(&mut s.ui_scale, 0.8..=2.0));
                });
            }
            Tab::Sound => {
                ui.checkbox(&mut s.audio_on, "Son de la carte");
                ui.add_space(6.0);
                field(ui, "Entrée audio", |ui| {
                    let txt = s.audio_name.clone().unwrap_or_else(|| "Auto (HD60, NZXT, Signal)".into());
                    ui.add_enabled_ui(s.audio_on, |ui| {
                        egui::ComboBox::from_id_salt("audio").selected_text(txt).width(w).truncate().show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.audio_name, None, "Auto (HD60, NZXT, Signal)");
                            for n in audio_list {
                                ui.selectable_value(&mut s.audio_name, Some(n.clone()), n);
                            }
                        });
                    });
                });
                field(ui, "Volume", |ui| {
                    let mut v = s.volume * 100.0;
                    ui.add(egui::Slider::new(&mut v, 0.0..=150.0).suffix(" %"));
                    s.volume = v / 100.0;
                });
                ui.checkbox(&mut s.muted, "Sourdine (M)");
                if home {
                    ui.add_space(6.0);
                    ui.label(muted("Le son démarre avec le direct."));
                } else if !info.audio_status.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(&info.audio_status).size(13.0).color(WARN));
                }
            }
            Tab::Capture => {
                field(ui, "Carte de capture", |ui| {
                    let txt = s.video_name.clone().unwrap_or_else(|| "Auto (HD60, NZXT, Signal)".into());
                    egui::ComboBox::from_id_salt("video").selected_text(txt).width(w).truncate().show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.video_name, None, "Auto (HD60, NZXT, Signal)");
                        for n in video_list {
                            ui.selectable_value(&mut s.video_name, Some(n.clone()), n);
                        }
                    });
                });
                field(ui, "Résolution", |ui| {
                    egui::ComboBox::from_id_salt("res").selected_text(format!("{}×{}", s.width, s.height)).width(w).truncate().show_ui(ui, |ui| {
                        for (rw, rh) in RESOLUTIONS {
                            if ui.selectable_label(s.width == rw && s.height == rh, format!("{rw}×{rh}")).clicked() {
                                s.width = rw;
                                s.height = rh;
                            }
                        }
                    });
                });
                field(ui, "Images par seconde", |ui| {
                    egui::ComboBox::from_id_salt("fps").selected_text(format!("{}", s.fps)).width(w).truncate().show_ui(ui, |ui| {
                        for f in FRAMERATES {
                            ui.selectable_value(&mut s.fps, f, format!("{f}"));
                        }
                    });
                });
                field(ui, "Format", |ui| {
                    egui::ComboBox::from_id_salt("format").selected_text(s.format.label()).width(w).truncate().show_ui(ui, |ui| {
                        for f in Format::ALL {
                            ui.selectable_value(&mut s.format, f, f.label());
                        }
                    });
                });
                if info.capture_dirty {
                    if primary_button(ui, "Appliquer la capture").clicked() {
                        actions.push(Action::ApplyCapture);
                    }
                    ui.label(RichText::new("Modifications pas encore appliquées.").size(13.0).color(WARN));
                } else if ui.add_sized([w, 38.0], egui::Button::new("Relancer la capture")).clicked() {
                    actions.push(Action::ApplyCapture);
                }
                if link(ui, "Actualiser les périphériques", ACCENT).clicked() {
                    actions.push(Action::RefreshDevices);
                }
                ui.add_space(4.0);
                if let Some(e) = &info.error {
                    ui.label(RichText::new(e).size(13.0).color(BAD));
                } else if !info.stream.is_empty() {
                    ui.label(muted(format!("Flux actuel : {} à {:.0} i/s", info.stream, info.fps)));
                }
            }
            Tab::General => {
                ui.checkbox(&mut s.skip_home, "Lancer directement au démarrage");
                ui.add_space(8.0);
                if ui.add_sized([w, 38.0], egui::Button::new("Enregistrer les réglages")).clicked() {
                    actions.push(Action::Save);
                }
                if ui.add_sized([w, 38.0], egui::Button::new("Valeurs par défaut")).clicked() {
                    actions.push(Action::Reset);
                }
                if !home && ui.add_sized([w, 38.0], egui::Button::new("Retour à l'accueil (H)")).clicked() {
                    actions.push(Action::GoHome);
                }
                if ui.add_sized([w, 38.0], egui::Button::new("Quitter l'application")).clicked() {
                    actions.push(Action::Quit);
                }
                ui.add_space(12.0);
                ui.label(muted(format!("Rendu : {}", info.gpu)));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(has_frame: bool, error: Option<&str>) -> Info {
        Info {
            has_frame,
            error: error.map(String::from),
            status: "Ouverture de la carte…".into(),
            stream: "1920x1080 YUYV".into(),
            fps: 59.9,
            gpu: "Test GPU".into(),
            present_label: "Mailbox".into(),
            filter_label: "Lanczos".into(),
            audio_status: "Son désactivé : test".into(),
            capture_dirty: true,
            message: "Réglages enregistrés".into(),
        }
    }

    /// Pixels d'une image dessinée hors écran : octets BGRA, `row` octets par ligne.
    struct Shot {
        data: Vec<u8>,
        row: u32,
        w: u32,
        h: u32,
    }

    impl Shot {
        /// Nombre de pixels de la zone qui vérifient `pred(b, g, r)`.
        fn count(&self, x0: u32, y0: u32, x1: u32, y1: u32, pred: impl Fn(u8, u8, u8) -> bool) -> usize {
            let mut n = 0;
            for y in y0..y1.min(self.h) {
                for x in x0..x1.min(self.w) {
                    let i = (y * self.row + x * 4) as usize;
                    if pred(self.data[i], self.data[i + 1], self.data[i + 2]) {
                        n += 1;
                    }
                }
            }
            n
        }
    }

    /// Dessine l'interface dans une texture (GPU logiciel). `None` sans pilote graphique.
    #[allow(clippy::too_many_arguments)]
    fn render(home: bool, open: bool, tab: Tab, has_frame: bool, video: &[String], audio: &[String], dump: &str) -> Option<Shot> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let Some(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
            eprintln!("(pas de GPU/pilote Vulkan disponible : test ignoré)");
            return None;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).unwrap();
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let mut renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);
        let ctx = Context::default();
        setup(&ctx);
        let (w, h) = (1280u32, 720u32); // taille par défaut de la fenêtre, en pixels
        let mut settings = Settings::default();
        let zoom = zoom_for(settings.ui_scale, h);
        ctx.set_zoom_factor(zoom);
        let mut open = open;
        let mut tab = tab;
        let mut actions = Vec::new();

        let mut last = None;
        for _ in 0..3 {
            let raw = egui::RawInput {
                // L'écran d'egui est en points : pixels / facteur d'agrandissement.
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32 / zoom, h as f32 / zoom))),
                ..Default::default()
            };
            let out = ctx.run(raw, |ctx| {
                draw(ctx, &mut open, home, &mut tab, 1.0, video, audio, &mut settings, &info(has_frame, Some("Erreur de test")), &mut actions)
            });
            for (id, delta) in &out.textures_delta.set {
                renderer.update_texture(&device, &queue, *id, delta);
            }
            last = Some(out);
        }
        let out = last.unwrap();
        assert!(actions.is_empty(), "aucune action sans clic");

        let ppp = out.pixels_per_point;
        let jobs = ctx.tessellate(out.shapes, ppp);
        assert!(!jobs.is_empty(), "rien à dessiner");

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: ppp };
        let mut enc = device.create_command_encoder(&Default::default());
        let cmds = renderer.update_buffers(&device, &queue, &mut enc, &jobs, &sd);
        {
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut pass = pass.forget_lifetime();
            renderer.render(&mut pass, &jobs, &sd);
        }
        let row = (w * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(cmds.into_iter().chain(Some(enc.finish())));
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);
        let data = slice.get_mapped_range().to_vec();
        let shot = Shot { data, row, w, h };
        if let Ok(path) = std::env::var(dump) {
            let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
            for y in 0..h {
                for x in 0..w {
                    let i = (y * row + x * 4) as usize;
                    ppm.extend_from_slice(&[shot.data[i + 2], shot.data[i + 1], shot.data[i]]); // BGRA -> RGB
                }
            }
            std::fs::write(path, ppm).unwrap();
        }
        Some(shot)
    }

    fn devices() -> (Vec<String>, Vec<String>) {
        (
            vec!["NZXT Signal HD60 Video".to_string(), "Webcam".to_string()],
            vec!["HDMI (NZXT Signal HD60 Audio)".to_string()],
        )
    }

    // Zone du panneau : les 344 derniers points de la fenêtre.
    fn panel_x(w: u32) -> u32 {
        w - (PANEL_W * zoom_for(Settings::default().ui_scale, 720)) as u32
    }

    #[test]
    fn accueil_dessine_panneau_clair_et_mire() {
        let (v, a) = devices();
        let Some(shot) = render(true, false, Tab::Image, false, &v, &a, "HOME_DUMP") else { return };
        let px = panel_x(shot.w);
        // Panneau : fond clair, texte foncé et bouton bleu.
        let light = shot.count(px + 4, 0, shot.w, shot.h, |b, g, r| b > 235 && g > 235 && r > 235);
        let dark = shot.count(px + 4, 0, shot.w, shot.h, |b, g, r| b < 90 && g < 90 && r < 90);
        let blue = shot.count(px + 4, 0, shot.w, shot.h, |b, g, r| b > 150 && r < 80 && g < 100);
        // Image : la mire de barres, colorée, remplit la zone à gauche du panneau.
        let colorful = shot.count(0, 0, px.saturating_sub(4), shot.h, |b, g, r| b.max(g).max(r) - b.min(g).min(r) > 120);
        eprintln!("accueil : panneau clair {light}, texte {dark}, bouton {blue}, mire {colorful}");
        assert!(light > 60_000, "le panneau doit être clair ({light})");
        assert!(dark > 1_500, "le panneau doit afficher du texte foncé ({dark})");
        assert!(blue > 5_000, "le bouton Lancer le direct doit être bleu ({blue})");
        assert!(colorful > 60_000, "la mire de barres doit remplir l'image ({colorful})");
    }

    #[test]
    fn reglages_dessine_les_onglets() {
        let (v, a) = devices();
        for tab in Tab::ALL {
            let Some(shot) = render(false, true, tab, true, &v, &a, "MENU_DUMP") else { return };
            let px = panel_x(shot.w);
            let dark = shot.count(px + 4, 0, shot.w, shot.h, |b, g, r| b < 90 && g < 90 && r < 90);
            eprintln!("réglages {} : texte {dark}", tab.label());
            assert!(dark > 800, "l'onglet {} doit afficher du contenu ({dark})", tab.label());
        }
    }

    #[test]
    fn direct_sans_panneau_ne_dessine_que_les_pastilles() {
        let ctx = Context::default();
        setup(&ctx);
        let mut settings = Settings::default();
        let mut open = false;
        let mut tab = Tab::Image;
        let mut actions = Vec::new();
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| draw(ctx, &mut open, false, &mut tab, 0.0, &[], &[], &mut settings, &info(true, None), &mut actions));
        }
        assert!(!open);
        assert!(actions.is_empty());
    }

    #[test]
    fn raccourcis_suivent_les_touches_actives() {
        let keys = |home, open| shortcut_items(home, open).iter().map(|(k, _)| *k).collect::<Vec<_>>();
        // Accueil : Entrée lance le direct ; M, volume, I et H ne font rien tant que le direct n'a pas commencé.
        assert_eq!(keys(true, false), ["Entrée", "Tab", "F", "Échap"]);
        // Direct : toutes les touches de jeu, et Échap revient à l'accueil.
        let live = shortcut_items(false, false);
        assert!(live.iter().any(|(k, w)| *k == "H" && *w == "Accueil"));
        assert!(live.iter().any(|(k, _)| *k == "M") && live.iter().any(|(k, _)| *k == "↑ ↓"));
        assert!(live.last() == Some(&("Échap", "Accueil")));
        // Réglages ouverts : Tab et Échap ferment le panneau.
        assert!(shortcut_items(true, true).iter().any(|(k, w)| *k == "Tab" && w.starts_with("Fermer")));
        assert!(shortcut_items(true, true).last() == Some(&("Échap", "Fermer")));
    }

    #[test]
    fn panneau_sans_materiel_ni_place_ne_plante_pas() {
        let ctx = Context::default();
        setup(&ctx);
        let mut settings = Settings::default();
        let mut open = false;
        let mut tab = Tab::Image;
        let mut actions = Vec::new();
        for (home, o, t) in [(true, false, Tab::Image), (false, true, Tab::Capture), (true, true, Tab::General), (false, true, Tab::Sound)] {
            for size in [(1280.0, 800.0), (320.0, 240.0), (700.0, 420.0)] {
                open = o;
                tab = t;
                for _ in 0..2 {
                    let raw = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.0, size.1))),
                        ..Default::default()
                    };
                    let _ = ctx.run(raw, |ctx| draw(ctx, &mut open, home, &mut tab, 0.6, &[], &[], &mut settings, &info(false, None), &mut actions));
                }
            }
        }
        assert!(actions.is_empty());
        let _ = (open, tab);
    }
}
