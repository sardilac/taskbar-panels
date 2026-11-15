//! Proveedores de IA: consumo de Claude (Claude Code), GitHub Copilot, API de OpenAI, API de
//! Anthropic, saldo de DeepSeek y Kimi, modelos de Gemini, Ollama (local y Cloud) y LM Studio, y
//! proveedores personalizados (URL JSON).
//!
//! config.json:
//!   "AiProviders": [ { "id": "openai-1", "type": "openai", "name": "OpenAI", "enabled": true, "budget": 50 }, ... ]
//!   "AiPrimary":   "claude" | "<id>" | "all"
//! Tipos: claude, copilot, openai, anthropic_api, deepseek, kimi, gemini, ollama, ollama_cloud,
//! lmstudio (url opcional; budget = saldo de referencia en deepseek/kimi) y custom (url,
//! percentField, valueField, labelField).
//! Las claves (admin keys, tokens) se guardan en el Administrador de credenciales de Windows con el
//! destino "TaskbarPanels:<id>", nunca en config.json.

use crate::config::Config;
use crate::state::agent;
use crate::util::{hidden_cmd, home_dir, now_epoch, parse_iso};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Write;
use std::process::Stdio;
use std::sync::mpsc::{channel, Sender};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

// ---------------------------------------------------------------- modelo
/// Indicador de un proveedor. `label` es una clave neutra (session5h, weekly, premium, month, …)
/// o un texto libre; se traduce al dibujar.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Metric {
    pub label: String,
    pub pct: Option<f64>,
    pub value: String,
    pub detail: String,
    pub reset: Option<i64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub plan: String,
    pub metrics: Vec<Metric>,
    /// error o aviso (vacío si todo está bien)
    pub status: String,
    pub fetched_at: i64,
}

/// Configuración de un proveedor (entrada de "AiProviders")
#[derive(Clone)]
pub struct ProviderCfg {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub v: Value,
}

pub static AI: LazyLock<Mutex<Vec<Provider>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static REFRESH: LazyLock<Mutex<Option<Sender<()>>>> = LazyLock::new(|| Mutex::new(None));

pub fn refresh() {
    if let Some(tx) = REFRESH.lock().unwrap().as_ref() {
        let _ = tx.send(());
    }
}

pub fn default_name(kind: &str) -> &'static str {
    match kind {
        "claude" => "Claude",
        "copilot" => "GitHub Copilot",
        "openai" => "OpenAI API",
        "anthropic_api" => "Anthropic API",
        "deepseek" => "DeepSeek",
        "kimi" => "Kimi",
        "gemini" => "Gemini",
        "ollama" => "Ollama",
        "ollama_cloud" => "Ollama Cloud",
        "lmstudio" => "LM Studio",
        _ => "IA",
    }
}

/// Proveedores configurados (por defecto: Claude y Copilot). Respeta el antiguo "Panels"/"UsageShow".
pub fn providers(cfg: &Config) -> Vec<ProviderCfg> {
    let list = cfg.v.get("AiProviders").and_then(|a| a.as_array()).cloned().unwrap_or_else(|| {
        vec![json!({ "id": "claude", "type": "claude" }), json!({ "id": "copilot", "type": "copilot" })]
    });
    list.into_iter()
        .filter_map(|v| {
            let kind = v["type"].as_str()?.to_string();
            let id = v["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| kind.clone());
            let name = v["name"].as_str().map(|s| s.to_string()).unwrap_or_else(|| default_name(&kind).to_string());
            let enabled = v["enabled"].as_bool().unwrap_or(true);
            Some(ProviderCfg { id, kind, name, enabled, v })
        })
        .collect()
}

/// Proveedor principal: id, o "all" para el resumen de todos
pub fn primary(cfg: &Config) -> String {
    if let Some(p) = cfg.v["AiPrimary"].as_str() {
        return p.to_string();
    }
    match cfg.v["UsageShow"].as_str() {
        Some("claude") => "claude".into(),
        Some("copilot") => "copilot".into(),
        _ => "all".into(),
    }
}

/// Lista de proveedores (habilitados) en el orden de la configuración, con sus datos
pub fn snapshot(cfg: &Config) -> Vec<Provider> {
    let data = AI.lock().unwrap().clone();
    providers(cfg)
        .into_iter()
        .filter(|p| p.enabled)
        .map(|p| {
            data.iter().find(|d| d.id == p.id).cloned().map(|mut d| {
                d.name = p.name.clone();
                d
            }).unwrap_or(Provider { id: p.id, kind: p.kind, name: p.name, status: "…".into(), ..Default::default() })
        })
        .collect()
}

// ---------------------------------------------------------------- credenciales de Windows
fn target(id: &str) -> Vec<u16> {
    crate::util::wide(&format!("TaskbarPanels:{id}"))
}

pub fn cred_write(id: &str, secret: &str) -> bool {
    use windows::Win32::Security::Credentials::*;
    let mut t = target(id);
    let mut blob: Vec<u8> = secret.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let c = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: windows::core::PWSTR(t.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    unsafe { CredWriteW(&c, 0).is_ok() }
}

pub fn cred_read(id: &str) -> Option<String> {
    use windows::Win32::Security::Credentials::*;
    let t = target(id);
    unsafe {
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        CredReadW(windows::core::PCWSTR(t.as_ptr()), CRED_TYPE_GENERIC, 0, &mut p).ok()?;
        let c = &*p;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let u16s: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        let s = String::from_utf16_lossy(&u16s);
        CredFree(p as *const _);
        Some(s)
    }
}

pub fn cred_delete(id: &str) {
    use windows::Win32::Security::Credentials::*;
    let t = target(id);
    unsafe {
        let _ = CredDeleteW(windows::core::PCWSTR(t.as_ptr()), CRED_TYPE_GENERIC, 0);
    }
}

// ---------------------------------------------------------------- utilidades
fn money(v: f64) -> String {
    if v >= 1000.0 { format!("${v:.0}") } else { format!("${v:.2}") }
}

/// Inicio del mes actual y del siguiente (UTC), en segundos Unix
fn month_bounds() -> (i64, i64) {
    let now = now_epoch();
    let days = now.div_euclid(86_400);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let start = now - (d - 1) * 86_400 - now.rem_euclid(86_400);
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let next = parse_iso(&format!("{ny:04}-{nm:02}-01")).unwrap_or(start + 31 * 86_400);
    (start, next)
}

/// Valor de un JSON por ruta con puntos ("a.b.0.c")
fn json_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        cur = match part.parse::<usize>() {
            Ok(i) if cur.is_array() => cur.get(i)?,
            _ => cur.get(part)?,
        };
    }
    Some(cur)
}

fn as_number(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().trim_end_matches('%').parse().ok()))
}

// ---------------------------------------------------------------- Claude (Claude Code)
fn fetch_claude(a: &ureq::Agent, prev: &Provider) -> Provider {
    let mut p = Provider { id: prev.id.clone(), kind: "claude".into(), ..prev.clone() };
    let cred: Option<Value> = std::fs::read_to_string(home_dir().join(".claude").join(".credentials.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let Some(cred) = cred else {
        p.status = "no_credentials".into();
        return p;
    };
    let tok = cred["claudeAiOauth"]["accessToken"].as_str().unwrap_or_default();
    p.plan = cred["claudeAiOauth"]["subscriptionType"].as_str().unwrap_or_default().to_string();
    match a
        .get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {tok}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call()
    {
        Ok(r) => match r.into_json::<Value>() {
            Ok(u) => {
                let m = |key: &str, label: &str| {
                    let pct = u[key]["utilization"].as_f64();
                    Metric {
                        label: label.into(),
                        pct,
                        value: pct.map(|v| format!("{v:.0}%")).unwrap_or("—".into()),
                        detail: String::new(),
                        reset: u[key]["resets_at"].as_str().and_then(parse_iso),
                    }
                };
                p.metrics = vec![m("five_hour", "session5h"), m("seven_day", "weekly")];
                p.status.clear();
            }
            Err(_) => p.status = "invalid".into(),
        },
        Err(ureq::Error::Status(401, _)) => p.status = "expired".into(),
        Err(ureq::Error::Status(429, _)) => p.status = "rate_limited".into(), // conserva los datos anteriores
        Err(_) => p.status = "no_data".into(),
    }
    p
}

// ---------------------------------------------------------------- GitHub Copilot
/// Credencial manual del token de GitHub para Copilot (Administrador de credenciales)
pub const COPILOT_TOKEN_ID: &str = "copilot-token";

/// Ejecuta un comando oculto y devuelve su salida estándar (si terminó bien)
fn cmd_output(exe: &str, args: &[&str], input: Option<&[u8]>) -> Option<String> {
    let mut child = hidden_cmd(exe)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(i) = input {
        child.stdin.take()?.write_all(i).ok()?;
    }
    let out = child.wait_with_output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Tokens de GitHub candidatos, en orden de preferencia (fuente, token):
/// 1. el que el usuario guardó a mano (menú IA → Token de GitHub para Copilot…)
/// 2. variables GH_TOKEN / GITHUB_TOKEN
/// 3. plugins oficiales de Copilot (Visual Studio, JetBrains, Neovim…): %LOCALAPPDATA%\github-copilot\apps.json / hosts.json
/// 4. GitHub CLI (`gh auth token`)
/// 5. Git Credential Manager (`git credential fill`)
fn github_tokens() -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let mut push = |src: &'static str, t: Option<String>| {
        if let Some(t) = t.map(|t| t.trim().to_string()).filter(|t| t.len() >= 20 && !t.contains(char::is_whitespace)) {
            if !out.iter().any(|(_, x)| *x == t) {
                out.push((src, t));
            }
        }
    };
    push("manual", cred_read(COPILOT_TOKEN_ID));
    push("env", std::env::var("GH_TOKEN").ok());
    push("env", std::env::var("GITHUB_TOKEN").ok());
    if let Some(dir) = std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join("github-copilot")) {
        for f in ["apps.json", "hosts.json"] {
            let Ok(s) = std::fs::read_to_string(dir.join(f)) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&s) else { continue };
            for (k, e) in v.as_object().into_iter().flatten() {
                if k.starts_with("github.com") {
                    push("copilot-plugin", e["oauth_token"].as_str().map(String::from));
                }
            }
        }
    }
    push("gh", cmd_output("gh", &["auth", "token", "--hostname", "github.com"], None));
    push(
        "git",
        cmd_output("git", &["credential", "fill"], Some(b"protocol=https\nhost=github.com\n\n"))
            .and_then(|o| o.lines().find_map(|l| l.strip_prefix("password=").map(|p| p.to_string()))),
    );
    out
}
fn compact(x: f64) -> String {
    let (v, s) = if x >= 1e6 { (x / 1e6, "M") } else if x >= 1e3 { (x / 1e3, "k") } else { (x, "") };
    let t = if v >= 100.0 || (v - v.round()).abs() < 0.05 { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{t}{s}")
}

fn fetch_copilot(a: &ureq::Agent, prev: &Provider, token: &mut Option<String>) -> Provider {
    let mut p = Provider { kind: "copilot".into(), ..prev.clone() };
    enum R {
        Ok(Value),
        Denied,
        Net,
    }
    let call = |t: &str| match a.get("https://api.github.com/copilot_internal/user").set("Authorization", &format!("token {t}")).call() {
        Ok(r) => r.into_json::<Value>().map(R::Ok).unwrap_or(R::Net),
        Err(ureq::Error::Status(401 | 403 | 404, _)) => R::Denied,
        Err(_) => R::Net,
    };
    // primero el token que funcionó la última vez; si falla, se prueban todas las fuentes
    let mut res = token.as_deref().map(call);
    if !matches!(res, Some(R::Ok(_))) {
        let cands = github_tokens();
        if cands.is_empty() {
            *token = None;
            p.status = "no_credentials".into();
            return p;
        }
        let mut denied = false;
        res = None;
        for (_, t) in cands {
            match call(&t) {
                R::Ok(v) => {
                    *token = Some(t);
                    res = Some(R::Ok(v));
                    break;
                }
                R::Denied => denied = true,
                R::Net => {}
            }
        }
        if res.is_none() {
            *token = None;
            // con algún rechazo explícito: el token no da acceso a Copilot; si no, fallo de red
            p.status = if denied { "no_copilot" } else { "no_data" }.into();
            return p;
        }
    }
    let Some(R::Ok(c)) = res else {
        p.status = "no_data".into();
        return p;
    };    let q = &c["quota_snapshots"]["premium_interactions"];
    let unlimited = q["unlimited"].as_bool().unwrap_or(false);
    let used = 100.0 - q["percent_remaining"].as_f64().unwrap_or(100.0);
    p.plan = c["copilot_plan"].as_str().unwrap_or_default().to_string();
    p.metrics = vec![Metric {
        label: "premium".into(),
        pct: Some(if unlimited { 0.0 } else { used }),
        value: if unlimited { "∞".into() } else if used < 10.0 { format!("{used:.1}%") } else { format!("{used:.0}%") },
        detail: if unlimited {
            "∞".into()
        } else {
            format!("{} / {}", compact(q["credits_used"].as_f64().unwrap_or(0.0)), compact(q["entitlement"].as_f64().unwrap_or(0.0)))
        },
        reset: c["quota_reset_date_utc"].as_str().and_then(parse_iso),
    }];
    p.status.clear();
    p
}

// ---------------------------------------------------------------- API de OpenAI (costos de la organización)
fn fetch_openai(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "openai".into(), ..prev.clone() };
    let Some(key) = cred_read(&cfg.id) else {
        p.status = "no_key".into();
        return p;
    };
    let (start, next) = month_bounds();
    let mut total = 0.0;
    let mut page: Option<String> = None;
    for _ in 0..6 {
        let mut req = a
            .get("https://api.openai.com/v1/organization/costs")
            .set("Authorization", &format!("Bearer {key}"))
            .query("start_time", &start.to_string())
            .query("limit", "31");
        if let Some(pg) = &page {
            req = req.query("page", pg);
        }
        let v = match req.call() {
            Ok(r) => r.into_json::<Value>().unwrap_or_default(),
            Err(ureq::Error::Status(401 | 403, _)) => {
                p.status = "bad_key".into();
                return p;
            }
            Err(_) => {
                p.status = "no_data".into();
                return p;
            }
        };
        for bucket in v["data"].as_array().into_iter().flatten() {
            for r in bucket["results"].as_array().into_iter().flatten() {
                total += as_number(&r["amount"]["value"]).unwrap_or(0.0);
            }
        }
        match (v["has_more"].as_bool(), v["next_page"].as_str()) {
            (Some(true), Some(n)) => page = Some(n.to_string()),
            _ => break,
        }
    }
    p.metrics = vec![budget_metric(total, cfg.v["budget"].as_f64(), next)];
    p.plan = "API".into();
    p.status.clear();
    p
}

fn budget_metric(spent: f64, budget: Option<f64>, next: i64) -> Metric {
    let budget = budget.filter(|b| *b > 0.0);
    Metric {
        label: "month".into(),
        pct: budget.map(|b| spent / b * 100.0),
        value: money(spent),
        detail: budget.map(|b| format!("{} / {}", money(spent), money(b))).unwrap_or_else(|| money(spent)),
        reset: Some(next),
    }
}

// ---------------------------------------------------------------- API de Anthropic (cost report, Admin API)
fn fetch_anthropic_api(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "anthropic_api".into(), ..prev.clone() };
    let Some(key) = cred_read(&cfg.id) else {
        p.status = "no_key".into();
        return p;
    };
    let (start, next) = month_bounds();
    let start_iso = {
        let d = start.div_euclid(86_400);
        // fecha civil del inicio de mes
        let z = d + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
        format!("{y:04}-{m:02}-01T00:00:00Z")
    };
    let mut total_cents = 0.0;
    let mut page: Option<String> = None;
    for _ in 0..6 {
        let mut req = a
            .get("https://api.anthropic.com/v1/organizations/cost_report")
            .set("x-api-key", &key)
            .set("anthropic-version", "2023-06-01")
            .query("starting_at", &start_iso)
            .query("limit", "31");
        if let Some(pg) = &page {
            req = req.query("page", pg);
        }
        let v = match req.call() {
            Ok(r) => r.into_json::<Value>().unwrap_or_default(),
            Err(ureq::Error::Status(401 | 403, _)) => {
                p.status = "bad_key".into();
                return p;
            }
            Err(_) => {
                p.status = "no_data".into();
                return p;
            }
        };
        for bucket in v["data"].as_array().into_iter().flatten() {
            for r in bucket["results"].as_array().into_iter().flatten() {
                // importe en la unidad mínima de la moneda (centavos), como cadena decimal
                total_cents += as_number(&r["amount"]).unwrap_or(0.0);
            }
        }
        match (v["has_more"].as_bool(), v["next_page"].as_str()) {
            (Some(true), Some(n)) => page = Some(n.to_string()),
            _ => break,
        }
    }
    p.metrics = vec![budget_metric(total_cents / 100.0, cfg.v["budget"].as_f64(), next)];
    p.plan = "API".into();
    p.status.clear();
    p
}

// ---------------------------------------------------------------- saldo de cuenta (DeepSeek, Kimi)
fn base_url(cfg: &ProviderCfg, default: &str) -> String {
    cfg.v["url"].as_str().map(str::trim).filter(|u| !u.is_empty()).unwrap_or(default).trim_end_matches('/').to_string()
}

fn cur_money(v: f64, cur: &str) -> String {
    let sym = match cur {
        "CNY" => "¥",
        "USD" => "$",
        _ => "",
    };
    if v >= 1000.0 { format!("{sym}{v:.0}") } else { format!("{sym}{v:.2}") }
}

/// Saldo restante; con un saldo de referencia (la última recarga) la barra muestra lo consumido
fn balance_metric(balance: f64, cur: &str, reference: Option<f64>) -> Metric {
    let reference = reference.filter(|r| *r > 0.0);
    Metric {
        label: "balance".into(),
        pct: reference.map(|r| ((1.0 - balance / r) * 100.0).clamp(0.0, 100.0)),
        value: cur_money(balance, cur),
        detail: reference.map(|r| format!("{} / {}", cur_money(balance, cur), cur_money(r, cur))).unwrap_or_default(),
        reset: None,
    }
}

/// Estado de una consulta fallida: clave rechazada, error del servidor o servidor inaccesible
fn http_status(e: ureq::Error) -> &'static str {
    match e {
        ureq::Error::Status(401 | 403, _) => "bad_key",
        ureq::Error::Status(..) => "no_data",
        _ => "offline",
    }
}

/// GET con la clave del proveedor como Bearer; Err(estado) con bad_key / no_data / offline / invalid
fn get_json(a: &ureq::Agent, url: &str, key: Option<&str>) -> Result<Value, &'static str> {
    let mut req = a.get(url);
    if let Some(k) = key {
        req = req.set("Authorization", &format!("Bearer {k}"));
    }
    req.call().map_err(http_status)?.into_json::<Value>().map_err(|_| "invalid")
}

/// Estado para un servicio en la nube: sin respuesta = sin conexión, no "servidor apagado"
fn cloud_status(e: &'static str) -> String {
    if e == "offline" { "no_data" } else { e }.into()
}

/// DeepSeek: `{"balance_infos":[{"currency":"USD","total_balance":"110.00",…}]}` → (saldo, moneda)
fn parse_deepseek(v: &Value) -> Option<(f64, String)> {
    v["balance_infos"]
        .as_array()?
        .iter()
        .filter_map(|b| Some((as_number(&b["total_balance"])?, b["currency"].as_str().unwrap_or_default().to_string())))
        .max_by(|a, b| a.0.total_cmp(&b.0))
}

fn fetch_deepseek(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "deepseek".into(), ..prev.clone() };
    let Some(key) = cred_read(&cfg.id) else {
        p.status = "no_key".into();
        return p;
    };
    match get_json(a, &format!("{}/user/balance", base_url(cfg, "https://api.deepseek.com")), Some(&key)) {
        Ok(v) => match parse_deepseek(&v) {
            Some((bal, cur)) => {
                p.metrics = vec![balance_metric(bal, &cur, cfg.v["budget"].as_f64())];
                p.plan = "API".into();
                p.status.clear();
            }
            None => p.status = "invalid".into(),
        },
        Err(e) => p.status = cloud_status(e),
    }
    p
}

/// Kimi (Moonshot AI): `{"data":{"available_balance":49.58,"voucher_balance":…,"cash_balance":…}}`
fn parse_kimi(v: &Value) -> Option<f64> {
    as_number(&v["data"]["available_balance"])
}

fn fetch_kimi(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "kimi".into(), ..prev.clone() };
    let Some(key) = cred_read(&cfg.id) else {
        p.status = "no_key".into();
        return p;
    };
    let base = base_url(cfg, "https://api.moonshot.ai");
    // la plataforma china (api.moonshot.cn) factura en yuanes; la internacional, en dólares
    let cur = if base.contains("moonshot.cn") { "CNY" } else { "USD" };
    match get_json(a, &format!("{base}/v1/users/me/balance"), Some(&key)) {
        Ok(v) => match parse_kimi(&v) {
            Some(bal) => {
                p.metrics = vec![balance_metric(bal, cur, cfg.v["budget"].as_f64())];
                p.plan = "API".into();
                p.status.clear();
            }
            None => p.status = "invalid".into(),
        },
        Err(e) => p.status = cloud_status(e),
    }
    p
}

// ---------------------------------------------------------------- Gemini (API de Google AI Studio)
fn count_metric(label: &str, n: usize) -> Metric {
    Metric { label: label.into(), pct: None, value: n.to_string(), detail: String::new(), reset: None }
}

/// Modelos de Gemini que generan contenido (se excluyen los de embeddings y similares)
fn parse_gemini(v: &Value) -> usize {
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["supportedGenerationMethods"].as_array().is_some_and(|g| g.iter().any(|x| x.as_str() == Some("generateContent"))))
        .count()
}

/// La API de Gemini no publica consumo ni cuota restante: se valida la clave y se cuentan los
/// modelos a los que da acceso
fn fetch_gemini(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "gemini".into(), ..prev.clone() };
    let Some(key) = cred_read(&cfg.id) else {
        p.status = "no_key".into();
        return p;
    };
    let url = format!("{}/v1beta/models", base_url(cfg, "https://generativelanguage.googleapis.com"));
    match a.get(&url).set("x-goog-api-key", &key).query("pageSize", "1000").call() {
        Ok(r) => match r.into_json::<Value>() {
            Ok(v) => {
                p.metrics = vec![count_metric("cloud_models", parse_gemini(&v))];
                p.plan = "API".into();
                p.status.clear();
            }
            Err(_) => p.status = "invalid".into(),
        },
        // una clave no válida se rechaza con 400 (API_KEY_INVALID), no con 401
        Err(ureq::Error::Status(400, _)) => p.status = "bad_key".into(),
        Err(e) => p.status = cloud_status(http_status(e)),
    }
    p
}

// ---------------------------------------------------------------- servidores de modelos (Ollama, LM Studio)
/// Indicador de modelos: valor "cargados/instalados" y, en el detalle, cuáles están cargados
fn models_metric(loaded: &[String], installed: Option<usize>, mem: f64) -> Metric {
    let mut detail = loaded.join(", ");
    if mem > 0.0 {
        detail = format!("{detail}  ·  {:.1} GB", mem / 1e9);
    }
    Metric {
        label: "models".into(),
        pct: None,
        value: installed.map(|n| format!("{}/{n}", loaded.len())).unwrap_or_else(|| loaded.len().to_string()),
        detail,
        reset: None,
    }
}

/// Ollama: `/api/ps` (modelos en memoria) y `/api/tags` (instalados)
fn parse_ollama(ps: &Value, tags: Option<&Value>) -> Metric {
    let models = ps["models"].as_array().cloned().unwrap_or_default();
    let loaded: Vec<String> = models.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect();
    let mem: f64 = models.iter().map(|m| m["size"].as_f64().unwrap_or(0.0)).sum();
    models_metric(&loaded, tags.and_then(|t| t["models"].as_array()).map(|a| a.len()), mem)
}

fn fetch_ollama(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "ollama".into(), ..prev.clone() };
    let base = base_url(cfg, "http://localhost:11434");
    let key = cred_read(&cfg.id);
    match get_json(a, &format!("{base}/api/ps"), key.as_deref()) {
        Ok(ps) => {
            let tags = get_json(a, &format!("{base}/api/tags"), key.as_deref()).ok();
            p.metrics = vec![parse_ollama(&ps, tags.as_ref())];
            p.plan = "local".into();
            p.status.clear();
        }
        Err(e) => {
            p.metrics.clear(); // sin servidor no hay modelos cargados: no se conserva lo anterior
            p.status = e.into();
        }
    }
    p
}

/// Ollama Cloud (ollama.com): no publica cuota de uso; se valida la clave y se cuentan los modelos
fn fetch_ollama_cloud(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "ollama_cloud".into(), ..prev.clone() };
    let Some(key) = cred_read(&cfg.id) else {
        p.status = "no_key".into();
        return p;
    };
    let base = base_url(cfg, "https://ollama.com");
    // /api/ps exige autenticación (a diferencia de /api/tags): sirve para comprobar la clave
    if let Err(e @ ("bad_key" | "offline")) = get_json(a, &format!("{base}/api/ps"), Some(&key)) {
        p.status = cloud_status(e);
        return p;
    }
    match get_json(a, &format!("{base}/api/tags"), Some(&key)) {
        Ok(t) => {
            p.metrics = vec![count_metric("cloud_models", t["models"].as_array().map(|a| a.len()).unwrap_or(0))];
            p.plan = "cloud".into();
            p.status.clear();
        }
        Err(e) => p.status = cloud_status(e),
    }
    p
}

/// LM Studio: `/api/v0/models` → `{"data":[{"id":"…","state":"loaded"|"not-loaded",…}]}`
fn parse_lmstudio(v: &Value) -> Metric {
    let models = v["data"].as_array().cloned().unwrap_or_default();
    let loaded: Vec<String> = models.iter().filter(|m| m["state"].as_str() == Some("loaded")).filter_map(|m| m["id"].as_str().map(String::from)).collect();
    models_metric(&loaded, Some(models.len()), 0.0)
}

fn fetch_lmstudio(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "lmstudio".into(), ..prev.clone() };
    let base = base_url(cfg, "http://localhost:1234");
    let key = cred_read(&cfg.id);
    let res = match get_json(a, &format!("{base}/api/v0/models"), key.as_deref()) {
        Ok(v) => Ok(parse_lmstudio(&v)),
        // versiones sin la API REST propia: la compatible con OpenAI solo lista los modelos
        Err("no_data") => get_json(a, &format!("{base}/v1/models"), key.as_deref())
            .map(|v| count_metric("models_available", v["data"].as_array().map(|a| a.len()).unwrap_or(0))),
        Err(e) => Err(e),
    };
    match res {
        Ok(m) => {
            p.metrics = vec![m];
            p.plan = "local".into();
            p.status.clear();
        }
        Err(e) => {
            p.metrics.clear();
            p.status = e.into();
        }
    }
    p
}

// ---------------------------------------------------------------- personalizado (URL JSON)
fn fetch_custom(a: &ureq::Agent, cfg: &ProviderCfg, prev: &Provider) -> Provider {
    let mut p = Provider { kind: "custom".into(), ..prev.clone() };
    let Some(url) = cfg.v["url"].as_str() else {
        p.status = "no_url".into();
        return p;
    };
    let mut req = a.get(url);
    if let Some(tok) = cred_read(&cfg.id) {
        req = req.set("Authorization", &format!("Bearer {tok}"));
    }
    let v = match req.call().ok().and_then(|r| r.into_json::<Value>().ok()) {
        Some(v) => v,
        None => {
            p.status = "no_data".into();
            return p;
        }
    };
    let pct = cfg.v["percentField"].as_str().and_then(|f| json_path(&v, f)).and_then(as_number);
    let value = cfg.v["valueField"]
        .as_str()
        .and_then(|f| json_path(&v, f))
        .map(|x| x.as_str().map(|s| s.to_string()).unwrap_or_else(|| x.to_string()))
        .or_else(|| pct.map(|p| format!("{p:.0}%")))
        .unwrap_or("—".into());
    let label = cfg.v["labelField"]
        .as_str()
        .and_then(|f| json_path(&v, f))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "usage".into());
    p.metrics = vec![Metric { label, pct, value: value.clone(), detail: value, reset: None }];
    p.status.clear();
    p
}

// ---------------------------------------------------------------- caché y trabajador
fn cache_path() -> std::path::PathBuf {
    crate::util::appdata_dir().join("ai_cache.json")
}

fn save_cache(last: &HashMap<String, i64>) {
    let v = json!({ "providers": *AI.lock().unwrap(), "fetched": last });
    let _ = std::fs::write(cache_path(), v.to_string());
}

fn load_cache() -> HashMap<String, i64> {
    let Some(v) = std::fs::read_to_string(cache_path()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
        return HashMap::new();
    };
    if let Ok(list) = serde_json::from_value::<Vec<Provider>>(v["providers"].clone()) {
        *AI.lock().unwrap() = list;
    }
    serde_json::from_value(v["fetched"].clone()).unwrap_or_default()
}

/// Intervalo de consulta por tipo (s): respeta los límites de cada API
fn every(kind: &str) -> i64 {
    match kind {
        "claude" => 180,
        "custom" => 120,
        "ollama" | "lmstudio" => 30, // servidores locales: lo cargado cambia a menudo
        _ => 300,
    }
}

pub fn spawn_worker() {
    let (tx, rx) = channel();
    *REFRESH.lock().unwrap() = Some(tx);
    std::thread::spawn(move || {
        crate::util::prof("ai");
        let a = agent();
        let mut gh_token = None;
        let mut last = load_cache();
        loop {
            let cfg = Config::load();
            let list = providers(&cfg);
            let now = now_epoch();
            let mut changed = false;
            for pc in list.iter().filter(|p| p.enabled) {
                if now - last.get(&pc.id).copied().unwrap_or(0) < every(&pc.kind) {
                    continue;
                }
                let prev = AI.lock().unwrap().iter().find(|p| p.id == pc.id).cloned().unwrap_or(Provider {
                    id: pc.id.clone(),
                    kind: pc.kind.clone(),
                    name: pc.name.clone(),
                    ..Default::default()
                });
                let mut p = match pc.kind.as_str() {
                    "claude" => fetch_claude(&a, &prev),
                    "copilot" => fetch_copilot(&a, &prev, &mut gh_token),
                    "openai" => fetch_openai(&a, pc, &prev),
                    "anthropic_api" => fetch_anthropic_api(&a, pc, &prev),
                    "deepseek" => fetch_deepseek(&a, pc, &prev),
                    "kimi" => fetch_kimi(&a, pc, &prev),
                    "gemini" => fetch_gemini(&a, pc, &prev),
                    "ollama" => fetch_ollama(&a, pc, &prev),
                    "ollama_cloud" => fetch_ollama_cloud(&a, pc, &prev),
                    "lmstudio" => fetch_lmstudio(&a, pc, &prev),
                    "custom" => fetch_custom(&a, pc, &prev),
                    _ => continue,
                };
                p.id = pc.id.clone();
                p.name = pc.name.clone();
                p.fetched_at = now;
                let mut ai = AI.lock().unwrap();
                match ai.iter_mut().find(|x| x.id == p.id) {
                    Some(x) => *x = p,
                    None => ai.push(p),
                }
                drop(ai);
                last.insert(pc.id.clone(), now);
                changed = true;
            }
            // olvidar proveedores eliminados de la configuración
            AI.lock().unwrap().retain(|p| list.iter().any(|c| c.id == p.id));
            if changed {
                save_cache(&last);
            }
            if rx.recv_timeout(Duration::from_secs(20)).is_ok() {
                last.clear(); // actualización manual (o proveedor nuevo): consultar todo ya
            }
        }
    });
}

// ---------------------------------------------------------------- agregar / quitar proveedores
/// Formulario (WinForms con estilos de Windows) para los datos de un proveedor nuevo; devuelve JSON
const ADD_FORM: &str = include_str!("../assets/add-provider.ps1");

/// Abre el formulario en un hilo aparte; al aceptar guarda la clave en el Administrador de
/// credenciales, añade el proveedor a config.json y fuerza una consulta
/// Muestra el formulario (bloqueante) y devuelve lo introducido; None si se cancela
fn run_form(kind: &str) -> Option<Value> {
    let script = std::env::temp_dir().join("taskbar-panels-add-provider.ps1");
    std::fs::write(&script, ADD_FORM.as_bytes()).ok()?;
    let lang = if crate::i18n::es() { "es" } else { "en" };
    let out = hidden_cmd("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-STA", "-File"])
        .arg(&script)
        .args(["-Kind", kind, "-Lang", lang])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let _ = std::fs::remove_file(&script);
    let text = String::from_utf8_lossy(&out.ok()?.stdout).into_owned();
    let line = text.lines().rev().find(|l| l.trim_start().starts_with('{'))?.to_string(); // sin JSON = cancelado
    serde_json::from_str::<Value>(&line).ok()
}

/// Pide el token de GitHub para Copilot y lo guarda (vacío = borrarlo); fuerza una consulta
pub fn set_copilot_token_async() {
    std::thread::spawn(|| {
        let Some(r) = run_form("copilot") else { return };
        match r["key"].as_str().map(str::trim).filter(|k| !k.is_empty()) {
            Some(k) => {
                cred_write(COPILOT_TOKEN_ID, k);
            }
            None => cred_delete(COPILOT_TOKEN_ID),
        }
        refresh();
    });
}

pub fn add_provider_async(kind: &'static str) {
    std::thread::spawn(move || {
        let Some(r) = run_form(kind) else { return };

        let id = format!("{kind}-{}", now_epoch() % 1_000_000);
        if let Some(k) = r["key"].as_str().filter(|k| !k.is_empty()) {
            cred_write(&id, k);
        }
        let mut entry = json!({ "id": id, "type": kind, "enabled": true });
        for f in ["name", "url", "percentField", "valueField", "labelField"] {
            if let Some(s) = r[f].as_str().filter(|s| !s.is_empty()) {
                entry[f] = json!(s);
            }
        }
        if let Some(b) = r["budget"].as_str().and_then(|b| b.replace(',', ".").parse::<f64>().ok()).or_else(|| r["budget"].as_f64()) {
            entry["budget"] = json!(b);
        }
        let mut cfg = Config::load();
        let mut list = providers(&cfg).into_iter().map(|p| p.v).collect::<Vec<_>>();
        list.push(entry);
        cfg.v["AiProviders"] = json!(list);
        cfg.save_raw();
        refresh();
    });
}

/// Quita un proveedor (y su credencial)
pub fn remove_provider(cfg: &mut Config, id: &str) {
    let list: Vec<Value> = providers(cfg).into_iter().filter(|p| p.id != id).map(|p| p.v).collect();
    cfg.v["AiProviders"] = json!(list);
    if cfg.v["AiPrimary"].as_str() == Some(id) {
        cfg.v["AiPrimary"] = json!("all");
    }
    cred_delete(id);
}

/// Activa o desactiva un proveedor
pub fn toggle_provider(cfg: &mut Config, id: &str) {
    let list: Vec<Value> = providers(cfg)
        .into_iter()
        .map(|mut p| {
            if p.id == id {
                p.v["enabled"] = json!(!p.enabled);
                if p.v.get("id").is_none() {
                    p.v["id"] = json!(p.id);
                }
            }
            p.v
        })
        .collect();
    cfg.v["AiProviders"] = json!(list);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saldo_deepseek() {
        let v = json!({ "is_available": true, "balance_infos": [
            { "currency": "CNY", "total_balance": "0.00" },
            { "currency": "USD", "total_balance": "7.25", "granted_balance": "0.00", "topped_up_balance": "7.25" } ] });
        let (bal, cur) = parse_deepseek(&v).unwrap();
        assert_eq!((bal, cur.as_str()), (7.25, "USD"));
        let m = balance_metric(bal, &cur, Some(10.0));
        assert_eq!(m.value, "$7.25");
        assert_eq!(m.detail, "$7.25 / $10.00");
        assert!((m.pct.unwrap() - 27.5).abs() < 1e-6);
        assert!(parse_deepseek(&json!({ "error": {} })).is_none());
    }

    #[test]
    fn saldo_kimi_sin_referencia() {
        let v = json!({ "code": 0, "data": { "available_balance": 49.58, "voucher_balance": 46.58, "cash_balance": 3.0 }, "status": true });
        let m = balance_metric(parse_kimi(&v).unwrap(), "CNY", None);
        assert_eq!((m.value.as_str(), m.pct, m.detail.as_str()), ("¥49.58", None, ""));
    }

    #[test]
    fn saldo_por_encima_de_la_referencia_no_da_negativo() {
        assert_eq!(balance_metric(25.0, "USD", Some(10.0)).pct, Some(0.0));
    }

    #[test]
    fn modelos_gemini_solo_los_generativos() {
        let v = json!({ "models": [
            { "name": "models/gemini-pro", "supportedGenerationMethods": ["generateContent", "countTokens"] },
            { "name": "models/embedding", "supportedGenerationMethods": ["embedContent"] },
            { "name": "models/gemini-flash", "supportedGenerationMethods": ["generateContent"] } ] });
        assert_eq!(parse_gemini(&v), 2);
        assert_eq!(parse_gemini(&json!({})), 0);
    }

    #[test]
    fn modelos_ollama() {
        let ps = json!({ "models": [ { "name": "llama3.2:3b", "size": 3_500_000_000u64 }, { "name": "qwen3:8b", "size": 6_000_000_000u64 } ] });
        let tags = json!({ "models": [{}, {}, {}, {}] });
        let m = parse_ollama(&ps, Some(&tags));
        assert_eq!(m.value, "2/4");
        assert_eq!(m.detail, "llama3.2:3b, qwen3:8b  ·  9.5 GB");
        let idle = parse_ollama(&json!({ "models": [] }), None);
        assert_eq!((idle.value.as_str(), idle.detail.as_str()), ("0", ""));
    }

    #[test]
    fn modelos_lmstudio() {
        let v = json!({ "data": [ { "id": "qwen2.5-7b", "state": "loaded" }, { "id": "nomic-embed", "state": "not-loaded" }, { "id": "phi-4", "state": "not-loaded" } ] });
        let m = parse_lmstudio(&v);
        assert_eq!((m.value.as_str(), m.detail.as_str()), ("1/3", "qwen2.5-7b"));
    }
}
