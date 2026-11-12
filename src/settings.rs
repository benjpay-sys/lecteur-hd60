//! Réglages modifiables depuis le menu, avec sauvegarde dans un petit fichier texte.
//! Emplacement : %APPDATA%\lecteur-hd60\reglages.txt (Windows) ou ~/.config/lecteur-hd60/reglages.txt.
//! Ce fichier est facultatif : sans lui, le programme utilise les valeurs par défaut.

use crate::gpu::{Compute, Filter, Present};
use nokhwa::utils::FrameFormat;
use std::{env, fs, path::PathBuf};

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Auto,
    Yuyv,
    Nv12,
    Mjpeg,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Auto, Format::Yuyv, Format::Nv12, Format::Mjpeg];
    pub fn label(self) -> &'static str {
        match self {
            Format::Auto => "Auto (formats bruts d'abord)",
            Format::Yuyv => "YUYV (brut)",
            Format::Nv12 => "NV12 (brut)",
            Format::Mjpeg => "MJPEG (compressé)",
        }
    }
    pub fn frame_format(self) -> Option<FrameFormat> {
        match self {
            Format::Auto => None,
            Format::Yuyv => Some(FrameFormat::YUYV),
            Format::Nv12 => Some(FrameFormat::NV12),
            Format::Mjpeg => Some(FrameFormat::MJPEG),
        }
    }
    fn key(self) -> &'static str {
        match self {
            Format::Auto => "auto",
            Format::Yuyv => "yuyv",
            Format::Nv12 => "nv12",
            Format::Mjpeg => "mjpeg",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.key() == s)
    }
}

/// Profils proposés sur l'écran d'accueil : ils ne règlent que l'affichage (filtre et présentation),
/// jamais la capture ni le son.
#[derive(Clone, Copy, PartialEq)]
pub enum Profile {
    Latency,
    Balanced,
    Quality,
}

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::Latency, Profile::Balanced, Profile::Quality];

    pub fn name(self) -> &'static str {
        match self {
            Profile::Latency => "Latence minimale",
            Profile::Balanced => "Équilibré",
            Profile::Quality => "Image soignée",
        }
    }

    pub fn description(self, compute: Compute) -> &'static str {
        match (self, compute) {
            (Profile::Latency, _) => "Affichage immédiat et filtre léger. Un léger déchirement de l'image est possible.",
            (Profile::Balanced, Compute::Gpu) => "Sans déchirement, agrandissement Lanczos. Recommandé pour jouer.",
            (Profile::Balanced, Compute::Cpu) => "Sans déchirement, filtre bilinéaire : le plus léger pour le processeur.",
            (Profile::Quality, Compute::Gpu) => "Lanczos synchronisé avec l'écran. Image la plus propre, un peu plus de retard.",
            (Profile::Quality, Compute::Cpu) => "Filtre bicubique synchronisé avec l'écran. Plus net, mais charge le processeur.",
        }
    }

    /// Filtre et mode d'affichage du profil. Sur le processeur (rendu logiciel), les filtres coûteux sont évités.
    fn values(self, compute: Compute) -> (Filter, Present) {
        match (self, compute) {
            (Profile::Latency, _) => (Filter::Bilinear, Present::Immediate),
            (Profile::Balanced, Compute::Gpu) => (Filter::Auto, Present::Auto),
            (Profile::Balanced, Compute::Cpu) => (Filter::Bilinear, Present::Auto),
            (Profile::Quality, Compute::Gpu) => (Filter::Lanczos, Present::Vsync),
            (Profile::Quality, Compute::Cpu) => (Filter::Bicubic, Present::Vsync),
        }
    }

    pub fn apply(self, s: &mut Settings) {
        (s.filter, s.present) = self.values(s.compute);
    }

    /// Profil qui correspond exactement aux réglages actuels, s'il y en a un.
    pub fn current(s: &Settings) -> Option<Profile> {
        Self::ALL.into_iter().find(|p| p.values(s.compute) == (s.filter, s.present))
    }
}

#[derive(Clone, PartialEq)]
pub struct Settings {
    /// Démarre directement le direct, sans passer par l'écran d'accueil.
    pub skip_home: bool,
    // Image (appliqué tout de suite)
    /// Matériel qui fait l'agrandissement et l'affichage.
    pub compute: Compute,
    pub filter: Filter,
    pub sharpen: f32,
    pub present: Present,
    pub fullscreen: bool,
    pub show_info: bool,
    pub ui_scale: f32,
    // Son
    pub audio_on: bool,
    pub audio_name: Option<String>,
    pub volume: f32,
    pub muted: bool,
    // Capture (appliqué avec le bouton « Appliquer »)
    pub video_name: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: Format,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            skip_home: false,
            compute: Compute::Gpu,
            filter: Filter::Auto,
            sharpen: 0.35,
            present: Present::Auto,
            fullscreen: false,
            show_info: true,
            ui_scale: 1.25,
            audio_on: true,
            audio_name: None,
            volume: 1.0,
            muted: false,
            video_name: None,
            width: 1920,
            height: 1080,
            fps: 60,
            format: Format::Auto,
        }
    }
}

fn filter_key(f: Filter) -> &'static str {
    match f {
        Filter::Auto => "auto",
        Filter::Nearest => "nearest",
        Filter::Bilinear => "bilinear",
        Filter::Bicubic => "bicubic",
        Filter::Lanczos => "lanczos",
    }
}

fn present_key(p: Present) -> &'static str {
    match p {
        Present::Auto => "auto",
        Present::Vsync => "vsync",
        Present::Immediate => "immediate",
    }
}

fn compute_key(c: Compute) -> &'static str {
    match c {
        Compute::Gpu => "gpu",
        Compute::Cpu => "cpu",
    }
}

pub fn parse_compute(s: &str) -> Option<Compute> {
    Compute::ALL.into_iter().find(|c| compute_key(*c) == s)
}

pub fn parse_filter(s: &str) -> Option<Filter> {
    [Filter::Auto, Filter::Nearest, Filter::Bilinear, Filter::Bicubic, Filter::Lanczos]
        .into_iter()
        .find(|f| filter_key(*f) == s)
}

pub fn parse_present(s: &str) -> Option<Present> {
    [Present::Auto, Present::Vsync, Present::Immediate].into_iter().find(|p| present_key(*p) == s)
}

pub fn parse_format(s: &str) -> Option<Format> {
    Format::parse(s)
}

fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        PathBuf::from(env::var_os("APPDATA")?)
    } else if let Some(x) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(x)
    } else {
        PathBuf::from(env::var_os("HOME")?).join(".config")
    };
    Some(base.join("lecteur-hd60").join("reglages.txt"))
}

impl Settings {
    /// Appelé après un changement de matériel (`compute`, ancienne valeur `from`) : si les réglages d'affichage
    /// correspondaient à un profil, on garde le même profil pour le nouveau matériel ; sinon on évite seulement
    /// les filtres trop lourds pour le processeur.
    pub fn adapt_compute(&mut self, from: Compute) {
        let mut old = self.clone();
        old.compute = from;
        match Profile::current(&old) {
            Some(p) => p.apply(self),
            None if self.compute == Compute::Cpu && matches!(self.filter, Filter::Auto | Filter::Lanczos) => {
                self.filter = Filter::Bilinear;
            }
            None => {}
        }
    }

    /// Lit le fichier de réglages ; toute valeur absente ou invalide garde sa valeur par défaut.
    pub fn load() -> Self {
        let mut s = Self::default();
        let Some(text) = path().and_then(|p| fs::read_to_string(p).ok()) else { return s };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let flag = |v: &str| v == "1" || v == "true";
            match k {
                "skip_home" => s.skip_home = flag(v),
                "compute" => s.compute = parse_compute(v).unwrap_or(s.compute),
                "filter" => s.filter = parse_filter(v).unwrap_or(s.filter),
                "sharpen" => s.sharpen = v.parse().map(|x: f32| x.clamp(0.0, 1.0)).unwrap_or(s.sharpen),
                "present" => s.present = parse_present(v).unwrap_or(s.present),
                "fullscreen" => s.fullscreen = flag(v),
                "show_info" => s.show_info = flag(v),
                "ui_scale" => s.ui_scale = v.parse().map(|x: f32| x.clamp(0.8, 2.5)).unwrap_or(s.ui_scale),
                "audio_on" => s.audio_on = flag(v),
                "audio_name" => s.audio_name = (!v.is_empty()).then(|| v.to_string()),
                "volume" => s.volume = v.parse().map(|x: f32| x.clamp(0.0, 1.5)).unwrap_or(s.volume),
                "muted" => s.muted = flag(v),
                "video_name" => s.video_name = (!v.is_empty()).then(|| v.to_string()),
                "width" => s.width = v.parse().map(|x: u32| x.clamp(160, 7680)).unwrap_or(s.width),
                "height" => s.height = v.parse().map(|x: u32| x.clamp(120, 4320)).unwrap_or(s.height),
                "fps" => s.fps = v.parse().map(|x: u32| x.clamp(1, 480)).unwrap_or(s.fps),
                "format" => s.format = parse_format(v).unwrap_or(s.format),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let p = path().ok_or("dossier de configuration introuvable")?;
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let b = |x: bool| if x { "1" } else { "0" };
        let text = format!(
            "skip_home={}\ncompute={}\nfilter={}\nsharpen={}\npresent={}\nfullscreen={}\nshow_info={}\nui_scale={}\n\
             audio_on={}\naudio_name={}\nvolume={}\nmuted={}\n\
             video_name={}\nwidth={}\nheight={}\nfps={}\nformat={}\n",
            b(self.skip_home), compute_key(self.compute), filter_key(self.filter), self.sharpen, present_key(self.present), b(self.fullscreen),
            b(self.show_info), self.ui_scale, b(self.audio_on),
            self.audio_name.clone().unwrap_or_default(), self.volume, b(self.muted),
            self.video_name.clone().unwrap_or_default(), self.width, self.height, self.fps,
            self.format.key()
        );
        fs::write(&p, text).map_err(|e| e.to_string())?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changement_de_materiel_garde_le_profil() {
        // Profil « Équilibré » sur carte graphique -> bascule sur le processeur : filtre allégé, même profil.
        let mut s = Settings::default();
        assert!(Profile::current(&s) == Some(Profile::Balanced));
        s.compute = Compute::Cpu;
        s.adapt_compute(Compute::Gpu);
        assert!(s.filter == Filter::Bilinear && Profile::current(&s) == Some(Profile::Balanced));
        // Retour sur la carte graphique : Lanczos revient.
        s.compute = Compute::Gpu;
        s.adapt_compute(Compute::Cpu);
        assert!(s.filter == Filter::Auto && Profile::current(&s) == Some(Profile::Balanced));
        // Réglages personnalisés : seul le filtre trop lourd est remplacé.
        let mut p = Settings { filter: Filter::Lanczos, present: Present::Immediate, ..Settings::default() };
        p.compute = Compute::Cpu;
        p.adapt_compute(Compute::Gpu);
        assert!(p.filter == Filter::Bilinear && p.present == Present::Immediate);
    }

    #[test]
    fn enregistrement_et_relecture() {
        let dir = std::env::temp_dir().join(format!("lecteur-hd60-test-{}", std::process::id()));
        // SAFETY : un seul test touche à l'environnement.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &dir);
        }
        if cfg!(windows) {
            return; // le chemin dépend de %APPDATA% sous Windows
        }
        let s = Settings {
            skip_home: true,
            compute: Compute::Cpu,
            filter: Filter::Bicubic,
            sharpen: 0.6,
            present: Present::Vsync,
            fullscreen: true,
            show_info: false,
            ui_scale: 1.5,
            audio_on: true,
            audio_name: Some("HDMI (NZXT Signal HD60 Audio)".into()),
            volume: 0.8,
            muted: true,
            video_name: Some("NZXT Signal HD60 Video".into()),
            width: 2560,
            height: 1440,
            fps: 120,
            format: Format::Nv12,
        };
        let p = s.save().expect("sauvegarde");
        assert!(p.exists());
        let t = Settings::load();
        assert!(s == t, "les réglages relus doivent être identiques");
        // Fichier abîmé : les valeurs valides sont lues, le reste garde sa valeur par défaut.
        std::fs::write(&p, "filter=nimportequoi\nfps=abc\nvolume=0.3\nligne sans egal\n").unwrap();
        let d = Settings::load();
        assert!(d.filter == Filter::Auto && d.fps == 60 && (d.volume - 0.3).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(dir);
    }
}
