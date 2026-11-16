//! Cuadro de búsqueda de la barra de tareas (Configuración → Personalización → Barra de tareas → Buscar).
//!
//! Con `AutoSearchBox` activo, cuando los paneles no caben entre los iconos de apps y el reloj el
//! cuadro se oculta (o se deja en icono) y se restaura al liberarse el espacio. El ajuste de Windows
//! es global (todas las pantallas). El modo original se guarda en `search.json` para restaurarlo
//! aunque la app se reinicie entre medias.
//!
//!   HKCU\Software\Microsoft\Windows\CurrentVersion\Search\SearchboxTaskbarMode
//!   0 = oculto · 1 = solo icono · 2 = cuadro de búsqueda · 3 = icono y etiqueta

use crate::util::{appdata_dir, wide};
use serde_json::json;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};

const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Search");
const VALUE: PCWSTR = w!("SearchboxTaskbarMode");

pub const HIDDEN: u32 = 0;
pub const ICON: u32 = 1;

/// Ancho (px lógicos) del botón de búsqueda en modo icono
pub const ICON_W: f32 = 44.0;

/// Modo actual del cuadro de búsqueda (None si la clave no existe o no se puede leer)
pub fn mode() -> Option<u32> {
    let (mut v, mut sz) = (0u32, 4u32);
    unsafe { RegGetValueW(HKEY_CURRENT_USER, KEY, VALUE, RRF_RT_REG_DWORD, None, Some(&mut v as *mut u32 as *mut _), Some(&mut sz)) }.is_ok().then_some(v)
}

/// Cambia el modo y avisa a Explorer (en otro hilo: la difusión espera a las ventanas lentas)
pub fn set_mode(m: u32) -> bool {
    let ok = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, KEY, VALUE, REG_DWORD.0, Some(&m as *const u32 as *const _), 4) }.is_ok();
    if ok {
        std::thread::spawn(|| unsafe {
            let s = wide("TraySettings");
            let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(s.as_ptr() as isize), SMTO_ABORTIFHUNG, 1000, None);
        });
    }
    ok
}

/// ¿Es un modo "ancho" (cuadro, o icono con etiqueta) que se puede recoger?
pub fn is_box(m: u32) -> bool {
    m == 2 || m == 3
}

/// Estado mientras el cuadro está recogido por la app
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saved {
    /// modo original que hay que restaurar
    pub mode: u32,
    /// modo que puso la app (si ya no es el actual, el usuario lo cambió a mano)
    pub to: u32,
    /// ancho del cuadro original en px lógicos
    pub width: f32,
}

pub fn saved() -> Option<Saved> {
    let s = std::fs::read_to_string(appdata_dir().join("search.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    Some(Saved { mode: v["Mode"].as_u64()? as u32, to: v["To"].as_u64()? as u32, width: v["Width"].as_f64().unwrap_or(220.0) as f32 })
}

pub fn save(s: &Saved) {
    let _ = std::fs::write(appdata_dir().join("search.json"), json!({ "Mode": s.mode, "To": s.to, "Width": s.width }).to_string());
}

pub fn clear() {
    let _ = std::fs::remove_file(appdata_dir().join("search.json"));
}

/// Devuelve el cuadro a su modo original si lo recogió la app y sigue como ella lo dejó
pub fn restore() {
    if let Some(s) = saved() {
        if mode() == Some(s.to) {
            set_mode(s.mode);
        }
        clear();
    }
}

/// Espacio de una barra para los paneles de la derecha (px físicos)
#[derive(Clone, Copy, Debug)]
pub struct Room {
    /// lo que ocupan los paneles en modo icono con su separación normal
    pub need: i32,
    /// hueco entre los iconos de apps (con su margen) y la bandeja
    pub avail: i32,
    pub scale: f32,
    /// iconos alineados a la izquierda: el cuadro desplaza los iconos todo su ancho (centrados: la mitad)
    pub left_aligned: bool,
}

/// ¿Falta sitio en alguna barra? (los paneles no caben ni como iconos)
pub fn lacks_room(rooms: &[Room]) -> bool {
    rooms.iter().any(|r| r.need > r.avail)
}

/// ¿Cabría de nuevo el cuadro en todas las barras? `gain` = px lógicos que recupera el cuadro al
/// volver. Se exige además el ancho de un icono de app de holgura para no ir y venir al abrir una.
pub fn fits_again(rooms: &[Room], gain: f32) -> bool {
    rooms.iter().all(|r| {
        let shift = gain * r.scale * if r.left_aligned { 1.0 } else { 0.5 };
        r.need + (44.0 * r.scale) as i32 <= r.avail - shift.ceil() as i32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recoger_y_restaurar_con_holgura() {
        // portátil a 150 %: 4 iconos (327 px) en un hueco de 235 → falta sitio
        let lleno = [Room { need: 327, avail: 235, scale: 1.5, left_aligned: false }, Room { need: 327, avail: 700, scale: 1.5, left_aligned: false }];
        assert!(lacks_room(&lleno));
        // recogido el cuadro (220 px lógicos → los iconos centrados se corren 165 px): hueco de 400
        let recogido = [Room { need: 327, avail: 400, scale: 1.5, left_aligned: false }];
        assert!(!lacks_room(&recogido));
        // restaurarlo lo dejaría otra vez en 235: no vuelve hasta que haya sitio con holgura
        assert!(!fits_again(&recogido, 220.0));
        assert!(!fits_again(&[Room { avail: 327 + 165 + 60, ..recogido[0] }], 220.0));
        assert!(fits_again(&[Room { avail: 327 + 165 + 66, ..recogido[0] }], 220.0));
        // iconos a la izquierda: el cuadro ocupa todo su ancho
        assert!(!fits_again(&[Room { avail: 327 + 165 + 66, left_aligned: true, ..recogido[0] }], 220.0));
        assert!(fits_again(&[Room { avail: 327 + 330 + 66, left_aligned: true, ..recogido[0] }], 220.0));
    }
}
