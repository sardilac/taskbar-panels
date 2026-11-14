//! Prueba de velocidad (bajo demanda) contra speed.cloudflare.com:
//! latencia/jitter (6 peticiones vacías), bajada y subida con 4 conexiones en paralelo durante
//! ~8 s cada una, con topes de datos (250 MB de bajada, 100 MB de subida).

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum Phase {
    #[default]
    Idle,
    Latency,
    Download,
    Upload,
    Done,
    Error,
}

#[derive(Clone, Default)]
pub struct Speed {
    pub phase: Phase,
    /// progreso de la fase actual (0..1)
    pub progress: f32,
    /// velocidad en vivo (Mb/s) durante bajada/subida
    pub live_mbps: f64,
    pub ping_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub down_mbps: Option<f64>,
    pub up_mbps: Option<f64>,
    /// centro de datos de Cloudflare (p. ej. "MIA")
    pub colo: String,
    pub finished_at: Option<i64>,
    pub error: String,
}

pub static SPEED: LazyLock<Mutex<Speed>> = LazyLock::new(|| Mutex::new(Speed::default()));
static RUNNING: AtomicBool = AtomicBool::new(false);

const STREAMS: usize = 4;
const PHASE_SECS: f64 = 8.0;
const WARMUP_SECS: f64 = 1.0;
const DOWN_CAP: u64 = 250_000_000;
const UP_CAP: u64 = 100_000_000;

fn set(f: impl FnOnce(&mut Speed)) {
    f(&mut SPEED.lock().unwrap());
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .user_agent("taskbar-panels-speedtest")
        .build()
}

/// Mide una fase de transferencia: lanza los hilos, muestrea la velocidad en vivo y devuelve
/// la velocidad media (Mb/s) descontando el calentamiento inicial
fn measure(counter: Arc<AtomicU64>, cap: u64, handles: Vec<std::thread::JoinHandle<()>>, stop: Arc<AtomicBool>) -> f64 {
    let start = Instant::now();
    let (mut prev_b, mut prev_t) = (0u64, start);
    let mut warm: Option<(u64, Instant)> = None;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let now = Instant::now();
        let b = counter.load(Ordering::Relaxed);
        let el = now.duration_since(start).as_secs_f64();
        let live = (b - prev_b) as f64 * 8.0 / now.duration_since(prev_t).as_secs_f64().max(1e-3) / 1e6;
        (prev_b, prev_t) = (b, now);
        if warm.is_none() && el >= WARMUP_SECS {
            warm = Some((b, now));
        }
        set(|s| {
            s.live_mbps = s.live_mbps * 0.5 + live * 0.5;
            s.progress = (el / PHASE_SECS).min(1.0) as f32;
        });
        if el >= PHASE_SECS || b >= cap || handles.iter().all(|h| h.is_finished()) {
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    let end = Instant::now();
    let total = counter.load(Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
    match warm {
        Some((wb, wt)) if end > wt => (total - wb) as f64 * 8.0 / end.duration_since(wt).as_secs_f64() / 1e6,
        _ => total as f64 * 8.0 / end.duration_since(start).as_secs_f64().max(1e-3) / 1e6,
    }
}

/// Cuerpo de subida que cuenta lo enviado y se corta al pedir la parada
struct UpBody {
    left: u64,
    counter: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl Read for UpBody {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 || self.stop.load(Ordering::Relaxed) {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(self.left).min(64 * 1024) as usize;
        for (i, b) in buf[..n].iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31); // datos no triviales (evita compresión)
        }
        self.left -= n as u64;
        self.counter.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn run() -> Result<(), String> {
    let a = agent();
    // ---- latencia y jitter
    set(|s| {
        *s = Speed { phase: Phase::Latency, ..Default::default() };
    });
    let mut times = Vec::new();
    for i in 0..6 {
        let t0 = Instant::now();
        let r = a.get("https://speed.cloudflare.com/__down?bytes=0").call().map_err(|e| e.to_string())?;
        if i == 0 {
            if let Some(ray) = r.header("cf-ray") {
                let colo = ray.rsplit('-').next().unwrap_or("").to_string();
                set(|s| s.colo = colo);
            }
        }
        let _ = r.into_string();
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
        set(|s| s.progress = (i + 1) as f32 / 6.0);
    }
    times.remove(0); // la primera incluye el establecimiento de la conexión
    let ping = times.iter().cloned().fold(f64::MAX, f64::min);
    let jitter = times.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (times.len() - 1).max(1) as f64;
    set(|s| {
        s.ping_ms = Some(ping);
        s.jitter_ms = Some(jitter);
    });

    // ---- bajada
    set(|s| {
        s.phase = Phase::Download;
        s.progress = 0.0;
        s.live_mbps = 0.0;
    });
    let counter = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let handles = (0..STREAMS)
        .map(|_| {
            let (a, counter, stop) = (a.clone(), counter.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                while !stop.load(Ordering::Relaxed) && counter.load(Ordering::Relaxed) < DOWN_CAP {
                    let Ok(r) = a.get(&format!("https://speed.cloudflare.com/__down?bytes={}", DOWN_CAP / STREAMS as u64)).call() else { break };
                    let mut rd = r.into_reader();
                    while !stop.load(Ordering::Relaxed) {
                        match rd.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                counter.fetch_add(n as u64, Ordering::Relaxed);
                            }
                        }
                    }
                }
            })
        })
        .collect();
    let down = measure(counter, DOWN_CAP, handles, stop);
    set(|s| s.down_mbps = Some(down));

    // ---- subida
    set(|s| {
        s.phase = Phase::Upload;
        s.progress = 0.0;
        s.live_mbps = 0.0;
    });
    let counter = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let per = UP_CAP / STREAMS as u64;
    let handles = (0..STREAMS)
        .map(|_| {
            let (a, counter, stop) = (a.clone(), counter.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) && counter.load(Ordering::Relaxed) < UP_CAP {
                    let chunk = 16_000_000u64.min(per);
                    let body = UpBody { left: chunk, counter: counter.clone(), stop: stop.clone() };
                    let r = a
                        .post("https://speed.cloudflare.com/__up")
                        .set("Content-Type", "application/octet-stream")
                        .set("Content-Length", &chunk.to_string())
                        .send(body);
                    if r.is_err() && stop.load(Ordering::Relaxed) {
                        break;
                    }
                }
            })
        })
        .collect();
    let up = measure(counter, UP_CAP, handles, stop);
    set(|s| {
        s.up_mbps = Some(up);
        s.phase = Phase::Done;
        s.progress = 1.0;
        s.live_mbps = 0.0;
        s.finished_at = Some(crate::util::now_epoch());
    });
    Ok(())
}

/// Inicia la prueba en segundo plano (si no hay otra en curso)
pub fn start() {
    if RUNNING.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        if let Err(e) = run() {
            set(|s| {
                s.phase = Phase::Error;
                s.error = e;
                s.live_mbps = 0.0;
            });
        }
        RUNNING.store(false, Ordering::Relaxed);
    });
}
