//! Son de la carte de capture -> sortie audio par défaut, avec un tampon très court.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};

/// Réglages partagés avec la fenêtre (volume, sourdine).
pub struct AudioControl {
    volume_bits: AtomicU32,
    pub muted: AtomicBool,
}

impl AudioControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { volume_bits: AtomicU32::new(1.0f32.to_bits()), muted: AtomicBool::new(false) })
    }
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume_bits.load(Ordering::Relaxed))
    }
    pub fn set_volume(&self, v: f32) {
        self.volume_bits.store(v.clamp(0.0, 1.5).to_bits(), Ordering::Relaxed);
    }
}

pub fn list_inputs() -> Vec<(usize, String)> {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(it) => it.enumerate().map(|(i, d)| (i, d.name().unwrap_or_else(|_| "?".into()))).collect(),
        Err(_) => Vec::new(),
    }
}

fn is_card_name(n: &str) -> bool {
    let n = n.to_lowercase();
    n.contains("hd60") || n.contains("nzxt") || n.contains("signal")
}

fn is_loose_card_name(n: &str) -> bool {
    let n = n.to_lowercase();
    n.contains("capture") || n.contains("digital audio") || n.contains("hdmi")
}

/// Entrée que le mode « Auto » choisirait parmi ces noms (affichée sur l'écran d'accueil).
pub fn guess_input(names: &[String]) -> Option<&String> {
    names.iter().find(|n| is_card_name(n)).or_else(|| names.iter().find(|n| is_loose_card_name(n)))
}

fn pick_input(index: Option<usize>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    let devs: Vec<cpal::Device> = host.input_devices().ok()?.collect();
    if let Some(i) = index {
        return devs.into_iter().nth(i);
    }
    let mut loose_match = None;
    for d in devs {
        let name = d.name().unwrap_or_default();
        if is_card_name(&name) {
            return Some(d);
        }
        if loose_match.is_none() && is_loose_card_name(&name) {
            loose_match = Some(d);
        }
    }
    loose_match
}

fn build_input<T>(
    dev: &cpal::Device,
    cfg: &StreamConfig,
    q: Arc<Mutex<VecDeque<f32>>>,
    max: usize,
) -> Result<Stream, cpal::BuildStreamError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    dev.build_input_stream(
        cfg,
        move |data: &[T], _| {
            let mut g = q.lock().unwrap();
            g.extend(data.iter().map(|&s| <f32 as FromSample<T>>::from_sample_(s)));
            // Si on prend du retard, on jette le plus ancien : la latence reste bornée.
            let len = g.len();
            if len > max {
                g.drain(..len - max);
            }
        },
        |e| eprintln!("Audio (entrée) : {e}"),
        None,
    )
}

/// Démarre l'entrée et la sortie. Les flux restent actifs tant que la valeur retournée est gardée.
pub fn start(index: Option<usize>, ctl: Arc<AudioControl>) -> Result<(Stream, Stream), String> {
    let host = cpal::default_host();
    let in_dev = pick_input(index).ok_or_else(|| {
        "aucune entrée audio de la carte trouvée (utilisez --list puis --audio N)".to_string()
    })?;
    let out_dev = host.default_output_device().ok_or("aucune sortie audio par défaut")?;
    eprintln!(
        "Audio : {} -> {}",
        in_dev.name().unwrap_or_default(),
        out_dev.name().unwrap_or_default()
    );

    let in_def = in_dev.default_input_config().map_err(|e| format!("entrée : {e}"))?;
    let in_fmt = in_def.sample_format();
    let in_cfg_base: StreamConfig = in_def.config();
    let in_ch = in_cfg_base.channels as usize;
    let in_rate = in_cfg_base.sample_rate.0;

    // Sortie : on cherche du f32 stéréo à la même fréquence, sinon la config par défaut.
    let mut out_cfg: Option<StreamConfig> = None;
    if let Ok(cfgs) = out_dev.supported_output_configs() {
        for c in cfgs {
            if c.sample_format() == SampleFormat::F32
                && c.channels() >= 2
                && c.min_sample_rate().0 <= in_rate
                && c.max_sample_rate().0 >= in_rate
            {
                out_cfg = Some(c.with_sample_rate(cpal::SampleRate(in_rate)).config());
                break;
            }
        }
    }
    let out_cfg_base = match out_cfg {
        Some(c) => c,
        None => {
            let d = out_dev.default_output_config().map_err(|e| format!("sortie : {e}"))?;
            if d.sample_format() != SampleFormat::F32 {
                return Err("la sortie audio n'accepte pas le format f32".into());
            }
            d.config()
        }
    };
    let out_ch = out_cfg_base.channels as usize;
    let out_rate = out_cfg_base.sample_rate.0;

    // Tampon : cible ~25 ms, maximum ~60 ms.
    let target = (in_rate as usize * in_ch) * 25 / 1000;
    let max = (in_rate as usize * in_ch) * 60 / 1000;
    let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(max * 2)));

    // Entrée : on essaie un petit tampon matériel, puis la valeur par défaut.
    let mut in_stream = None;
    for buf in [BufferSize::Fixed(256), BufferSize::Default] {
        let mut cfg = in_cfg_base.clone();
        cfg.buffer_size = buf;
        let r = match in_fmt {
            SampleFormat::F32 => build_input::<f32>(&in_dev, &cfg, queue.clone(), max),
            SampleFormat::I16 => build_input::<i16>(&in_dev, &cfg, queue.clone(), max),
            SampleFormat::U16 => build_input::<u16>(&in_dev, &cfg, queue.clone(), max),
            SampleFormat::I32 => build_input::<i32>(&in_dev, &cfg, queue.clone(), max),
            other => return Err(format!("format d'entrée non géré : {other:?}")),
        };
        if let Ok(s) = r {
            in_stream = Some(s);
            break;
        }
    }
    let in_stream = in_stream.ok_or("impossible d'ouvrir l'entrée audio")?;

    // Sortie : on essaie un petit tampon matériel, puis la valeur par défaut.
    let mut out_stream = None;
    for buf in [BufferSize::Fixed(256), BufferSize::Default] {
        let mut cfg = out_cfg_base.clone();
        cfg.buffer_size = buf;
        let r = out_dev.build_output_stream(
            &cfg,
            make_output_callback(queue.clone(), ctl.clone(), in_ch, out_ch, in_rate, out_rate, target),
            |e| eprintln!("Audio (sortie) : {e}"),
            None,
        );
        if let Ok(st) = r {
            out_stream = Some(st);
            break;
        }
    }
    let out_stream = out_stream.ok_or("impossible d'ouvrir la sortie audio")?;

    in_stream.play().map_err(|e| format!("lecture entrée : {e}"))?;
    out_stream.play().map_err(|e| format!("lecture sortie : {e}"))?;
    Ok((in_stream, out_stream))
}

/// Fonction appelée par la carte son : lit la file, rééchantillonne (linéaire) et applique le volume.
fn make_output_callback(
    q: Arc<Mutex<VecDeque<f32>>>,
    ctl: Arc<AudioControl>,
    in_ch: usize,
    out_ch: usize,
    in_rate: u32,
    out_rate: u32,
    target: usize,
) -> impl FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + 'static {
    let step = in_rate as f64 / out_rate as f64;
    let mut primed = false;
    let mut pos = 0.0f64;
    let mut a = vec![0.0f32; in_ch];
    let mut b = vec![0.0f32; in_ch];
    let mut have = false;

    move |data: &mut [f32], _| {
        let vol = if ctl.muted.load(Ordering::Relaxed) { 0.0 } else { ctl.volume() };
        let mut g = q.lock().unwrap();
        if !primed {
            if g.len() >= target {
                primed = true;
            } else {
                data.fill(0.0);
                return;
            }
        }
        for frame in data.chunks_mut(out_ch) {
            if !have {
                if g.len() < in_ch * 2 {
                    primed = false; // sous-alimenté : on remplit à nouveau avant de reprendre
                    frame.fill(0.0);
                    continue;
                }
                for c in 0..in_ch { a[c] = g.pop_front().unwrap(); }
                for c in 0..in_ch { b[c] = g.pop_front().unwrap(); }
                have = true;
            }
            let t = pos as f32;
            let sample = |c: usize| a[c] + (b[c] - a[c]) * t;
            if out_ch == 1 {
                let mix: f32 = (0..in_ch).map(sample).sum::<f32>() / in_ch as f32;
                frame[0] = mix * vol;
            } else {
                for (o, slot) in frame.iter_mut().enumerate() {
                    let c = if in_ch == 1 { 0 } else { o.min(in_ch - 1) };
                    *slot = sample(c) * vol;
                }
            }
            pos += step;
            while pos >= 1.0 {
                pos -= 1.0;
                std::mem::swap(&mut a, &mut b);
                if g.len() >= in_ch {
                    for c in 0..in_ch { b[c] = g.pop_front().unwrap(); }
                } else {
                    have = false;
                    break;
                }
            }
        }
    }
}
