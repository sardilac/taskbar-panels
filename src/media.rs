//! Multimedia: sesión actual de Windows (WinRT GSMTC) y analizador de espectro
//! (WASAPI loopback + FFT de 4096 puntos, 64 bandas logarítmicas).

use crate::state::st;
use rustfft::{num_complex::Complex, FftPlanner};
use std::sync::mpsc::{channel, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use windows::Media::Control::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

pub const BANDS: usize = 64;

/// Carátula del álbum decodificada (ART_SIZE², BGRA premultiplicado) y color dominante
pub struct Art {
    pub key: String,
    pub px: Vec<u32>,
    pub accent: (u8, u8, u8),
}
pub const ART_SIZE: usize = 128;
pub static ART: LazyLock<Mutex<Option<std::sync::Arc<Art>>>> = LazyLock::new(|| Mutex::new(None));

/// Línea de tiempo: posición (s) medida en `at`, duración (s)
#[derive(Clone, Copy, Default)]
pub struct Timeline {
    pub pos: f64,
    pub dur: f64,
    pub at: Option<Instant>,
}
pub static TIMELINE: LazyLock<Mutex<Timeline>> = LazyLock::new(|| Mutex::new(Timeline::default()));

/// Decodifica la miniatura del álbum: recorte centrado a ART_SIZE² y color dominante
fn load_art(props: &GlobalSystemMediaTransportControlsSessionMediaProperties, key: &str) -> Option<Art> {
    use windows::Graphics::Imaging::*;
    let thumb = props.Thumbnail().ok()?;
    let stream = thumb.OpenReadAsync().ok()?.get().ok()?;
    let dec = BitmapDecoder::CreateAsync(&stream).ok()?.get().ok()?;
    let (pw, ph) = (dec.PixelWidth().ok()? as f64, dec.PixelHeight().ok()? as f64);
    if pw < 1.0 || ph < 1.0 {
        return None;
    }
    let k = ART_SIZE as f64 / pw.min(ph); // escala "cover"
    let (sw, sh) = ((pw * k).round() as u32, (ph * k).round() as u32);
    let tr = BitmapTransform::new().ok()?;
    tr.SetScaledWidth(sw).ok()?;
    tr.SetScaledHeight(sh).ok()?;
    tr.SetInterpolationMode(BitmapInterpolationMode::Fant).ok()?;
    tr.SetBounds(BitmapBounds {
        X: (sw - ART_SIZE as u32) / 2,
        Y: (sh - ART_SIZE as u32) / 2,
        Width: ART_SIZE as u32,
        Height: ART_SIZE as u32,
    })
    .ok()?;
    let pd = dec
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &tr,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
        .ok()?
        .get()
        .ok()?;
    let bytes = pd.DetachPixelData().ok()?;
    if bytes.len() < ART_SIZE * ART_SIZE * 4 {
        return None;
    }
    let px: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    // Color dominante: promedio ponderado por saturación, llevado a un tono brillante
    let (mut r, mut g, mut b, mut wsum) = (0f64, 0f64, 0f64, 0f64);
    for p in &px {
        let (pb, pg, pr) = ((p & 0xFF) as f64, ((p >> 8) & 0xFF) as f64, ((p >> 16) & 0xFF) as f64);
        let mx = pr.max(pg).max(pb);
        let mn = pr.min(pg).min(pb);
        let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
        let w = sat * sat * (mx / 255.0) + 0.02;
        r += pr * w;
        g += pg * w;
        b += pb * w;
        wsum += w;
    }
    let (r, g, b) = (r / wsum, g / wsum, b / wsum);
    let mx = r.max(g).max(b).max(1.0);
    let boost = 235.0 / mx; // brillo alto para que funcione como acento sobre la barra oscura
    let acc = |v: f64| (v * boost).clamp(0.0, 255.0) as u8;
    Some(Art { key: key.to_string(), px, accent: (acc(r), acc(g), acc(b)) })
}

fn now_filetime() -> i64 {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    d.as_secs() as i64 * 10_000_000 + d.subsec_nanos() as i64 / 100 + 116_444_736_000_000_000
}
pub static LEVELS: LazyLock<Mutex<Vec<f32>>> = LazyLock::new(|| Mutex::new(vec![0.0; BANDS]));

#[derive(Clone, Copy)]
pub enum Cmd {
    Prev,
    Toggle,
    Next,
}
static CMD: LazyLock<Mutex<Option<Sender<Cmd>>>> = LazyLock::new(|| Mutex::new(None));

pub fn send(c: Cmd) {
    if let Some(tx) = CMD.lock().unwrap().as_ref() {
        let _ = tx.send(c);
    }
}

// ---------------------------------------------------------------- Sesión multimedia
fn poll(mgr: &GlobalSystemMediaTransportControlsSessionManager) {
    let Ok(s) = mgr.GetCurrentSession() else {
        let mut st = st();
        st.title = crate::i18n::tr("Sin reproducción", "Nothing playing").into();
        st.artist.clear();
        st.media_app.clear();
        st.playing = false;
        drop(st);
        *ART.lock().unwrap() = None;
        *TIMELINE.lock().unwrap() = Timeline::default();
        return;
    };
    let app = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
    let app = app.rsplit('!').next().unwrap_or(&app).trim_end_matches(".exe").to_string();
    let props = s.TryGetMediaPropertiesAsync().and_then(|op| op.get()).ok();
    let title = props.as_ref().and_then(|p| p.Title().ok()).map(|h| h.to_string()).filter(|t| !t.is_empty());
    let artist = props
        .as_ref()
        .and_then(|p| p.Artist().ok())
        .map(|h| h.to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| props.as_ref().and_then(|p| p.AlbumArtist().ok()).map(|h| h.to_string()).filter(|t| !t.is_empty()));
    let playing = s
        .GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .map(|p| p == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
        .unwrap_or(false);
    let title = title.unwrap_or_else(|| app.clone());
    let artist = artist.unwrap_or_else(|| app.clone());

    // Carátula: solo se decodifica cuando cambia la pista
    let key = format!("{title}|{artist}|{app}");
    let need = ART.lock().unwrap().as_ref().map(|a| a.key != key).unwrap_or(true);
    if need {
        let art = props.as_ref().and_then(|p| load_art(p, &key)).unwrap_or(Art { key: key.clone(), px: Vec::new(), accent: (0, 230, 255) });
        *ART.lock().unwrap() = Some(std::sync::Arc::new(art));
    }

    // Línea de tiempo (no todas las apps la informan)
    let mut tl = Timeline::default();
    if let Ok(tp) = s.GetTimelineProperties() {
        let start = tp.StartTime().map(|d| d.Duration).unwrap_or(0);
        let end = tp.EndTime().map(|d| d.Duration).unwrap_or(0);
        let mut pos = tp.Position().map(|d| d.Duration).unwrap_or(0) as f64 / 1e7;
        let upd = tp.LastUpdatedTime().map(|d| d.UniversalTime).unwrap_or(0);
        if playing && upd > 0 {
            pos += ((now_filetime() - upd) as f64 / 1e7).max(0.0);
        }
        let dur = (end - start) as f64 / 1e7;
        if dur > 1.0 {
            tl = Timeline { pos: pos.clamp(0.0, dur), dur, at: Some(Instant::now()) };
        }
    }
    *TIMELINE.lock().unwrap() = tl;

    let album = props.as_ref().and_then(|p| p.AlbumTitle().ok()).map(|h| h.to_string()).unwrap_or_default();
    let mut st = st();
    st.media_album = album;
    st.title = title;
    st.artist = artist;
    st.media_app = app;
    st.playing = playing;
}

fn exec(mgr: &GlobalSystemMediaTransportControlsSessionManager, c: Cmd) {
    if let Ok(s) = mgr.GetCurrentSession() {
        let _ = match c {
            Cmd::Prev => s.TrySkipPreviousAsync().and_then(|o| o.get()),
            Cmd::Toggle => s.TryTogglePlayPauseAsync().and_then(|o| o.get()),
            Cmd::Next => s.TrySkipNextAsync().and_then(|o| o.get()),
        };
    }
}

pub fn spawn_media_worker() {
    let (tx, rx) = channel();
    *CMD.lock().unwrap() = Some(tx);
    std::thread::spawn(move || unsafe {
        crate::util::prof("media");
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut mgr = None;
        loop {
            if mgr.is_none() {
                mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|o| o.get()).ok();
            }
            if let Some(m) = &mgr {
                poll(m);
            }
            if let Ok(c) = rx.recv_timeout(Duration::from_millis(1000)) {
                if let Some(m) = &mgr {
                    exec(m, c);
                    poll(m);
                }
            }
        }
    });
}

// ---------------------------------------------------------------- Espectro (WASAPI loopback)
const N: usize = 4096;

struct Capture {
    client: IAudioClient,
    cap: IAudioCaptureClient,
    id: String,
    channels: usize,
    bits: u16,
    float: bool,
    rate: u32,
}

unsafe fn default_device() -> Option<(IMMDevice, String)> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let dev = en.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
    let pid = dev.GetId().ok()?;
    let id = pid.to_string().unwrap_or_default();
    CoTaskMemFree(Some(pid.0 as *const _));
    Some((dev, id))
}

unsafe fn open_capture() -> Option<Capture> {
    let (dev, id) = default_device()?;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).ok()?;
    let fmt = client.GetMixFormat().ok()?;
    let f = &*fmt;
    let (tag, channels, rate, bits) = (f.wFormatTag, f.nChannels as usize, f.nSamplesPerSec, f.wBitsPerSample);
    let float = tag == 3 || (tag == 0xFFFE && bits == 32);
    let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 1_000_000, 0, fmt, None);
    CoTaskMemFree(Some(fmt as *const _));
    r.ok()?;
    let cap: IAudioCaptureClient = client.GetService().ok()?;
    client.Start().ok()?;
    Some(Capture { client, cap, id, channels, bits, float, rate })
}

/// Lee los paquetes pendientes al búfer circular; devuelve cuántas muestras llegaron
unsafe fn drain(c: &Capture, ring: &mut [f32], pos: &mut usize) -> windows::core::Result<usize> {
    let mut total = 0;
    while c.cap.GetNextPacketSize()? > 0 {
        let mut data = std::ptr::null_mut();
        let (mut frames, mut flags) = (0u32, 0u32);
        c.cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
        let silent = flags & 2 != 0;
        let bpf = c.channels * c.bits as usize / 8;
        let used = c.channels.min(2);
        for f in 0..frames as usize {
            let mut s = 0f32;
            if !silent && !data.is_null() {
                for ch in 0..used {
                    let p = data.add(f * bpf + ch * c.bits as usize / 8);
                    s += if c.float {
                        (p as *const f32).read_unaligned()
                    } else if c.bits == 16 {
                        (p as *const i16).read_unaligned() as f32 / 32768.0
                    } else if c.bits == 32 {
                        (p as *const i32).read_unaligned() as f32 / 2_147_483_648.0
                    } else {
                        0.0
                    };
                }
                s /= used as f32;
            }
            ring[*pos] = s;
            *pos = (*pos + 1) % N;
        }
        c.cap.ReleaseBuffer(frames)?;
        total += frames as usize;
    }
    Ok(total)
}

pub fn spawn_audio_worker() {
    std::thread::spawn(|| unsafe {
        crate::util::prof("audio");
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let fft = FftPlanner::<f32>::new().plan_fft_forward(N);
        let window: Vec<f32> = (0..N).map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos())).collect();
        let mut ring = vec![0f32; N];
        let mut pos = 0usize;
        let mut buf = vec![Complex::new(0f32, 0f32); N];
        let mut level = vec![0f32; BANDS];
        let mut cap: Option<Capture> = None;
        let mut last_check = Instant::now();
        let mut last_data = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(33));
            // Cada ~2 s: ¿cambió la salida por defecto (audífonos, monitor...)?
            if cap.is_none() || last_check.elapsed() > Duration::from_secs(2) {
                last_check = Instant::now();
                let changed = match (&cap, default_device()) {
                    (Some(c), Some((_, id))) => c.id != id,
                    (None, _) => true,
                    _ => false,
                };
                if changed {
                    if let Some(c) = cap.take() {
                        let _ = c.client.Stop();
                    }
                    cap = open_capture();
                }
            }
            let rate = cap.as_ref().map(|c| c.rate).unwrap_or(48_000);
            let got = match &cap {
                Some(c) => drain(c, &mut ring, &mut pos).unwrap_or_else(|_| {
                    cap = None;
                    0
                }),
                None => 0,
            };
            if got > 0 {
                last_data = Instant::now();
            } else if last_data.elapsed() > Duration::from_millis(150) {
                // Sin audio: el loopback no entrega paquetes → rellenar con silencio
                for _ in 0..(rate as usize / 30).min(N) {
                    ring[pos] = 0.0;
                    pos = (pos + 1) % N;
                }
            }
            for i in 0..N {
                buf[i] = Complex::new(ring[(pos + i) % N] * window[i], 0.0);
            }
            fft.process(&mut buf);
            let (fmin, fmax) = (50.0f64, (rate as f64 / 2.0).min(16_000.0));
            let bin_hz = rate as f64 / N as f64;
            for b in 0..BANDS {
                let f0 = fmin * (fmax / fmin).powf(b as f64 / BANDS as f64);
                let f1 = fmin * (fmax / fmin).powf((b + 1) as f64 / BANDS as f64);
                let i0 = ((f0 / bin_hz) as usize).max(1);
                let i1 = ((f1 / bin_hz) as usize).max(i0 + 1);
                let mut peak = 0f64;
                for k in i0..i1.min(N / 2) {
                    peak = peak.max(buf[k].norm() as f64);
                }
                let mut db = 20.0 * (peak / (N as f64 / 4.0) + 1e-9).log10();
                db += 1.5 * (f0 / fmin).log2(); // compensar la caída natural de agudos
                let mut v = ((db + 45.0) / 52.0).clamp(0.0, 1.0) as f32;
                v *= v;
                level[b] = if v > level[b] { v } else { level[b] * 0.82 + v * 0.18 };
            }
            LEVELS.lock().unwrap().copy_from_slice(&level);
        }
    });
}
