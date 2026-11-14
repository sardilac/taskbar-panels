//! Estado compartido entre hilos + trabajadores de IP pública y consumo (Claude/Copilot).

use crate::util::{fmt_left, hidden_cmd, home_dir, parse_iso};
use serde_json::Value;
use std::io::Write;
use std::process::Stdio;
use std::sync::mpsc::{channel, Sender};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Clone, Default)]
pub struct UsageRow {
    pub pct: Option<f64>,
    pub text: String,
}

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
    // Consumo
    pub usage: [UsageRow; 3],
    pub tip_claude: String,
    pub tip_copilot: String,
    /// créditos premium de Copilot en formato compacto, p. ej. "1.5k / 1M"
    pub copilot_detail: String,
    // Detalle estructurado para la ventana flotante de consumo
    pub claude_plan: String,
    pub claude_reset5: Option<i64>,
    pub claude_reset7: Option<i64>,
    pub copilot_plan: String,
    pub copilot_used: f64,
    pub copilot_total: f64,
    pub copilot_unlimited: bool,
    pub copilot_reset: Option<i64>,
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
        ip: "consultando...".into(),
        usage: [
            UsageRow { pct: None, text: "…".into() },
            UsageRow { pct: None, text: "…".into() },
            UsageRow { pct: None, text: "…".into() },
        ],
        tip_claude: "Claude: consultando...".into(),
        tip_copilot: "Copilot: consultando...".into(),
        cpu: -1.0,
        cpu_t: -1,
        gpu: -1,
        gpu_t: -1,
        fan_c: -1,
        fan_g: -1,
        title: "Sin reproducción".into(),
        ..Default::default()
    })
});

pub fn st() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

static IP_REFRESH: LazyLock<Mutex<Option<Sender<()>>>> = LazyLock::new(|| Mutex::new(None));
static USAGE_REFRESH: LazyLock<Mutex<Option<Sender<()>>>> = LazyLock::new(|| Mutex::new(None));

pub fn refresh_ip() {
    if let Some(tx) = IP_REFRESH.lock().unwrap().as_ref() {
        let _ = tx.send(());
    }
}
pub fn refresh_usage() {
    if let Some(tx) = USAGE_REFRESH.lock().unwrap().as_ref() {
        let _ = tx.send(());
    }
}

fn agent() -> ureq::Agent {
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
                None => st().ip = "sin conexión".into(),
            }
            let _ = rx.recv_timeout(Duration::from_secs(300));
        }
    });
}

// ---------------------------------------------------------------- Consumo
fn update_claude(a: &ureq::Agent) {
    let cred: Option<Value> = std::fs::read_to_string(home_dir().join(".claude").join(".credentials.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let Some(cred) = cred else {
        set_claude_error("sin credenciales de Claude Code");
        return;
    };
    let tok = cred["claudeAiOauth"]["accessToken"].as_str().unwrap_or_default();
    let plan = cred["claudeAiOauth"]["subscriptionType"].as_str().unwrap_or("?").to_string();
    let res = a
        .get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {tok}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call();
    match res {
        Ok(r) => match r.into_json::<Value>() {
            Ok(u) => {
                let p5 = u["five_hour"]["utilization"].as_f64();
                let p7 = u["seven_day"]["utilization"].as_f64();
                let r5 = u["five_hour"]["resets_at"].as_str().and_then(parse_iso);
                let r7 = u["seven_day"]["resets_at"].as_str().and_then(parse_iso);
                let mut s = st();
                s.claude_plan = plan.clone();
                s.claude_reset5 = r5;
                s.claude_reset7 = r7;
                s.usage[0] = UsageRow { pct: p5, text: p5.map(|p| format!("{p:.0}%")).unwrap_or("—".into()) };
                s.usage[1] = UsageRow { pct: p7, text: p7.map(|p| format!("{p:.0}%")).unwrap_or("—".into()) };
                s.tip_claude = format!(
                    "Claude ({plan})\n  Sesión 5h: {}%  · reinicia en {}\n  Semana:    {}%  · reinicia en {}",
                    p5.unwrap_or(0.0),
                    r5.map(fmt_left).unwrap_or("?".into()),
                    p7.unwrap_or(0.0),
                    r7.map(fmt_left).unwrap_or("?".into())
                );
            }
            Err(_) => set_claude_error("respuesta no válida"),
        },
        Err(ureq::Error::Status(401, _)) => set_claude_error("token expirado (abre Claude Code)"),
        Err(ureq::Error::Status(429, _)) => {
            // Límite de consultas: se conservan los últimos valores (no se muestran guiones)
            let mut s = st();
            if !s.tip_claude.contains("(429)") {
                s.tip_claude.push_str("\n  (datos anteriores: la API respondió 429)");
            }
            return;
        }
        Err(_) => set_claude_error("sin datos"),
    }
}

fn set_claude_error(msg: &str) {
    let mut s = st();
    for i in 0..2 {
        s.usage[i] = UsageRow { pct: None, text: "—".into() };
    }
    s.tip_claude = format!("Claude: {msg}");
}

/// Token de GitHub guardado por Git Credential Manager (sin interacción)
fn github_token() -> Option<String> {
    let mut child = hidden_cmd("git")
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(b"protocol=https\nhost=github.com\n\n").ok()?;
    let out = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("password=").map(|p| p.trim().to_string()))
        .filter(|t| !t.is_empty())
}

/// Número compacto: 1486 → "1.5k", 1000000 → "1M"
fn compact(x: f64) -> String {
    let (v, s) = if x >= 1e6 { (x / 1e6, "M") } else if x >= 1e3 { (x / 1e3, "k") } else { (x, "") };
    let t = if v >= 100.0 || (v - v.round()).abs() < 0.05 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{t}{s}")
}

fn update_copilot(a: &ureq::Agent, token: &mut Option<String>) {
    let call = |t: &str| {
        a.get("https://api.github.com/copilot_internal/user")
            .set("Authorization", &format!("token {t}"))
            .call()
            .ok()
            .and_then(|r| r.into_json::<Value>().ok())
    };
    if token.is_none() {
        *token = github_token();
    }
    let mut c = token.as_deref().and_then(call);
    if c.is_none() {
        *token = github_token(); // reintento con credencial renovada
        c = token.as_deref().and_then(call);
    }
    let mut s = st();
    let Some(c) = c else {
        s.usage[2] = UsageRow { pct: None, text: "—".into() };
        s.tip_copilot = "Copilot: sin datos (credencial de GitHub)".into();
        return;
    };
    let p = &c["quota_snapshots"]["premium_interactions"];
    if p["unlimited"].as_bool().unwrap_or(false) {
        s.usage[2] = UsageRow { pct: Some(0.0), text: "∞".into() };
    } else {
        let used = 100.0 - p["percent_remaining"].as_f64().unwrap_or(100.0);
        let text = if used < 10.0 { format!("{used:.1}%") } else { format!("{used:.0}%") };
        s.usage[2] = UsageRow { pct: Some(used), text };
    }
    s.copilot_detail = if p["unlimited"].as_bool().unwrap_or(false) {
        "ilimitado".into()
    } else {
        format!("{} / {}", compact(p["credits_used"].as_f64().unwrap_or(0.0)), compact(p["entitlement"].as_f64().unwrap_or(0.0)))
    };
    s.copilot_plan = c["copilot_plan"].as_str().unwrap_or("?").to_string();
    s.copilot_used = p["credits_used"].as_f64().unwrap_or(0.0);
    s.copilot_total = p["entitlement"].as_f64().unwrap_or(0.0);
    s.copilot_unlimited = p["unlimited"].as_bool().unwrap_or(false);
    s.copilot_reset = c["quota_reset_date_utc"].as_str().and_then(parse_iso);
    let reset = c["quota_reset_date_utc"].as_str().unwrap_or_default();
    s.tip_copilot = format!(
        "Copilot ({})\n  Premium: {:.0} de {:.0} usados ({:.2}%)\n  Reinicia: {} (en {})",
        c["copilot_plan"].as_str().unwrap_or("?"),
        p["credits_used"].as_f64().unwrap_or(0.0),
        p["entitlement"].as_f64().unwrap_or(0.0),
        100.0 - p["percent_remaining"].as_f64().unwrap_or(100.0),
        reset.get(0..10).unwrap_or(reset),
        parse_iso(reset).map(fmt_left).unwrap_or("?".into())
    );
}

// ---------------------------------------------------------------- caché de consumo
fn cache_path() -> std::path::PathBuf {
    crate::util::appdata_dir().join("usage_cache.json")
}

/// Guarda los últimos valores para mostrarlos al instante al reiniciar (sin esperar a la API)
fn save_cache(claude_at: i64, copilot_at: i64) {
    let s = st();
    let rows: Vec<Value> = s.usage.iter().map(|r| serde_json::json!({ "pct": r.pct, "text": r.text })).collect();
    let v = serde_json::json!({
        "claude_at": claude_at, "copilot_at": copilot_at, "usage": rows,
        "tip_claude": s.tip_claude, "tip_copilot": s.tip_copilot, "copilot_detail": s.copilot_detail,
        "claude_plan": s.claude_plan, "claude_reset5": s.claude_reset5, "claude_reset7": s.claude_reset7,
        "copilot_plan": s.copilot_plan, "copilot_used": s.copilot_used, "copilot_total": s.copilot_total,
        "copilot_unlimited": s.copilot_unlimited, "copilot_reset": s.copilot_reset,
    });
    let _ = std::fs::write(cache_path(), v.to_string());
}

/// Carga la caché; devuelve (claude_at, copilot_at)
fn load_cache() -> (i64, i64) {
    let Some(v) = std::fs::read_to_string(cache_path()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
        return (0, 0);
    };
    let mut s = st();
    if let Some(rows) = v["usage"].as_array() {
        for (i, r) in rows.iter().take(3).enumerate() {
            s.usage[i] = UsageRow { pct: r["pct"].as_f64(), text: r["text"].as_str().unwrap_or("—").to_string() };
        }
    }
    s.tip_claude = v["tip_claude"].as_str().unwrap_or_default().to_string();
    s.tip_copilot = v["tip_copilot"].as_str().unwrap_or_default().to_string();
    s.copilot_detail = v["copilot_detail"].as_str().unwrap_or_default().to_string();
    s.claude_plan = v["claude_plan"].as_str().unwrap_or_default().to_string();
    s.claude_reset5 = v["claude_reset5"].as_i64();
    s.claude_reset7 = v["claude_reset7"].as_i64();
    s.copilot_plan = v["copilot_plan"].as_str().unwrap_or_default().to_string();
    s.copilot_used = v["copilot_used"].as_f64().unwrap_or(0.0);
    s.copilot_total = v["copilot_total"].as_f64().unwrap_or(0.0);
    s.copilot_unlimited = v["copilot_unlimited"].as_bool().unwrap_or(false);
    s.copilot_reset = v["copilot_reset"].as_i64();
    // Caché de una versión anterior (sin detalle estructurado): consultar ya para completarlo
    let claude_at = if v.get("claude_plan").is_some() { v["claude_at"].as_i64().unwrap_or(0) } else { 0 };
    let copilot_at = if v.get("copilot_plan").is_some() { v["copilot_at"].as_i64().unwrap_or(0) } else { 0 };
    (claude_at, copilot_at)
}

pub fn spawn_usage_worker() {
    const CLAUDE_EVERY: i64 = 180; // s
    const COPILOT_EVERY: i64 = 300; // s
    let (tx, rx) = channel();
    *USAGE_REFRESH.lock().unwrap() = Some(tx);
    std::thread::spawn(move || {
        let a = agent();
        let mut token = None;
        // Respetar los intervalos aunque el programa se reinicie (evita el 429 de Claude)
        let (mut claude_at, mut copilot_at) = load_cache();
        loop {
            let now = crate::util::now_epoch();
            let mut changed = false;
            if now - claude_at >= CLAUDE_EVERY {
                update_claude(&a);
                claude_at = now;
                changed = true;
            }
            if now - copilot_at >= COPILOT_EVERY {
                update_copilot(&a, &mut token);
                copilot_at = now;
                changed = true;
            }
            if changed {
                save_cache(claude_at, copilot_at);
            }
            let next = (claude_at + CLAUDE_EVERY).min(copilot_at + COPILOT_EVERY) - crate::util::now_epoch();
            if rx.recv_timeout(Duration::from_secs(next.clamp(5, 300) as u64)).is_ok() {
                // actualización manual desde el menú: ambos ya
                claude_at = 0;
                copilot_at = 0;
            }
        }
    });
}