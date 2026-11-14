//! Prueba de velocidad (bajo demanda) con tres proveedores a elegir (`SpeedTestProvider`):
//! - `cloudflare`: speed.cloudflare.com (red anycast de Cloudflare)
//! - `ookla`: servidores de Speedtest.net (Ookla); se elige el de menor latencia entre los más cercanos
//! - `librespeed`: servidores públicos de LibreSpeed (código abierto); se elige el de menor latencia
//! - `gci`: servidor LibreSpeed de GCI (speedtest.gci.co.cr, Costa Rica)
//!
//! Mide latencia/jitter (6 peticiones vacías), bajada y subida con 4 conexiones en paralelo durante
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

/// Proveedores disponibles: (clave de configuración, nombre visible)
pub const PROVIDERS: [(&str, &str); 4] = [("ookla", "Speedtest.net"), ("cloudflare", "Cloudflare"), ("librespeed", "LibreSpeed"), ("gci", "GCI")];

/// Nombre visible de un proveedor (clave desconocida → Speedtest.net, el predeterminado)
pub fn provider_name(key: &str) -> &'static str {
    PROVIDERS.iter().find(|p| p.0 == key).unwrap_or(&PROVIDERS[0]).1
}

/// Proveedor normalizado a partir de la configuración
pub fn provider_key(cfg: &serde_json::Value) -> &'static str {
    let k = cfg["SpeedTestProvider"].as_str().unwrap_or("ookla");
    PROVIDERS.iter().find(|p| p.0 == k).unwrap_or(&PROVIDERS[0]).0
}

/// Proveedor elegido (índice en PROVIDERS), accesible desde el pintado sin tocar la configuración
static SELECTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn select(key: &str) {
    SELECTED.store(PROVIDERS.iter().position(|p| p.0 == key).unwrap_or(0), Ordering::Relaxed);
}

pub fn selected() -> &'static str {
    PROVIDERS[SELECTED.load(Ordering::Relaxed) % PROVIDERS.len()].0
}

/// Siguiente proveedor de la lista (para el selector de la tarjeta)
pub fn next_provider() -> &'static str {
    PROVIDERS[(SELECTED.load(Ordering::Relaxed) + 1) % PROVIDERS.len()].0
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
    /// proveedor usado (clave) y servidor elegido (p. ej. "SJO", "Metrocom · San Rafael")
    pub provider: String,
    pub server: String,
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

fn nonce() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// Servidor de prueba: URLs de latencia, bajada (por tamaño en bytes) y subida
#[derive(Clone)]
struct Target {
    ping: String,
    /// bajada: prefijo + tamaño + sufijo
    down: (String, u64),
    up: String,
    /// tamaño de cada envío de subida
    up_chunk: u64,
    label: String,
}

impl Target {
    fn down_url(&self, size: u64) -> String {
        self.down.0.replace("{n}", &nonce().to_string()).replace("{size}", &size.to_string())
    }
    fn up_url(&self) -> String {
        self.up.replace("{n}", &nonce().to_string())
    }
}

/// Mide una petición (ms) a la URL de latencia
fn ping_once(a: &ureq::Agent, url: &str) -> Option<f64> {
    let t0 = Instant::now();
    let r = a.get(url).call().ok()?;
    let _ = r.into_string();
    Some(t0.elapsed().as_secs_f64() * 1000.0)
}

/// De una lista de candidatos, el de menor latencia (2 peticiones cada uno, en paralelo)
fn fastest(a: &ureq::Agent, cands: Vec<Target>) -> Option<Target> {
    let handles: Vec<_> = cands
        .into_iter()
        .map(|t| {
            let a = a.clone();
            std::thread::spawn(move || {
                let _ = ping_once(&a, &t.ping)?; // la primera abre la conexión
                ping_once(&a, &t.ping).map(|ms| (ms, t))
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok().flatten()).min_by(|x, y| x.0.total_cmp(&y.0)).map(|x| x.1)
}

fn target_cloudflare(a: &ureq::Agent) -> Result<Target, String> {
    // el centro de datos (colo) viene en la cabecera cf-ray: "<id>-SJO"
    let r = a.get("https://speed.cloudflare.com/__down?bytes=0").call().map_err(|e| e.to_string())?;
    let colo = r.header("cf-ray").and_then(|ray| ray.rsplit('-').next()).unwrap_or("").to_string();
    Ok(Target {
        ping: "https://speed.cloudflare.com/__down?bytes=0".into(),
        down: ("https://speed.cloudflare.com/__down?bytes={size}".into(), DOWN_CAP / STREAMS as u64),
        up: "https://speed.cloudflare.com/__up".into(),
        up_chunk: 16_000_000,
        label: colo,
    })
}

fn target_ookla(a: &ureq::Agent) -> Result<Target, String> {
    let list: serde_json::Value = a
        .get("https://www.speedtest.net/api/js/servers?engine=js&https_functional=true&limit=8")
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let cands: Vec<Target> = list
        .as_array()
        .map(|v| v.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let host = s["host"].as_str()?;
            let label = format!("{} · {}", s["sponsor"].as_str().unwrap_or(""), s["name"].as_str().unwrap_or(""));
            Some(Target {
                ping: format!("https://{host}/hello?nocache={{n}}"),
                down: (format!("https://{host}/download?nocache={{n}}&size={{size}}"), 25_000_000),
                up: format!("https://{host}/upload?nocache={{n}}"),
                up_chunk: 4_000_000,
                label,
            })
        })
        .take(5)
        .map(|mut t| {
            t.ping = t.ping.replace("{n}", "0");
            t
        })
        .collect();
    fastest(a, cands).ok_or_else(|| crate::i18n::tr("no hay servidores de Speedtest.net disponibles", "no Speedtest.net servers available").into())
}

fn target_librespeed(a: &ureq::Agent) -> Result<Target, String> {
    let list: serde_json::Value = a
        .get("https://librespeed.org/backend-servers/servers.php")
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    let join = |base: &str, path: &str| {
        let base = if base.starts_with("//") { format!("https:{base}") } else { base.to_string() };
        format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
    };
    let cands: Vec<Target> = list
        .as_array()
        .map(|v| v.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let base = s["server"].as_str()?;
            let dl = join(base, s["dlURL"].as_str()?);
            let ul = join(base, s["ulURL"].as_str()?);
            let pi = join(base, s["pingURL"].as_str()?);
            if !dl.starts_with("https://") {
                return None;
            }
            Some(Target {
                ping: pi,
                // garbage.php entrega ckSize bloques de 1 MiB
                down: (format!("{dl}?cors=true&r={{n}}&ckSize={{size}}"), 25),
                up: format!("{ul}?cors=true&r={{n}}"),
                up_chunk: 1_000_000,
                label: s["name"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect();
    fastest(a, cands).ok_or_else(|| crate::i18n::tr("no hay servidores de LibreSpeed disponibles", "no LibreSpeed servers available").into())
}

/// Servidor LibreSpeed propio de GCI (instalación autónoma: backend/garbage.php y backend/empty.php)
fn target_gci() -> Target {
    let base = "https://speedtest.gci.co.cr/backend";
    Target {
        ping: format!("{base}/empty.php?cors=true&r={{n}}"),
        down: (format!("{base}/garbage.php?cors=true&r={{n}}&ckSize={{size}}"), 25),
        up: format!("{base}/empty.php?cors=true&r={{n}}"),
        up_chunk: 1_000_000,
        label: "speedtest.gci.co.cr".into(),
    }
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
    // los hilos terminan solos al ver la parada (no se espera: una subida en curso puede tardar en
    // cerrarse hasta el timeout del servidor y dejaría la tarjeta "midiendo" sin necesidad)
    drop(handles);
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

fn run(provider: &'static str) -> Result<(), String> {
    let a = agent();
    // ---- servidor y latencia/jitter
    set(|s| {
        *s = Speed { phase: Phase::Latency, provider: provider.into(), ..Default::default() };
    });
    let t = match provider {
        "ookla" => target_ookla(&a)?,
        "librespeed" => target_librespeed(&a)?,
        "gci" => target_gci(),
        _ => target_cloudflare(&a)?,
    };
    set(|s| s.server = t.label.clone());
    let mut times = Vec::new();
    for i in 0..6 {
        times.push(ping_once(&a, &t.ping.replace("{n}", &nonce().to_string())).ok_or(crate::i18n::tr("sin respuesta del servidor", "no response from server"))?);
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
            let (a, t, counter, stop) = (a.clone(), t.clone(), counter.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                // si el servidor rechaza el bloque (p. ej. 429 por límite de peticiones), se reintenta
                // con bloques más pequeños hasta 1/25 del tamaño inicial
                let mut size = t.down.1;
                while !stop.load(Ordering::Relaxed) && counter.load(Ordering::Relaxed) < DOWN_CAP {
                    let r = match a.get(&t.down_url(size)).call() {
                        Ok(r) => r,
                        Err(_) if size > t.down.1 / 25 => {
                            size = (size / 5).max(1);
                            continue;
                        }
                        Err(_) => break,
                    };
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
    let got = counter.clone();
    let down = measure(counter, DOWN_CAP, handles, stop);
    if got.load(Ordering::Relaxed) == 0 {
        return Err(crate::i18n::tr("el servidor rechazó la descarga (límite de peticiones)", "server rejected the download (rate limit)").into());
    }
    set(|s| s.down_mbps = Some(down));

    // ---- subida
    set(|s| {
        s.phase = Phase::Upload;
        s.progress = 0.0;
        s.live_mbps = 0.0;
    });
    let counter = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let handles = (0..STREAMS)
        .map(|_| {
            let (a, t, counter, stop) = (a.clone(), t.clone(), counter.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut fails = 0;
                while !stop.load(Ordering::Relaxed) && counter.load(Ordering::Relaxed) < UP_CAP && fails < 3 {
                    let body = UpBody { left: t.up_chunk, counter: counter.clone(), stop: stop.clone() };
                    let r = a
                        .post(&t.up_url())
                        .set("Content-Type", "application/octet-stream")
                        .set("Content-Length", &t.up_chunk.to_string())
                        .send(body);
                    if r.is_err() {
                        fails += 1;
                    }
                }
            })
        })
        .collect();
    let got = counter.clone();
    let up = measure(counter, UP_CAP, handles, stop);
    if got.load(Ordering::Relaxed) == 0 {
        return Err(crate::i18n::tr("el servidor rechazó la subida", "server rejected the upload").into());
    }
    set(|s| {
        s.up_mbps = Some(up);
        s.phase = Phase::Done;
        s.progress = 1.0;
        s.live_mbps = 0.0;
        s.finished_at = Some(crate::util::now_epoch());
    });
    Ok(())
}

/// Inicia la prueba en segundo plano con el proveedor indicado (si no hay otra en curso)
pub fn start(provider: &'static str) {
    if RUNNING.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        if let Err(e) = run(provider) {
            set(|s| {
                s.phase = Phase::Error;
                s.error = e;
                s.live_mbps = 0.0;
            });
        }
        RUNNING.store(false, Ordering::Relaxed);
    });
}

/// ¿Hay una prueba en curso?
pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}
