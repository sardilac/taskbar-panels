//! Utilidades: cadenas UTF-16, colores, fechas ISO-8601, procesos ocultos y rutas.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn now_epoch() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "2026-11-13T23:00:00.004763-06:00", "2026-12-01T00:00:00Z" o "2026-12-01" → segundos Unix
pub fn parse_iso(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let mut secs = days_from_civil(y, mo, d) * 86_400;
    if s.len() >= 19 {
        secs += n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?;
        let mut rest = &s[19..];
        if rest.starts_with('.') {
            rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
        }
        if rest.starts_with('+') || rest.starts_with('-') {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let h: i64 = rest.get(1..3)?.parse().ok()?;
            let m: i64 = rest.get(4..6)?.parse().ok()?;
            secs -= sign * (h * 3600 + m * 60);
        }
    }
    Some(secs)
}

/// Tiempo restante legible: "ahora", "4h05", "5d 3h"
pub fn fmt_left(target: i64) -> String {
    let d = target - now_epoch();
    if d < 60 {
        crate::i18n::tr("ahora", "now").into()
    } else if d < 86_400 {
        format!("{}h{:02}", d / 3600, (d % 3600) / 60)
    } else {
        format!("{}d {}h", d / 86_400, (d % 86_400) / 3600)
    }
}

pub fn hidden_cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Abre una URL con el shell del usuario (explorer.exe la abre sin elevación)
pub fn open_url(url: &str) {
    let _ = Command::new("explorer.exe").arg(url).spawn();
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

/// Carpeta de configuración: %APPDATA%\TaskbarPanels. Si no existe y hay una de la versión
/// anterior (%APPDATA%\PublicIpTray), se copian sus archivos para conservar los ajustes.
pub fn appdata_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let base = PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()));
        let p = base.join("TaskbarPanels");
        if !p.exists() {
            let _ = std::fs::create_dir_all(&p);
            let old = base.join("PublicIpTray");
            for f in ["config.json", "pos.json", "usage_cache.json"] {
                let _ = std::fs::copy(old.join(f), p.join(f));
            }
        }
        p
    })
    .clone()
}

pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
}

/// Fecha/hora local de un instante Unix: (día de la semana 0=dom, día, mes, hora, minuto)
pub fn local_parts(epoch: i64) -> Option<(u16, u16, u16, u16, u16)> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ft_val = (epoch * 10_000_000 + 116_444_736_000_000_000) as u64;
    let ft = FILETIME { dwLowDateTime: ft_val as u32, dwHighDateTime: (ft_val >> 32) as u32 };
    unsafe {
        let mut utc = SYSTEMTIME::default();
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        let mut loc = SYSTEMTIME::default();
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut loc).ok()?;
        Some((loc.wDayOfWeek, loc.wDay, loc.wMonth, loc.wHour, loc.wMinute))
    }
}

/// "23:00" (hoy) o "jue 19 04:00"
pub fn fmt_local(epoch: i64) -> String {
    let Some((dow, d, _m, h, mi)) = local_parts(epoch) else { return String::new() };
    let today = local_parts(now_epoch()).map(|p| p.1) == Some(d) && epoch - now_epoch() < 86_400;
    if today {
        format!("{h:02}:{mi:02}")
    } else {
        let dn = if crate::i18n::es() { ["dom", "lun", "mar", "mié", "jue", "vie", "sáb"] } else { ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] }[dow as usize % 7];
        format!("{dn} {d} {h:02}:{mi:02}")
    }
}

/// "01/12" (español) o "Dec 1" (inglés)
pub fn fmt_date(epoch: i64) -> String {
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    local_parts(epoch)
        .map(|(_, d, m, _, _)| if crate::i18n::es() { format!("{d:02}/{m:02}") } else { format!("{} {d}", MON[(m as usize).clamp(1, 12) - 1]) })
        .unwrap_or_default()
}
// ---------------------------------------------------------------- diagnóstico: CPU por hilo
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThread() -> isize;
    fn GetCurrentProcess() -> isize;
    fn DuplicateHandle(sp: isize, sh: isize, dp: isize, out: *mut isize, access: u32, inherit: i32, opts: u32) -> i32;
    fn GetThreadTimes(h: isize, creation: *mut u64, exit: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
}

/// Hilos registrados: nombre, handle y tiempo de CPU (100 ns) en el último volcado
static THREADS: std::sync::Mutex<Vec<(&'static str, isize, u64)>> = std::sync::Mutex::new(Vec::new());

/// Registra el hilo actual con un nombre para el volcado de CPU por hilo
pub fn prof(name: &'static str) {
    unsafe {
        let mut h = 0isize;
        if DuplicateHandle(GetCurrentProcess(), GetCurrentThread(), GetCurrentProcess(), &mut h, 0, 0, 2) != 0 {
            THREADS.lock().unwrap().push((name, h, 0));
        }
    }
}

/// CPU gastada por cada hilo registrado desde el volcado anterior, en ms ("ui=120 sys=4 …")
pub fn prof_dump() -> String {
    let mut out = Vec::new();
    for t in THREADS.lock().unwrap().iter_mut() {
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        if unsafe { GetThreadTimes(t.1, &mut c, &mut e, &mut k, &mut u) } != 0 {
            let total = k + u;
            out.push(format!("{}={}", t.0, (total - t.2.min(total)) / 10_000));
            t.2 = total;
        }
    }
    out.join(" ")
}

thread_local! {
    /// tiempo acumulado por sección del hilo de interfaz (nombre, µs, veces) desde el último volcado
    static SECTIONS: std::cell::RefCell<Vec<(&'static str, u64, u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Mide una sección del hilo de interfaz para el diagnóstico
pub fn timed<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    let t = std::time::Instant::now();
    let r = f();
    let us = t.elapsed().as_micros() as u64;
    SECTIONS.with(|s| {
        let mut s = s.borrow_mut();
        match s.iter_mut().find(|e| e.0 == name) {
            Some(e) => {
                e.1 += us;
                e.2 += 1;
            }
            None => s.push((name, us, 1)),
        }
    });
    r
}

/// Secciones del hilo de interfaz desde el volcado anterior: "nombre=ms/veces"
pub fn sections_dump() -> String {
    SECTIONS.with(|s| s.borrow_mut().drain(..).map(|(n, us, k)| format!("{n}={}/{k}", us / 1000)).collect::<Vec<_>>().join(" "))
}
