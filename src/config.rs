//! Configuración en %APPDATA%\TaskbarPanels\config.json y separación del panel de red en pos.json.
//!
//!   Displays             : "primary" | "all" | ["SAM7B0C", ...]
//!   PanelDisplays        : { "MediaPanel": ["SDC420B"], ... }   (opcional, por panel)
//!                          Paneles: NetworkPanel, UsagePanel, SystemPanel, MediaPanel, RadioPanel
//!   Panels               : { "MediaPanel": false, ... }         (opcional, activar/desactivar)
//!   SecondaryRightMargin : px lógicos reservados al reloj en barras secundarias (respaldo: el
//!                          tamaño real se mide en cada pantalla, ver host.rs)
//!   MonitorIds           : código → nombre (informativo)
//!   AutoSearchBox        : true | "hide" | "icon" → recoger el cuadro de búsqueda si falta espacio

use crate::util::appdata_dir;
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant};

pub struct Config {
    pub v: Value,
    at: Instant,
}

fn read_json(name: &str) -> Option<Value> {
    let s = std::fs::read_to_string(appdata_dir().join(name)).ok()?;
    serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()
}

impl Config {
    pub fn load() -> Self {
        let mut v = read_json("config.json").unwrap_or_else(|| json!({}));
        if !v.is_object() {
            v = json!({});
        }
        let o = v.as_object_mut().unwrap();
        o.entry("Displays").or_insert(json!("primary"));
        o.entry("SecondaryRightMargin").or_insert(json!(130));
        Config { v, at: Instant::now() }
    }

    /// Relee el archivo como mucho cada 5 s (los cambios manuales se aplican en caliente)
    pub fn refresh(&mut self) {
        if self.at.elapsed() > Duration::from_secs(5) {
            *self = Config::load();
        }
    }

    pub fn save(&mut self, monitors: &[(String, String, bool)]) {
        let mut ids = Map::new();
        for (id, name, primary) in monitors {
            let n = if *primary { format!("{name} (principal)") } else { name.clone() };
            ids.insert(id.clone(), json!(n));
        }
        self.v["MonitorIds"] = Value::Object(ids);
        if let Ok(s) = serde_json::to_string_pretty(&self.v) {
            let _ = std::fs::write(appdata_dir().join("config.json"), s);
        }
        self.at = Instant::now();
    }

    /// Guarda config.json tal cual (sin recalcular MonitorIds); usado desde otros hilos
    pub fn save_raw(&mut self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.v) {
            let _ = std::fs::write(appdata_dir().join("config.json"), s);
        }
        self.at = Instant::now();
    }

    pub fn selection(&self, panel: &str) -> &Value {
        let pd = self.v.get("PanelDisplays");
        pd.and_then(|p| p.get(panel))
            .or_else(|| pd.and_then(|p| p.get(legacy_name(panel))))
            .unwrap_or(&self.v["Displays"])
    }

    pub fn selected(&self, panel: &str, primary: bool, id: &str) -> bool {
        match self.selection(panel) {
            Value::String(s) if s == "all" => true,
            Value::String(s) if s == "primary" => primary,
            Value::String(s) => s == id,
            Value::Array(a) => a.iter().any(|x| x.as_str() == Some(id)),
            _ => primary,
        }
    }

    pub fn enabled(&self, panel: &str) -> bool {
        let p = self.v.get("Panels");
        p.and_then(|p| p.get(panel))
            .or_else(|| p.and_then(|p| p.get(legacy_name(panel))))
            .and_then(|b| b.as_bool())
            .unwrap_or(true)
    }

    pub fn set_enabled(&mut self, panel: &str, on: bool) {
        if !self.v.get("Panels").map(|p| p.is_object()).unwrap_or(false) {
            self.v["Panels"] = json!({});
        }
        self.v["Panels"][panel] = json!(on);
    }

    pub fn margin(&self) -> f32 {
        self.v["SecondaryRightMargin"].as_f64().unwrap_or(130.0) as f32
    }

    /// Nombre conocido del monitor (de MonitorIds), sin el sufijo " (principal)"
    pub fn monitor_name(&self, id: &str) -> String {
        self.v
            .get("MonitorIds")
            .and_then(|m| m.get(id))
            .and_then(|n| n.as_str())
            .map(|n| n.trim_end_matches(" (principal)").to_string())
            .unwrap_or_else(|| id.to_string())
    }
}

/// Nombre que usaba la versión anterior para el mismo panel (compatibilidad de config.json)
fn legacy_name(panel: &str) -> &str {
    if panel == "NetworkPanel" { "PublicIpTray" } else { panel }
}

/// Separación de cada panel de la derecha (red, IA, sistema, radio) respecto al borde derecho útil de la
/// barra, en px lógicos. Migra el antiguo `Gap` único (toda la cadena se movía junta).
pub fn load_offsets() -> [i32; 4] {
    let v = read_json("pos.json");
    let gap = v.as_ref().and_then(|v| v["Gap"].as_i64()).unwrap_or(8) as i32;
    let o = v.as_ref().map(|v| v["Offsets"].clone()).unwrap_or(Value::Null);
    let get = |k: &str| o[k].as_i64().map(|x| x.max(0) as i32).unwrap_or(gap);
    [get("NetworkPanel"), get("UsagePanel"), get("SystemPanel"), get("RadioPanel")]
}

pub fn save_offsets(o: &[i32; 4]) {
    let v = json!({ "Offsets": { "NetworkPanel": o[0], "UsagePanel": o[1], "SystemPanel": o[2], "RadioPanel": o[3] } });
    let _ = std::fs::write(appdata_dir().join("pos.json"), v.to_string());
}
