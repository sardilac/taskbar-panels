//! Estado compartido entre hilos + trabajador de IP pública. El consumo de IA está en ai.rs.

use serde_json::Value;
use std::sync::mpsc::{channel, Sender};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

/// Unidad fija: letra, nombre del volumen, total y libre (bytes)
#[derive(Clone, Default)]
pub struct Drive {
    pub letter: char,
    pub label: String,
    pub total: f64,
    pub free: f64,
}

#[derive(Clone, Default)]
pub struct State {
    // Red / IP
    pub up: String,
    pub down: String,
    pub ip: String,
    pub isp: String,
    pub city: String,
    pub ip_changed: Option<(String, String, String)>,
    pub ip_changed_at: Option<std::time::Instant>,
    pub down_bps: f64,
    pub up_bps: f64,
    /// historial de 60 s (bytes/s), el más reciente al final
    pub hist_down: Vec<f32>,
    pub hist_up: Vec<f32>,
    // Totales de la sesión (bytes)
    pub sess_down: f64,
    pub sess_up: f64,
    pub media_album: String,
    // Sistema
    pub cpu: f32,
    pub cpu_t: i32,
    pub gpu: i32,
    pub gpu_t: i32,
    pub fan_c: i32,
    pub fan_g: i32,
    pub fps: u32,
    pub app: String,
    // Detalle del sistema (ventana flotante)
    pub cpu_name: String,
    pub cpu_mhz: f32,
    pub gpu_name: String,
    pub vram_used: f32,  // GB
    pub vram_total: f32, // GB
    pub gpu_power: f32,  // W
    pub gpu_mhz: u32,
    pub ram_used: f32,  // GB
    pub ram_total: f32, // GB
    /// historiales de 60 s (el más reciente al final)
    pub hist_cpu: Vec<f32>,
    pub hist_gpu: Vec<f32>,
    pub hist_cpu_t: Vec<f32>,
    pub hist_gpu_t: Vec<f32>,
    pub hist_fps: Vec<f32>,
    // Discos
    pub disk_read: f64,  // bytes/s
    pub disk_write: f64, // bytes/s
    pub hist_disk_r: Vec<f32>,
    pub hist_disk_w: Vec<f32>,
    pub drives: Vec<Drive>,
    // Multimedia
    pub title: String,
    pub artist: String,
    pub media_app: String,
    pub playing: bool,
}

pub static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        up: "↑ —".into(),
        down: "↓ —".into(),
        ip: crate::i18n::tr("consultando...", "checking...").into(),
        cpu: -1.0,
        cpu_t: -1,
        gpu: -1,
        gpu_t: -1,
        fan_c: -1,
        fan_g: -1,
        title: crate::i18n::tr("Sin reproducción", "Nothing playing").into(),
        ..Default::default()
    })
});

pub fn st() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

static IP_REFRESH: LazyLock<Mutex<Option<Sender<()>>>> = LazyLock::new(|| Mutex::new(None));

pub fn refresh_ip() {
    if let Some(tx) = IP_REFRESH.lock().unwrap().as_ref() {
        let _ = tx.send(());
    }
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).user_agent("taskbar-panels").build()
}

// ---------------------------------------------------------------- IP pública
fn fetch_ip(a: &ureq::Agent) -> Option<(String, String, String)> {
    if let Ok(v) = a.get("https://ifconfig.co/json").call().and_then(|r| Ok(r.into_json::<Value>()?)) {
        let ip = v["ip"].as_str().unwrap_or_default().to_string();
        if !ip.is_empty() {
            let city = [v["city"].as_str(), v["country"].as_str()].iter().flatten().copied().collect::<Vec<_>>().join(", ");
            return Some((ip, v["asn_org"].as_str().unwrap_or_default().to_string(), city));
        }
    }
    let ip = a.get("https://api.ipify.org").call().ok()?.into_string().ok()?;
    Some((ip.trim().to_string(), String::new(), String::new()))
}

pub fn spawn_ip_worker() {
    let (tx, rx) = channel();
    *IP_REFRESH.lock().unwrap() = Some(tx);
    std::thread::spawn(move || {
        let a = agent();
        loop {
            match fetch_ip(&a) {
                Some((ip, isp, city)) => {
                    let mut s = st();
                    let old = s.ip.clone();
                    if !old.is_empty() && old.contains('.') && old != ip {
                        s.ip_changed = Some((old, ip.clone(), isp.clone()));
                        s.ip_changed_at = Some(std::time::Instant::now());
                    }
                    s.ip = ip;
                    s.isp = isp;
                    s.city = city;
                }
                None => st().ip = crate::i18n::tr("sin conexión", "offline").into(),
            }
            let _ = rx.recv_timeout(Duration::from_secs(300));
        }
    });
}
