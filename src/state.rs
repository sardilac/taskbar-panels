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
    /// código ISO del país de la IP pública (p. ej. "CR")
    pub country: String,
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
/// País según Cloudflare (`loc` de cdn-cgi/trace): suele estar más al día que la base de ifconfig.co
/// (p. ej. bloques de un operador de un país anunciados desde otro, VPN, centros de datos)
fn cloudflare_loc(a: &ureq::Agent) -> Option<(String, String)> {
    let body = a.get("https://www.cloudflare.com/cdn-cgi/trace").call().ok()?.into_string().ok()?;
    let field = |k: &str| body.lines().find_map(|l| l.strip_prefix(k)).map(|v| v.trim().to_string());
    let loc = field("loc=")?;
    (loc.len() == 2 && loc != "XX" && loc != "T1").then(|| (field("ip=").unwrap_or_default(), loc))
}

/// Nombre del país (ISO alfa-2) en el idioma de Windows
fn country_name(iso: &str) -> String {
    use windows::Win32::Globalization::{GetGeoInfoEx, GEO_FRIENDLYNAME};
    let code = crate::util::wide(iso);
    let mut buf = [0u16; 128];
    let n = unsafe { GetGeoInfoEx(windows::core::PCWSTR(code.as_ptr()), windows::Win32::Globalization::SYSGEOTYPE(GEO_FRIENDLYNAME.0), Some(&mut buf)) };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { iso.to_string() }
}

fn fetch_ip(a: &ureq::Agent) -> Option<(String, String, String, String)> {
    let cf = cloudflare_loc(a);
    if let Ok(v) = a.get("https://ifconfig.co/json").call().and_then(|r| Ok(r.into_json::<Value>()?)) {
        let ip = v["ip"].as_str().unwrap_or_default().to_string();
        if !ip.is_empty() {
            let mut iso = v["country_iso"].as_str().unwrap_or_default().to_string();
            let mut city = [v["city"].as_str(), v["country"].as_str()].iter().flatten().copied().collect::<Vec<_>>().join(", ");
            // si Cloudflare (misma IP) ubica la IP en otro país, manda Cloudflare
            if let Some((cf_ip, loc)) = &cf {
                if (cf_ip.is_empty() || *cf_ip == ip) && !loc.eq_ignore_ascii_case(&iso) {
                    iso = loc.clone();
                    city = country_name(loc);
                }
            }
            return Some((ip, v["asn_org"].as_str().unwrap_or_default().to_string(), city, iso));
        }
    }
    let ip = a.get("https://api.ipify.org").call().ok()?.into_string().ok()?;
    let (city, iso) = cf.map(|(_, loc)| (country_name(&loc), loc)).unwrap_or_default();
    Some((ip.trim().to_string(), String::new(), city, iso))
}

pub fn spawn_ip_worker() {
    let (tx, rx) = channel();
    *IP_REFRESH.lock().unwrap() = Some(tx);
    std::thread::spawn(move || {
        // COM para decodificar la bandera (WinRT)
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let a = agent();
        loop {
            match fetch_ip(&a) {
                Some((ip, isp, city, country)) => {
                    if !country.is_empty() {
                        crate::flags::ensure(&a, &country);
                    }
                    let mut s = st();
                    let old = s.ip.clone();
                    if !old.is_empty() && old.contains('.') && old != ip {
                        s.ip_changed = Some((old, ip.clone(), isp.clone()));
                        s.ip_changed_at = Some(std::time::Instant::now());
                    }
                    s.ip = ip;
                    s.isp = isp;
                    s.city = city;
                    s.country = country;
                }
                None => st().ip = crate::i18n::tr("sin conexión", "offline").into(),
            }
            let _ = rx.recv_timeout(Duration::from_secs(300));
        }
    });
}
