//! Idioma de la interfaz: español si Windows está en español; inglés en cualquier otro caso.
//! Se puede forzar con `"Language": "es" | "en"` en config.json (o desde el menú *Idioma*).

use std::sync::atomic::{AtomicU8, Ordering};

/// 0 = sin inicializar, 1 = español, 2 = inglés
static LANG: AtomicU8 = AtomicU8::new(0);

/// ¿La interfaz de Windows está en español? (LANG_SPANISH = 0x0A en el idioma principal)
pub fn system_is_spanish() -> bool {
    unsafe { (windows::Win32::Globalization::GetUserDefaultUILanguage() & 0x3FF) == 0x0A }
}

/// Fija el idioma según la configuración: "es", "en" o automático (idioma del sistema)
pub fn init(setting: Option<&str>) {
    let es = match setting {
        Some("es") => true,
        Some("en") => false,
        _ => system_is_spanish(),
    };
    LANG.store(if es { 1 } else { 2 }, Ordering::Relaxed);
}

pub fn es() -> bool {
    match LANG.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => system_is_spanish(),
    }
}

/// Texto en el idioma activo
pub fn tr(es: &'static str, en: &'static str) -> &'static str {
    if self::es() { es } else { en }
}
