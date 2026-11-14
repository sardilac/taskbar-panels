//! Detección de barras de tareas (principal y secundarias), código de monitor (EDID),
//! escala por monitor y detección de pantalla completa.

use crate::util::wide;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Debug)]
pub struct Taskbar {
    pub key: isize,
    pub hwnd: HWND,
    pub primary: bool,
    pub left: i32,
    pub top: i32,
    /// borde derecho útil: inicio de la bandeja (principal) o reloj (secundarias)
    pub right: i32,
    /// borde izquierdo del botón Inicio (los paneles de la izquierda no deben pasarlo)
    pub start_left: i32,
    /// borde derecho del área de iconos de aplicaciones (MSTaskListWClass)
    pub tasks_right: i32,
    /// iconos alineados a la izquierda (el botón Inicio está pegado al borde izquierdo)
    pub left_aligned: bool,
    pub height: i32,
    pub scale: f32,
    pub monitor_id: String,
    /// la barra está en el borde superior del monitor
    pub at_top: bool,
    /// rectángulo del monitor
    pub mon: RECT,
    /// barra vertical (izquierda o derecha) y su lado
    pub vertical: bool,
    pub side_left: bool,
    /// rectángulo de la barra
    pub rect: RECT,
    /// vertical: borde superior de la bandeja (los paneles se apilan por encima)
    pub tray_top: i32,
    /// vertical: borde inferior del último icono de aplicación
    pub tasks_bottom: i32,
    /// vertical: borde superior del primer icono (Inicio incluido)
    pub tasks_top: i32,
}

fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn rect_of(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    r
}

struct ChildSearch {
    class: &'static str,
    rect: Option<RECT>,
}

unsafe extern "system" fn child_cb(h: HWND, lp: LPARAM) -> BOOL {
    let s = &mut *(lp.0 as *mut ChildSearch);
    if class_of(h) == s.class {
        s.rect = Some(rect_of(h));
        return FALSE;
    }
    TRUE
}

/// Rectángulo de un descendiente por clase (búsqueda recursiva)
fn child_rect(parent: HWND, class: &'static str) -> Option<RECT> {
    let mut s = ChildSearch { class, rect: None };
    unsafe {
        let _ = EnumChildWindows(parent, Some(child_cb), LPARAM(&mut s as *mut _ as isize));
    }
    s.rect
}

/// Código de hardware del monitor de la ventana, p. ej. "SAM7B0C" (de MONITOR\SAM7B0C\{guid}\0004)
pub fn monitor_id(h: HWND) -> String {
    unsafe {
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(mon, &mut mi as *mut _ as *mut MONITORINFO).as_bool() {
            return String::new();
        }
        let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        let dev = PCWSTR(mi.szDevice.as_ptr());
        let device_name = String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').to_string();
        if !EnumDisplayDevicesW(dev, 0, &mut dd, 0).as_bool() {
            return device_name;
        }
        let id = String::from_utf16_lossy(&dd.DeviceID);
        let id = id.trim_end_matches('\0');
        id.split('\\').nth(1).map(|s| s.to_string()).unwrap_or(device_name)
    }
}

// ---------------------------------------------------------------- observador de iconos (UI Automation)
// Las ventanas Win32 heredadas (Start, MSTaskListWClass, TrayNotifyWnd) no siguen bien al taskbar XAML
// de Windows 11. Un hilo en segundo plano consulta UI Automation cada 0,7 s y deja aquí, por barra,
// la extensión real de los iconos (botón Inicio + aplicaciones) y el borde superior de la bandeja.
// Así el hilo de la interfaz nunca se bloquea en llamadas UIA.

#[derive(Clone, Copy)]
struct IconInfo {
    icons: RECT,
    tray_top: Option<i32>,
}

static ICONS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<isize, IconInfo>>> =
    std::sync::LazyLock::new(Default::default);
/// barras a observar: (hwnd, vertical)
static WATCH: std::sync::LazyLock<std::sync::Mutex<Vec<(isize, bool)>>> = std::sync::LazyLock::new(Default::default);

unsafe fn uia_extent(uia: &windows::Win32::UI::Accessibility::IUIAutomation, hw: HWND, vertical: bool) -> Option<IconInfo> {
    use windows::Win32::UI::Accessibility::*;
    let el = uia.ElementFromHandle(hw).ok()?;
    let all = el.FindAll(TreeScope_Descendants, &uia.CreateTrueCondition().ok()?).ok()?;
    let mut icons: Option<RECT> = None;
    let mut tray_top: Option<i32> = None;
    for i in 0..all.Length().ok()? {
        let Ok(e) = all.GetElement(i) else { continue };
        let cls = e.CurrentClassName().map(|b| b.to_string()).unwrap_or_default();
        let is_icon = cls == "Taskbar.TaskListButtonAutomationPeer"
            || e.CurrentAutomationId().map(|b| b.to_string() == "StartButton").unwrap_or(false);
        let is_tray = vertical && cls.starts_with("SystemTray.");
        if !is_icon && !is_tray {
            continue;
        }
        let Ok(r) = e.CurrentBoundingRectangle() else { continue };
        if r.right <= r.left || r.bottom <= r.top {
            continue;
        }
        if is_icon {
            icons = Some(match icons {
                None => r,
                Some(x) => RECT { left: x.left.min(r.left), top: x.top.min(r.top), right: x.right.max(r.right), bottom: x.bottom.max(r.bottom) },
            });
        } else {
            tray_top = Some(tray_top.map_or(r.top, |t| t.min(r.top)));
        }
    }
    icons.map(|icons| IconInfo { icons, tray_top })
}

pub fn spawn_icons_watcher() {
    std::thread::spawn(|| unsafe {
        use windows::Win32::System::Com::*;
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, windows::Win32::UI::Accessibility::IUIAutomation>(
            &windows::Win32::UI::Accessibility::CUIAutomation,
            None,
            CLSCTX_INPROC_SERVER,
        ) else {
            return;
        };
        loop {
            let bars = WATCH.lock().unwrap().clone();
            let mut fresh = std::collections::HashMap::new();
            for (hw, vertical) in bars {
                if let Some(info) = uia_extent(&uia, HWND(hw as *mut _), vertical) {
                    fresh.insert(hw, info);
                }
            }
            *ICONS.lock().unwrap() = fresh;
            std::thread::sleep(std::time::Duration::from_millis(700));
        }
    });
}
fn find_window(class: &str, after: HWND) -> HWND {
    let c = wide(class);
    unsafe { FindWindowExW(HWND::default(), after, PCWSTR(c.as_ptr()), PCWSTR::null()).unwrap_or_default() }
}

/// Todas las barras visibles (sin filtrar por la configuración)
pub fn taskbars(secondary_margin: f32) -> Vec<Taskbar> {
    let mut hwnds: Vec<(HWND, bool)> = Vec::new();
    let p = find_window("Shell_TrayWnd", HWND::default());
    if !p.is_invalid() {
        hwnds.push((p, true));
    }
    let mut h = HWND::default();
    loop {
        h = find_window("Shell_SecondaryTrayWnd", h);
        if h.is_invalid() {
            break;
        }
        if unsafe { IsWindowVisible(h).as_bool() } {
            hwnds.push((h, false));
        }
    }
    let mut out = Vec::new();
    let mut watch = Vec::new();
    for (hw, primary) in hwnds {
        let r = rect_of(hw);
        if r.right - r.left <= 0 || r.bottom - r.top <= 0 {
            continue;
        }
        let dpi = unsafe { GetDpiForWindow(hw) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        let right = if primary {
            let c = wide("TrayNotifyWnd");
            let tray = unsafe { FindWindowExW(hw, HWND::default(), PCWSTR(c.as_ptr()), PCWSTR::null()).unwrap_or_default() };
            if tray.is_invalid() { r.right } else { rect_of(tray).left }
        } else {
            r.right - (secondary_margin * scale) as i32
        };
        let start = child_rect(hw, "Start").filter(|s| s.left >= r.left && s.right > s.left);
        // borde izquierdo real de los iconos (Inicio incluido): de UI Automation si está disponible
        let start_left = ICONS.lock().unwrap().get(&(hw.0 as isize)).map(|u| u.icons.left)
            .or(start.map(|s| s.left))
            .filter(|&x| x > r.left)
            .unwrap_or(r.right);
        let left_aligned = (r.right - r.left) >= (r.bottom - r.top) && (start_left - r.left) as f32 <= 40.0 * scale;
        let vertical = (r.bottom - r.top) > (r.right - r.left);
        watch.push((hw.0 as isize, vertical));
        let ui = ICONS.lock().unwrap().get(&(hw.0 as isize)).copied();
        let uia = ui.map(|u| u.icons);
        let tasks_right = uia.map(|e| e.right)
            .or_else(|| child_rect(hw, "MSTaskListWClass").filter(|t| t.right > t.left).map(|t| t.right))
            .or(start.map(|s| s.right))
            .unwrap_or(r.left);

        let mon = unsafe {
            let m = MonitorFromWindow(hw, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(m, &mut mi);
            mi.rcMonitor
        };
        let at_top = !vertical && (r.top - mon.top) < (mon.bottom - r.bottom);
        let side_left = vertical && (r.left - mon.left) < (mon.right - r.right);
        let tray_top = if vertical {
            let c = wide("TrayNotifyWnd");
            let tray = unsafe { FindWindowExW(hw, HWND::default(), PCWSTR(c.as_ptr()), PCWSTR::null()).unwrap_or_default() };
            let legacy = if tray.is_invalid() { None } else { Some(rect_of(tray).top) };
            // aceptar solo valores en la mitad inferior de la barra; si no, reservar ~170 px lógicos
            let mid = (r.top + r.bottom) / 2;
            ui.and_then(|u| u.tray_top)
                .filter(|&y| y > mid && y <= r.bottom)
                .or(legacy.filter(|&y| y > mid && y <= r.bottom))
                .unwrap_or(r.bottom - (170.0 * scale) as i32)
        } else {
            r.bottom
        };
        let tasks_top = if vertical { uia.map(|e| e.top).or(start.map(|s| s.top)).unwrap_or(r.top) } else { r.top };
        let tasks_bottom = if vertical {
            uia.map(|e| e.bottom)
                .or_else(|| child_rect(hw, "MSTaskListWClass").filter(|t| t.bottom > t.top).map(|t| t.bottom))
                .or(start.map(|s| s.bottom))
                .unwrap_or(r.top)
        } else {
            r.top
        };
        out.push(Taskbar {
            key: hw.0 as isize,
            hwnd: hw,
            primary,
            left: r.left,
            top: r.top,
            right,
            start_left,
            tasks_right,
            left_aligned,
            height: r.bottom - r.top,
            scale,
            monitor_id: monitor_id(hw),
            at_top,
            mon,
            vertical,
            side_left,
            rect: r,
            tray_top,
            tasks_bottom,
            tasks_top,
        });
    }
    *WATCH.lock().unwrap() = watch;
    out
}

struct FsSearch {
    mon: RECT,
    fg: HWND,
    hide: bool,
    reason: String,
    /// proceso del shell (dueño de la barra): sus ventanas nunca cuentan como pantalla completa
    shell_pid: u32,
}

unsafe extern "system" fn fs_cb(h: HWND, lp: LPARAM) -> BOOL {
    let s = &mut *(lp.0 as *mut FsSearch);
    if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
        return TRUE;
    }
    let r = rect_of(h);
    let m = s.mon;
    if !(r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom) {
        return TRUE;
    }
    let topmost = (GetWindowLongW(h, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0) != 0;
    if h != s.fg && !topmost {
        return TRUE;
    }
    // Ventanas del shell (desplegable de iconos ocultos, calendario, menús de la barra...):
    // son "siempre encima" y pueden cubrir el monitor, pero no son pantalla completa
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, Some(&mut pid));
    if pid == s.shell_pid {
        return TRUE;
    }
    let c = class_of(h);
    if matches!(
        c.as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Windows.UI.Core.CoreWindow"
            | "TopLevelWindowForOverflowXamlIsland" | "Xaml_WindowedPopupClass" | "XamlExplorerHostIslandWindow"
            | "NotifyIconOverflowWindow" | "ForegroundStaging"
    ) {
        return TRUE;
    }
    // ventanas ocultas por DWM (apps UWP suspendidas, etc.): visibles para Win32 pero no en pantalla
    let mut cloaked = 0u32;
    if windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(
        h,
        windows::Win32::Graphics::Dwm::DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut _,
        4,
    )
    .is_ok()
        && cloaked != 0
    {
        return TRUE;
    }
    s.hide = true;
    s.reason = format!("ventana a pantalla completa: {c} (pid {pid}, {})", if h == s.fg { "primer plano" } else { "siempre encima" });
    FALSE
}

/// Motivo para ocultar los paneles de una barra (None = visibles): barra auto-ocultada o una ventana
/// a pantalla completa en su monitor que está en primer plano o es "siempre encima" (juegos, video, iCUE...)
pub fn hide_reason(tb: &Taskbar) -> Option<String> {
    unsafe {
        if !IsWindowVisible(tb.hwnd).as_bool() {
            return Some("barra no visible".into());
        }
        let mon = MonitorFromWindow(tb.hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return None;
        }
        // barra auto-ocultada (abajo o arriba)
        if tb.top >= mi.rcMonitor.bottom - 4 || (tb.at_top && tb.top + tb.height <= mi.rcMonitor.top + 4) {
            return Some("barra auto-ocultada".into());
        }
        let mut shell_pid = 0u32;
        GetWindowThreadProcessId(tb.hwnd, Some(&mut shell_pid));
        let mut s = FsSearch { mon: mi.rcMonitor, fg: GetForegroundWindow(), hide: false, reason: String::new(), shell_pid };
        let _ = EnumWindows(Some(fs_cb), LPARAM(&mut s as *mut _ as isize));
        if s.hide { Some(s.reason) } else { None }
    }
}
