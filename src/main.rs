//! taskbar-panels: paneles sobre la barra de tareas de Windows 11.
//!   · Red/IP        (↑ subida, ↓ bajada, IP pública)
//!   · Consumo de IA (Claude, Copilot, OpenAI, Anthropic, personalizados)
//!   · Sistema       (CPU, GPU, temperaturas, FPS)
//!   · Multimedia    (canción actual, botones, espectro neón)
//! Una ventana por panel y por barra seleccionada (config.json). Proceso único.

#![windows_subsystem = "windows"]

mod config;
mod ai;
mod host;
mod i18n;
mod media;
mod net;
mod radio;
mod render;
mod flags;
mod gpu;
mod speedtest;
mod state;
mod sys;
mod util;

use config::Config;
use host::Taskbar;
use i18n::tr;
use state::st;
use std::cell::RefCell;
use std::time::Instant;
use render::{Rgb, Surface, WHITE};
use util::wide;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ======================================================================= tipos
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Ip,
    Usage,
    System,
    Media,
    Radio,
}
const KINDS: [Kind; 5] = [Kind::Ip, Kind::Usage, Kind::System, Kind::Media, Kind::Radio];
/// Paneles de la derecha (se colocan en cadena y se pueden arrastrar)
const CHAIN: [Kind; 4] = [Kind::Ip, Kind::Usage, Kind::System, Kind::Radio];

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Ip => "NetworkPanel",
            Kind::Usage => "UsagePanel",
            Kind::System => "SystemPanel",
            Kind::Media => "MediaPanel",
            Kind::Radio => "RadioPanel",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Ip => tr("Red e IP pública", "Network and public IP"),
            Kind::Usage => tr("Consumo de IA", "AI usage"),
            Kind::System => tr("Sistema (CPU, GPU, FPS)", "System (CPU, GPU, FPS)"),
            Kind::Media => tr("Multimedia", "Media"),
            Kind::Radio => tr("Radio (emisoras)", "Radio (stations)"),
        }
    }
    /// ancho en px lógicos (96 dpi)
    fn width(self) -> f32 {
        match self {
            Kind::Ip => 96.0,
            Kind::Usage => 170.0,
            Kind::System => 150.0, // métricas (110) + anillo del disco del sistema
            Kind::Media => media_w(),
            Kind::Radio => 94.0, // nota + letrero «ON AIR»; el nombre se desplaza debajo
        }
    }
}

/// Ancho del panel multimedia (px lógicos): tamaños del menú y clave `MediaWidth` de config.json
const MEDIA_SIZES: [(f32, &str, &str); 5] = [
    (400.0, "Compacto", "Compact"),
    (480.0, "Mediano", "Medium"),
    (560.0, "Grande (predeterminado)", "Large (default)"),
    (680.0, "Extra grande", "Extra large"),
    (800.0, "Máximo", "Maximum"),
];
static MEDIA_W: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn media_w() -> f32 {
    match MEDIA_W.load(std::sync::atomic::Ordering::Relaxed) {
        0 => 560.0,
        b => f32::from_bits(b),
    }
}

fn set_media_w(cfg: &Config) {
    let w = cfg.v["MediaWidth"].as_f64().map(|v| v.clamp(320.0, 1200.0) as f32).unwrap_or(560.0);
    MEDIA_W.store(w.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

struct Panel {
    hwnd: HWND,
    /// barra de tareas de la que el panel es ventana hija (nulo = ventana flotante "siempre encima")
    parent: HWND,
    kind: Kind,
    tb_key: isize,
    scale: f32,
    caps: Vec<f32>,
    disp: Vec<f32>,
    hover: i32,
    surf: Option<Surface>,
    /// panel en una barra vertical (diseño compacto)
    vertical: bool,
    /// modo icono (barra con muchas apps)
    compact: bool,
    /// 0 = vinilo escondido, 1 = asomado (animado)
    vinyl: f32,
    spin: f32,
    last_t: f32,
}

/// Arrastre en curso de un panel de la derecha
#[derive(Clone, Copy)]
struct Drag {
    slot: usize,
    x0: i32,
    off0: i32,
    max: i32,
    moved: bool,
}

/// Índice en `App::offs` de los paneles de la derecha (red, IA, sistema)
fn chain_slot(k: Kind) -> Option<usize> {
    match k {
        Kind::Ip => Some(0),
        Kind::Usage => Some(1),
        Kind::System => Some(2),
        Kind::Media => None,
        Kind::Radio => Some(3),
    }
}

/// Coloca los paneles de la derecha según la separación propia de cada uno. Se ordenan por posición
/// deseada (de derecha a izquierda) y, si dos se tocarían, el de la izquierda se desplaza hasta quedar
/// a 14 px lógicos del otro: se mueven de forma independiente pero nunca se solapan.
/// `min_left` es el límite izquierdo (p. ej. el espacio del multimedia): los que lo pasen se empujan
/// a la derecha, también sin solaparse. Devuelve (tipo, x, ancho) en px físicos, de derecha a izquierda.
fn chain_layout(b: &Taskbar, items: &[(Kind, i32)], offs: &[i32; 4], min_left: i32) -> Vec<(Kind, i32, i32)> {
    let mut v: Vec<(Kind, i32)> = items.to_vec();
    v.sort_by_key(|(k, _)| chain_slot(*k).map(|i| (offs[i], i)).unwrap_or((0, 0)));
    let sep = (14.0 * b.scale) as i32;
    let mut edge = b.right + sep;
    let mut placed: Vec<(Kind, i32, i32)> = v
        .into_iter()
        .map(|(k, w)| {
            let off = chain_slot(k).map(|i| offs[i]).unwrap_or(0);
            let xr = (b.right - (off as f32 * b.scale) as i32).min(edge - sep);
            edge = xr - w;
            (k, xr - w, w)
        })
        .collect();
    // Límite izquierdo: nunca antes del borde de la barra (las separaciones son globales y pueden venir
    // de un monitor más ancho, o un vecino puede empujar hasta pasarlo) → se empujan a la derecha
    let mut floor = min_left.max(b.left);
    for c in placed.iter_mut().rev() {
        c.1 = c.1.max(floor);
        floor = c.1 + c.2 + sep;
    }
    // Límite derecho: si el empuje hacia la derecha los sacó por ese lado, se recolocan pegados a él.
    // Solo podrían pasar del izquierdo si la suma de anchos no cabe en la barra (no es un caso real).
    let mut ceil = b.right;
    for c in placed.iter_mut() {
        c.1 = c.1.min(ceil - c.2);
        ceil = c.1 - sep;
    }
    placed
}

/// Paneles automáticos que pasan a modo icono: solo los que hace falta para despejar la zona de los
/// iconos de apps (`zone`, px físicos). Se prueba de izquierda a derecha y un panel solo se condensa
/// si con ello se reduce la invasión (la suya o la del vecino al que empuja): mover uno hasta los
/// iconos no cambia el formato de los demás. `items` = (tipo, ancho actual, ancho en icono, automático).
fn auto_compact(b: &Taskbar, items: &[(Kind, i32, i32, bool)], offs: &[i32; 4], zone: (i32, i32)) -> Vec<Kind> {
    let layout = |compact: &[Kind]| {
        let it: Vec<(Kind, i32)> = items.iter().map(|&(k, full, icon, _)| (k, if compact.contains(&k) { icon } else { full })).collect();
        chain_layout(b, &it, offs, i32::MIN)
    };
    let invaded = |p: &[(Kind, i32, i32)]| -> i64 { p.iter().map(|&(_, x, w)| ((x + w).min(zone.1) as i64 - x.max(zone.0) as i64).max(0)).sum() };
    let mut compact: Vec<Kind> = Vec::new();
    loop {
        let placed = layout(&compact);
        let cur = invaded(&placed);
        if cur == 0 {
            break;
        }
        let next = placed.iter().rev().map(|c| c.0).find(|k| {
            let auto = items.iter().any(|i| i.0 == *k && i.3);
            auto && !compact.contains(k) && invaded(&layout(&[compact.as_slice(), &[*k]].concat())) < cur
        });
        match next {
            Some(k) => compact.push(k),
            None => break,
        }
    }
    compact
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys_ok() -> state::State {
        state::State {
            cpu: 20.0,
            cpu_t: 60,
            gpu_t: 55,
            hist_cpu_t: vec![60.0; 60],
            hist_gpu_t: vec![55.0; 60],
            ram_used: 8.0,
            ram_total: 32.0,
            drives: vec![state::Drive { letter: std::env::var("SystemDrive").ok().and_then(|v| v.chars().next()).unwrap_or('C'), label: String::new(), total: 1000e9, free: 400e9 }],
            bat_present: true,
            bat_pct: 80,
            ..Default::default()
        }
    }

    #[test]
    fn sistema_sin_avisos() {
        assert!(sys_alerts(&sys_ok()).is_empty());
        assert_eq!(sys_level(&sys_ok()), net::Level::Ok);
        assert_eq!(sys_level(&state::State { cpu: -1.0, ..Default::default() }), net::Level::Unknown);
    }

    #[test]
    fn temperatura_solo_si_es_sostenida() {
        let mut s = sys_ok();
        // pico de 10 s a 99 °C: no avisa
        s.hist_cpu_t = [vec![60.0; 50], vec![99.0; 10]].concat();
        assert!(sys_alerts(&s).is_empty());
        // 30 s a 96 °C: ámbar; a 101 °C: rojo
        s.hist_cpu_t = [vec![60.0; 30], vec![96.0; 30]].concat();
        s.cpu_t = 96;
        assert_eq!(sys_alerts(&s), vec![(net::Level::Warn, SysAlert::CpuTemp(96))]);
        s.hist_cpu_t = [vec![60.0; 30], vec![101.0; 30]].concat();
        assert_eq!(sys_level(&s), net::Level::Bad);
        // la GPU avisa antes que la CPU
        let mut g = sys_ok();
        g.hist_gpu_t = vec![88.0; 60];
        g.gpu_t = 88;
        assert_eq!(sys_alerts(&g), vec![(net::Level::Warn, SysAlert::GpuTemp(88))]);
    }

    #[test]
    fn memoria_disco_y_bateria() {
        let mut s = sys_ok();
        s.ram_used = 29.5; // 92 %
        s.drives[0].free = 30e9; // 3 % libre
        s.bat_pct = 12;
        let a = sys_alerts(&s);
        assert_eq!(a.len(), 3);
        assert_eq!(a[0], (net::Level::Warn, SysAlert::Ram(92)));
        assert_eq!(a[1].0, net::Level::Bad);
        assert!(matches!(a[1].1, SysAlert::Disk(_, 30)));
        assert_eq!(a[2], (net::Level::Warn, SysAlert::Battery(12)));
        assert_eq!(sys_level(&s), net::Level::Bad);
        // enchufado: la batería baja no es un aviso
        s.bat_ac = true;
        assert_eq!(sys_alerts(&s).len(), 2);
    }

    #[test]
    fn alto_de_la_ventana_de_sistema_cubre_ambas_columnas() {
        let mut s = sys_ok();
        s.gpus = vec![state::GpuStat::default(); 4];
        s.fan_c = 2000;
        // columna izquierda larga: 5 tarjetas de 92 + ventiladores
        assert!(sys_popup_h(&s) >= 40.0 + 92.0 * 5.0 + FAN_CARD_H + 8.0);
        // sin GPU detectadas manda la derecha, que siempre incluye la tarjeta de FPS
        s.gpus.clear();
        assert!(sys_popup_h(&s) > 40.0 + sys_left_h(&s) + FPS_CARD_MIN);
        s.gpus = vec![state::GpuStat::default(); 4];
        // con un aviso crece lo que ocupa la franja
        let base = sys_popup_h(&s);
        s.ram_used = 31.0;
        assert_eq!(sys_popup_h(&s), base + alerts_h(1) + 8.0);
    }

    fn bar(left: i32, right: i32, scale: f32) -> Taskbar {
        Taskbar { left, right, scale, height: 48, ..Default::default() }
    }

    /// Comprueba límites de la barra, `min_left` (si cabe) y que no haya solapes
    fn check(b: &Taskbar, items: &[(Kind, i32)], offs: &[i32; 4], min_left: i32) {
        let p = chain_layout(b, items, offs, min_left);
        assert_eq!(p.len(), items.len());
        let sep = (14.0 * b.scale) as i32;
        let total: i32 = items.iter().map(|i| i.1).sum::<i32>() + sep * (items.len() as i32 - 1).max(0);
        for (k, x, w) in &p {
            assert!(x + w <= b.right, "{k:?} pasa del borde derecho: {x}+{w} > {} (offs={offs:?})", b.right);
            assert!(*x >= b.left, "{k:?} pasa del borde izquierdo: {x} < {} (offs={offs:?})", b.left);
            if min_left > b.left && min_left + total <= b.right {
                assert!(*x >= min_left, "{k:?} invade el multimedia: {x} < {min_left} (offs={offs:?})");
            }
        }
        let mut s = p.clone();
        s.sort_by_key(|c| c.1);
        for w in s.windows(2) {
            assert!(w[0].1 + w[0].2 + sep <= w[1].1, "solape {:?} / {:?} (offs={offs:?})", w[0], w[1]);
        }
    }

    #[test]
    fn modo_icono_solo_para_el_panel_que_invade_los_iconos() {
        // barra 0..1920 con iconos centrados entre x=700 y x=1200 (zona con margen: 684..1216)
        let b = Taskbar { start_left: 700, tasks_right: 1200, ..bar(0, 1920, 1.0) };
        let zone = (684, 1216);
        let items = [(Kind::Ip, 112, 48, true), (Kind::Usage, 150, 48, true), (Kind::System, 236, 48, true), (Kind::Radio, 94, 48, true)];
        // todos a la derecha con sitio de sobra: ninguno cambia
        assert!(auto_compact(&b, &items, &[8, 140, 310, 560], zone).is_empty());
        // la radio arrastrada hasta los iconos: solo ella pasa a icono
        assert_eq!(auto_compact(&b, &items, &[8, 140, 310, 730], zone), vec![Kind::Radio]);
        // la radio a la izquierda de Inicio (sitio libre): nadie cambia
        assert!(auto_compact(&b, &items, &[8, 140, 310, 1500], zone).is_empty());
        // barra llena (iconos hasta x=1500): se condensan de izquierda a derecha solo los necesarios
        let full = Taskbar { start_left: 300, tasks_right: 1500, ..bar(0, 1920, 1.0) };
        assert_eq!(auto_compact(&full, &items, &[8, 140, 310, 560], (284, 1516)), vec![Kind::Radio, Kind::System]);
        // un panel fijo (no automático) que invade no arrastra a los demás
        let fixed = [(Kind::Ip, 112, 48, true), (Kind::Radio, 94, 48, false)];
        assert!(auto_compact(&b, &fixed, &[8, 140, 310, 730], zone).is_empty());
    }

    #[test]
    fn paneles_dentro_de_la_barra_y_sin_solapes() {
        for (left, right, scale) in [(0, 1920, 1.0), (1920, 3840, 1.5), (-1280, 0, 1.25), (0, 1366, 2.0)] {
            let b = bar(left, right, scale);
            let wid = |k: Kind, compact: bool| ((if compact { compact_width(k) } else { k.width() }) * scale) as i32;
            let span = ((right - left) as f32 / scale) as i32;
            let steps: Vec<i32> = (0..=12).map(|i| i * (span + 400) / 12).chain([-50, 0, 8]).collect();
            for compact in [false, true] {
                let all = [(Kind::Ip, wid(Kind::Ip, compact)), (Kind::Usage, wid(Kind::Usage, compact)), (Kind::System, wid(Kind::System, compact))];
                for &a in &steps {
                    for &u in &steps {
                        for &s in &steps {
                            let offs = [a.max(0), u.max(0), s.max(0), 8];
                            for min_left in [i32::MIN, left + (400.0 * scale) as i32, right - 50] {
                                check(&b, &all, &offs, min_left);
                                check(&b, &all[..2], &offs, min_left);
                                check(&b, &all[1..], &offs, min_left);
                                check(&b, &all[2..], &offs, min_left);
                            }
                        }
                    }
                }
            }
        }
    }
}

struct App {
    cfg: Config,
    main: HWND,
    panels: Vec<Panel>,
    bars: Vec<Taskbar>,
    /// separación de red, IA y sistema respecto al borde derecho (px lógicos, ver `chain_layout`)
    offs: [i32; 4],
    drag: Option<Drag>,
    net_prev: Option<(u64, u64, Instant)>,
    taskbar_created: u32,
    t0: Instant,
    popup: HWND,
    popup_surf: Option<Surface>,
    /// cuándo se ocultó la ventana flotante (su lienzo se libera pasados unos segundos)
    popup_hidden_at: Option<Instant>,
    popup_scale: f32,
    popup_shown: bool,
    hover_since: Option<Instant>,
    popup_kind: Option<Kind>,
    popup_disp: Vec<f32>,
    /// desde cuándo cada barra cumple la condición de ocultar (margen anti-parpadeo)
    hide_since: std::collections::HashMap<isize, Instant>,
    hidden: std::collections::HashSet<isize>,
    /// última vez que había reproducción en curso
    last_playing: Option<Instant>,
    /// estilo de fondo aplicado a la ventana flotante
    popup_style: String,
    /// modo claro (true) u oscuro de cada barra según su color real en pantalla: Explorer a veces
    /// no repinta alguna barra (sobre todo las secundarias) al cambiar el modo de Windows
    bar_light: std::collections::HashMap<isize, bool>,
    /// barra del panel cuya ventana flotante está abierta y modo con el que se aplicó su estilo
    popup_tb: isize,
    popup_light: Option<bool>,
}

impl App {
    /// Modo de una barra: el detectado en pantalla o, si aún no hay muestra, el de Windows
    fn light_of(&self, tb_key: isize) -> bool {
        self.bar_light.get(&tb_key).copied().unwrap_or_else(render::system_light)
    }
}

/// Luminancia mediana de una línea de píxeles de la barra junto a su borde interior (lejos de los
/// iconos), saltando las zonas que ocupan nuestros paneles. None si no hay muestras suficientes.
unsafe fn bar_luma(b: &Taskbar, skip: &[RECT]) -> Option<f32> {
    let r = b.rect;
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 8 || h <= 8 {
        return None;
    }
    // horizontal: fila a 3 px del borde interior; vertical: columna a 3 px del borde interior
    let (x0, y0, cw, ch) = if b.vertical {
        (if b.side_left { r.right - 4 } else { r.left + 3 }, r.top, 1, h)
    } else {
        (r.left, if b.at_top { r.bottom - 4 } else { r.top + 3 }, w, 1)
    };
    let screen = GetDC(HWND::default());
    let mem = CreateCompatibleDC(screen);
    let bmp = CreateCompatibleBitmap(screen, cw, ch);
    let old = SelectObject(mem, bmp);
    let ok = BitBlt(mem, 0, 0, cw, ch, screen, x0, y0, SRCCOPY).is_ok();
    let mut lum = Vec::new();
    if ok {
        let n = cw.max(ch);
        let step = (n / 64).max(4);
        for i in (0..n).step_by(step as usize) {
            let (px, py) = if b.vertical { (0, i) } else { (i, 0) };
            let (sx, sy) = (x0 + px, y0 + py);
            if skip.iter().any(|k| sx >= k.left && sx < k.right && sy >= k.top && sy < k.bottom) {
                continue;
            }
            let c = GetPixel(mem, px, py).0;
            if c == CLR_INVALID {
                continue;
            }
            let (rr, gg, bb) = ((c & 0xFF) as f32, ((c >> 8) & 0xFF) as f32, ((c >> 16) & 0xFF) as f32);
            lum.push(0.299 * rr + 0.587 * gg + 0.114 * bb);
        }
    }
    SelectObject(mem, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    ReleaseDC(HWND::default(), screen);
    if lum.len() < 6 {
        return None;
    }
    lum.sort_by(|a, b| a.total_cmp(b));
    Some(lum[lum.len() / 2])
}

/// Actualiza el modo claro/oscuro de cada barra visible según su color real (con histéresis)
unsafe fn refresh_bar_themes(app: &mut App) {
    let fallback = render::system_light();
    let mut out = std::collections::HashMap::new();
    for b in &app.bars {
        let prev = app.bar_light.get(&b.key).copied();
        let mode = if app.hidden.contains(&b.key) {
            prev // barra oculta (pantalla completa, autoocultar): se conserva lo último
        } else {
            let skip: Vec<RECT> = app
                .panels
                .iter()
                .filter(|p| p.tb_key == b.key && IsWindowVisible(p.hwnd).as_bool())
                .map(|p| {
                    let mut r = RECT::default();
                    let _ = GetWindowRect(p.hwnd, &mut r);
                    r
                })
                .collect();
            match bar_luma(b, &skip) {
                Some(l) if l >= 150.0 => Some(true),
                Some(l) if l <= 110.0 => Some(false),
                _ => prev, // dudoso (fondo semitransparente sobre el escritorio): se mantiene
            }
        };
        out.insert(b.key, mode.unwrap_or(fallback));
    }
    app.bar_light = out;
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

const WM_TRAY: u32 = WM_APP + 1;
const TIMER_MAIN: usize = 1;
const TIMER_FAST: usize = 2;
const CLASS: PCWSTR = w!("TaskbarPanelsWnd");

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

// ======================================================================= dibujo (alfa por píxel)
const GAINSBORO: Rgb = Rgb::u8(220, 220, 220);
const TRACK: Rgb = Rgb::u8(60, 66, 90);
const GRAY: Rgb = Rgb::u8(128, 128, 128);

fn severity(pct: Option<f64>, base: Rgb) -> Rgb {
    match pct {
        None => base, // indicador sin porcentaje (saldo, modelos): color del proveedor
        Some(p) if p >= 90.0 => Rgb::u8(255, 70, 90),
        Some(p) if p >= 70.0 => Rgb::u8(255, 200, 60),
        _ => base,
    }
}
fn usage_color(v: f32, base: Rgb) -> Rgb {
    if v >= 90.0 { Rgb::u8(255, 70, 90) } else if v >= 75.0 { Rgb::u8(255, 200, 60) } else { base }
}
fn temp_color(t: i32) -> Rgb {
    match t {
        t if t < 0 => GRAY,
        t if t >= 90 => Rgb::u8(255, 70, 90),
        t if t >= 80 => Rgb::u8(255, 160, 60),
        t if t >= 70 => Rgb::u8(255, 210, 80),
        _ => Rgb::u8(170, 220, 255),
    }
}

/// Barra de progreso redondeada (pista + relleno)
fn pill(c: &mut Surface, x: f32, yc: f32, w: f32, h: f32, frac: Option<f32>, col: Rgb) {
    let r = h / 2.0;
    c.capsule_h(x, x + w, yc, r, TRACK, 0.9);
    if let Some(f) = frac {
        let fw = (w * f.clamp(0.0, 1.0)).max(h);
        c.capsule_h(x, x + fw, yc, r, col, 1.0);
    }
}

/// Velocidad separada en cifra y unidad (bits/s), p. ej. ("12.4", "Mb/s")
fn rate_parts(bytes_per_s: f64) -> (String, &'static str) {
    let b = bytes_per_s * 8.0;
    if b >= 1e9 {
        (format!("{:.2}", b / 1e9), "Gb/s")
    } else if b >= 1e6 {
        (format!("{:.1}", b / 1e6), "Mb/s")
    } else if b >= 1e3 {
        (format!("{:.0}", b / 1e3), "Kb/s")
    } else {
        (format!("{b:.0}"), "b/s")
    }
}

const UP_COL: Rgb = Rgb::u8(110, 160, 255);
const DOWN_COL: Rgb = Rgb::u8(60, 230, 160);

fn paint_ip(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, t: f32, show_ip: bool) {
    // 3 filas con IP, 2 sin ella: letra, iconos y gráficas crecen para ocupar la altura
    let n = if show_ip { 3.0 } else { 2.0 };
    let row = h / n;
    let px = (10.0 * sc * (3.0 / n as f32).sqrt()).min((row * if show_ip { 0.72 } else { 0.60 }).floor());
    let icon_px = (px * 0.95).floor();
    let icon_w = (px * 1.25).max(14.0 * sc);
    let x_txt = icon_w + 2.0 * sc;
    // Escala común de las gráficas: máximo del último minuto (mín. 64 Kb/s para no amplificar ruido)
    let maxv = s.hist_down.iter().chain(s.hist_up.iter()).fold(8_000.0f32, |m, v| m.max(*v)) * 1.15;
    let rows = [("\u{E898}", s.up_bps, &s.hist_up, UP_COL), ("\u{E896}", s.down_bps, &s.hist_down, DOWN_COL)];
    for (i, (glyph, bps, hist, col)) in rows.iter().enumerate() {
        let y = i as f32 * row;
        // gráfica de fondo en la banda de la fila (deja 1 px de separación entre filas)
        c.sparkline(0.0, y + 1.0, w, row - 2.0, hist, maxv, *col, 1.0);
        c.text(glyph, 1.0 * sc, y, icon_w, row, icon_px, 400, "Segoe Fluent Icons", *col, DT_CENTER);
        let (num, unit) = rate_parts(*bps);
        let nw = c.text_width(&num, px, 600, "Segoe UI");
        c.text(&num, x_txt, y, nw + 2.0, row, px, 600, "Segoe UI", col.mix(WHITE, 0.35), DT_LEFT);
        c.text(unit, x_txt + nw + 2.5 * sc, y, w - x_txt - nw, row, (px * 0.86).floor(), 400, "Segoe UI", Rgb::u8(165, 175, 195), DT_LEFT);
    }
    health_dot(c, w, sc);
    if !show_ip {
        return;
    }
    // Fila de IP: punto de estado con halo + IP
    let y = 2.0 * row;
    let (dot_col, glow) = if s.ip.contains('.') || s.ip.contains(':') {
        // late durante 10 s después de un cambio de IP
        let recent = s.ip_changed_at.map(|t0| t0.elapsed().as_secs_f32() < 10.0).unwrap_or(false);
        let pulse = if recent { 0.5 + 0.5 * (t * 6.0).sin() } else { 0.0 };
        (Rgb::u8(60, 230, 120).mix(Rgb::u8(255, 220, 80), pulse), 0.45 + 0.4 * pulse)
    } else if s.ip == tr("consultando...", "checking...") {
        (Rgb::u8(255, 200, 60), 0.4)
    } else {
        (Rgb::u8(255, 70, 90), 0.5)
    };
    // con conexión: bandera del país de la IP en lugar del punto (si ya se cargó)
    let online = s.ip.contains('.') || s.ip.contains(':');
    let fh = (px * 0.78).max(6.0 * sc);
    let fw = if online { flag_width(fh) } else { 0.0 };
    if fw > 0.0 {
        if glow > 0.5 {
            c.dot(1.0 * sc + fw / 2.0, y + row / 2.0, 2.6 * sc, dot_col, 0.0, 4.0 * sc, glow); // halo tras un cambio de IP
        }
        draw_flag(c, 1.0 * sc, y + row / 2.0, fh, sc);
    } else {
        c.dot(1.0 * sc + icon_w / 2.0, y + row / 2.0, 2.6 * sc, dot_col, 1.0, 2.5 * sc, glow);
    }
    let xt = x_txt.max(1.0 * sc + fw + 3.0 * sc);
    c.text(&s.ip, xt, y, w - xt, row, px, 400, "Segoe UI", GAINSBORO, DT_LEFT | DT_END_ELLIPSIS);
}

/// Ancho de la bandera del país de la IP a una altura dada (0 si no hay bandera)
fn flag_width(h: f32) -> f32 {
    flags::FLAG.lock().unwrap().as_ref().map(|f| (h * f.w as f32 / f.h as f32).round()).unwrap_or(0.0)
}

/// Dibuja la bandera (alto `h`, centrada en `cy`) con un borde tenue; devuelve el ancho usado
fn draw_flag(c: &mut Surface, x: f32, cy: f32, h: f32, sc: f32) -> f32 {
    let Some(f) = flags::FLAG.lock().unwrap().clone() else { return 0.0 };
    let w = (h * f.w as f32 / f.h as f32).round();
    let (x, y) = (x.round(), (cy - h / 2.0).round());
    let r = (1.5 * sc).min(h * 0.2);
    c.round_rect(x - 0.5 * sc, y - 0.5 * sc, w + 1.0 * sc, h + 1.0 * sc, r + 0.5 * sc, WHITE, 0.18);
    c.image_rect(x, y, w, h, r, &f.px, f.w, f.h, 1.0);
    w
}

// ======================================================================= IA: proveedores, iconos y paneles
/// Lo que muestran los paneles de IA: proveedores habilitados y el principal ("all" = resumen)
#[derive(Clone, Default)]
struct AiView {
    list: Vec<ai::Provider>,
    primary: String,
}

impl AiView {
    fn from_cfg(cfg: &Config) -> AiView {
        AiView { list: ai::snapshot(cfg), primary: ai::primary(cfg) }
    }
    /// Proveedores que se ven en la barra: el principal, o todos si es "all" (o no existe)
    fn shown(&self) -> Vec<&ai::Provider> {
        match self.list.iter().find(|p| p.id == self.primary) {
            Some(p) if self.primary != "all" => vec![p],
            _ => self.list.iter().take(3).collect(),
        }
    }
}

/// Color de acento de cada proveedor
fn ai_color(p: &ai::Provider) -> Rgb {
    match p.kind.as_str() {
        "claude" => Rgb::u8(232, 132, 96),
        "copilot" => Rgb::u8(130, 110, 250),
        "openai" => Rgb::u8(40, 200, 150),
        "anthropic_api" => Rgb::u8(222, 170, 130),
        "deepseek" => Rgb::u8(100, 135, 255),
        "kimi" => Rgb::u8(70, 190, 255),
        "gemini" => Rgb::u8(120, 150, 250),
        "ollama" => Rgb::u8(215, 218, 228),
        "ollama_cloud" => Rgb::u8(120, 185, 255),
        "lmstudio" => Rgb::u8(165, 125, 255),
        _ => {
            let h = p.name.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
            neon_at((h % 1000) as f32 / 1000.0)
        }
    }
}

/// Icono estilizado de cada proveedor (dibujado, no son los logotipos oficiales). Conserva sus
/// colores propios también en modo claro.
fn draw_ai_icon(c: &mut Surface, p: &ai::Provider, cx: f32, cy: f32, r: f32) {
    c.raw(|c| draw_ai_icon_colors(c, p, cx, cy, r));
}

fn draw_ai_icon_colors(c: &mut Surface, p: &ai::Provider, cx: f32, cy: f32, r: f32) {
    use std::f32::consts::TAU;
    match p.kind.as_str() {
        "claude" => {
            // mascota pixelada de Claude Code (rejilla de 24×24, cuerpo de y=5 a y=20): cuerpo,
            // brazos, cuatro patas y dos ojos huecos
            let col = Rgb::u8(217, 119, 87);
            let k = r * 2.0 / 24.0;
            let (ox, oy) = (cx - r, cy - r - 0.5 * k);
            let e = 0.06; // solape para que no se vean costuras entre rectángulos
            let mut px = |x0: f32, y0: f32, x1: f32, y1: f32| c.round_rect(ox + x0 * k, oy + y0 * k, (x1 - x0) * k, (y1 - y0) * k, 0.0, col, 1.0);
            px(3.0, 5.0, 21.0, 8.102 + e); // frente
            px(3.0, 8.102, 6.0 + e, 10.949 + e); // franja de los ojos
            px(7.488 - e, 8.102, 16.51 + e, 10.949 + e);
            px(18.0 - e, 8.102, 21.0, 10.949 + e);
            px(0.0, 10.949, 24.0, 14.051); // brazos
            px(3.0, 14.051 - e, 21.0, 17.079 + e); // parte baja
            for x in [4.487f32, 7.488, 15.0, 18.0] {
                px(x, 17.079, x + 1.513, 20.0); // patas
            }
        }
        "copilot" => {
            // casco de Copilot a color: degradado azul → morado, gafas de piloto lavanda arriba y
            // visor oscuro con dos ojos cian luminosos
            let (blue, purple) = (Rgb::u8(70, 110, 245), Rgb::u8(150, 70, 225));
            // "orejas" (laterales del casco) y casco
            c.round_rect_grad(cx - r * 1.0, cy - r * 0.05, r * 2.0, r * 0.62, r * 0.28, blue.mix(purple, 0.4), purple, 1.0);
            c.round_rect_grad(cx - r * 0.86, cy - r * 0.92, r * 1.72, r * 1.78, r * 0.80, blue, purple, 1.0);
            // gafas: montura lavanda con lentes moradas, sobre la frente
            let frame = Rgb::u8(196, 178, 255);
            c.round_rect(cx - r * 0.78, cy - r * 0.80, r * 1.56, r * 0.58, r * 0.27, frame, 1.0);
            for dx in [-0.36f32, 0.36] {
                c.round_rect(cx + r * dx - r * 0.28, cy - r * 0.71, r * 0.56, r * 0.40, r * 0.19, Rgb::u8(92, 64, 190), 1.0);
                c.round_rect(cx + r * dx - r * 0.16, cy - r * 0.64, r * 0.20, r * 0.10, r * 0.05, WHITE, 0.35); // reflejo
            }
            // visor oscuro
            c.round_rect(cx - r * 0.62, cy - r * 0.12, r * 1.24, r * 0.78, r * 0.32, Rgb::u8(26, 22, 58), 1.0);
            // ojos cian con halo
            for dx in [-0.22f32, 0.22] {
                c.round_rect(cx + r * dx - r * 0.085, cy + r * 0.06, r * 0.17, r * 0.36, r * 0.085, Rgb::u8(70, 235, 255), 1.0);
                c.round_rect(cx + r * dx - r * 0.04, cy + r * 0.11, r * 0.08, r * 0.26, r * 0.04, Rgb::u8(200, 252, 255), 0.8);
            }
        }
        "openai" => {
            // nudo hexagonal blanco sobre verde
            c.dot(cx, cy, r, Rgb::u8(16, 163, 127), 1.0, 0.0, 0.0);
            for k in 0..3 {
                c.ellipse_ring(cx, cy, r * 0.58, r * 0.26, k as f32 * TAU / 6.0, (r * 0.13).max(1.2), WHITE, 1.0);
            }
        }
        "anthropic_api" => {
            c.dot(cx, cy, r, Rgb::u8(240, 234, 222), 1.0, 0.0, 0.0);
            c.text("A", cx - r, cy - r, r * 2.0, r * 2.0, r * 1.25, 900, "Segoe UI", Rgb::u8(25, 25, 25), DT_CENTER);
        }
        "deepseek" => {
            // ballena blanca sobre azul: cuerpo, cola y ojo
            let blue = Rgb::u8(77, 107, 254);
            c.dot(cx, cy, r, blue, 1.0, 0.0, 0.0);
            c.round_rect(cx - r * 0.62, cy - r * 0.22, r * 1.0, r * 0.62, r * 0.31, WHITE, 1.0);
            c.round_rect(cx + r * 0.22, cy - r * 0.50, r * 0.20, r * 0.52, r * 0.10, WHITE, 1.0);
            c.round_rect(cx + r * 0.10, cy - r * 0.56, r * 0.50, r * 0.20, r * 0.10, WHITE, 1.0);
            c.dot(cx - r * 0.34, cy + r * 0.02, r * 0.09, blue, 1.0, 0.0, 0.0);
        }
        "kimi" => {
            // K blanca sobre negro con punto azul
            c.dot(cx, cy, r, Rgb::u8(22, 22, 28), 1.0, 0.0, 0.0);
            c.text("K", cx - r * 1.1, cy - r, r * 2.0, r * 2.0, r * 1.2, 900, "Segoe UI", WHITE, DT_CENTER);
            c.dot(cx + r * 0.50, cy - r * 0.46, r * 0.17, Rgb::u8(40, 140, 255), 1.0, 0.0, 0.0);
        }
        "gemini" => {
            // destello de cuatro puntas blanco sobre degradado azul → morado
            c.round_rect_grad(cx - r, cy - r, r * 2.0, r * 2.0, r, Rgb::u8(66, 133, 244), Rgb::u8(155, 114, 203), 1.0);
            let t = r * 0.13;
            c.round_rect(cx - t, cy - r * 0.70, t * 2.0, r * 1.40, t, WHITE, 1.0);
            c.round_rect(cx - r * 0.70, cy - t, r * 1.40, t * 2.0, t, WHITE, 1.0);
            c.dot(cx, cy, r * 0.30, WHITE, 1.0, 0.0, 0.0);
        }
        "ollama" | "ollama_cloud" => {
            // llama: orejas, cabeza, ojos y hocico; en Cloud, un punto azul
            let (light, dark) = (Rgb::u8(244, 244, 246), Rgb::u8(34, 34, 38));
            c.dot(cx, cy, r, light, 1.0, 0.0, 0.0);
            for dx in [-0.30f32, 0.30] {
                c.round_rect(cx + r * dx - r * 0.11, cy - r * 0.78, r * 0.22, r * 0.60, r * 0.11, dark, 1.0);
            }
            c.round_rect(cx - r * 0.46, cy - r * 0.40, r * 0.92, r * 1.10, r * 0.40, dark, 1.0);
            for dx in [-0.20f32, 0.20] {
                c.dot(cx + r * dx, cy - r * 0.06, r * 0.085, light, 1.0, 0.0, 0.0);
            }
            c.round_rect(cx - r * 0.20, cy + r * 0.16, r * 0.40, r * 0.28, r * 0.14, light, 1.0);
            if p.kind == "ollama_cloud" {
                c.dot(cx + r * 0.66, cy + r * 0.62, r * 0.30, Rgb::u8(60, 150, 255), 1.0, 0.0, 0.0);
            }
        }
        "lmstudio" => {
            // cuadrado redondeado morado → azul con tres barras blancas escalonadas
            c.round_rect_grad(cx - r * 0.92, cy - r * 0.92, r * 1.84, r * 1.84, r * 0.42, Rgb::u8(120, 80, 240), Rgb::u8(70, 110, 250), 1.0);
            for (i, (x0, x1)) in [(-0.50f32, 0.30f32), (-0.30, 0.52), (-0.50, 0.10)].iter().enumerate() {
                c.capsule_h(cx + r * x0, cx + r * x1, cy + r * (-0.40 + 0.40 * i as f32), r * 0.11, WHITE, 1.0);
            }
        }
        _ => {
            // personalizado: círculo de color con iniciales
            c.dot(cx, cy, r, ai_color(p), 1.0, 0.0, 0.0);
            let ini: String = p.name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect::<String>().to_uppercase();
            c.text(if ini.is_empty() { "AI" } else { &ini }, cx - r, cy - r, r * 2.0, r * 2.0, r * 0.95, 700, "Segoe UI", WHITE, DT_CENTER);
        }
    }
}

fn metric_label(key: &str) -> String {
    match key {
        "session5h" => tr("Sesión 5 h", "5h session").into(),
        "weekly" => tr("Semana", "Week").into(),
        "premium" => tr("Premium", "Premium").into(),
        "month" => tr("Gasto del mes", "Month spend").into(),
        "usage" => tr("Uso", "Usage").into(),
        "balance" => tr("Saldo", "Balance").into(),
        "models" => tr("Modelos cargados", "Loaded models").into(),
        "cloud_models" => tr("Modelos disponibles", "Available models").into(),
        "models_available" => tr("Modelos disponibles", "Available models").into(),
        other => other.to_string(),
    }
}

fn status_text(s: &str) -> &'static str {
    match s {
        "no_credentials" => tr("sin credenciales", "no credentials"),
        "expired" => tr("token expirado (abre Claude Code)", "token expired (open Claude Code)"),
        "rate_limited" => tr("límite de consultas: datos anteriores", "rate limited: previous data"),
        "invalid" => tr("respuesta no válida", "invalid response"),
        "no_key" => tr("falta la clave", "missing key"),
        "bad_key" => tr("clave no válida o sin permisos", "invalid key or no permission"),
        "no_url" => tr("falta la URL", "missing URL"),
        "no_data" => tr("sin datos (sin conexión)", "no data (offline)"),
        "offline" => tr("servidor apagado o inaccesible", "server not running or unreachable"),
        "no_copilot" => tr("el token de GitHub no tiene acceso a Copilot", "GitHub token has no Copilot access"),
        "" => "",
        _ => tr("consultando…", "checking…"),
    }
}

/// Texto del reinicio de un indicador ("reinicia en 2h05 · 23:00" / "resets on Dec 1 · in 17d")
fn reset_text(m: &ai::Metric) -> String {
    let Some(e) = m.reset else { return String::new() };
    if e - util::now_epoch() < 2 * 86_400 {
        format!("{} {}  ·  {}", tr("reinicia en", "resets in"), util::fmt_left(e), util::fmt_local(e))
    } else if i18n::es() {
        format!("reinicia el {}  ·  en {}", util::fmt_date(e), util::fmt_left(e))
    } else {
        format!("resets on {}  ·  in {}", util::fmt_date(e), util::fmt_left(e))
    }
}

/// Panel de IA en la barra: el principal con su icono, o un resumen de todos
fn paint_ai(c: &mut Surface, w: f32, h: f32, sc: f32, view: &AiView) {
    let shown = view.shown();
    if shown.is_empty() {
        c.text(tr("Sin proveedores de IA", "No AI providers"), 2.0 * sc, 0.0, w, h, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        return;
    }
    let single = shown.len() == 1 && view.primary != "all";
    if !single {
        // resumen: una fila por proveedor (icono · barra · valor del primer indicador)
        let n = shown.len() as f32;
        let row = h / n;
        let px = (10.0 * sc * (3.0 / n).sqrt()).min((row * 0.62).floor());
        let ir = (row * 0.34).min(9.0 * sc);
        let vw = c.text_width("100%", px, 600, "Segoe UI");
        for (i, p) in shown.iter().enumerate() {
            let y = i as f32 * row;
            draw_ai_icon(c, p, 2.0 * sc + ir, y + row / 2.0, ir);
            let col = ai_color(p);
            let xb = 2.0 * sc + ir * 2.0 + 6.0 * sc;
            let wb = (w - xb - vw - 8.0 * sc).max(10.0 * sc);
            let m = p.metrics.first();
            let pct = m.and_then(|m| m.pct);
            pill(c, xb, y + row / 2.0, wb, (row * 0.16).clamp(3.0, 6.0 * sc), pct.map(|v| v as f32 / 100.0), severity(pct, col));
            let val = m.map(|m| m.value.as_str()).unwrap_or("—");
            c.text(val, w - vw - 2.0 * sc, y, vw, row, px, 600, "Segoe UI", severity(pct, col), DT_RIGHT);
        }
        return;
    }
    // principal: icono grande + nombre + hasta 2 indicadores
    let p = shown[0];
    let col = ai_color(p);
    let ir = (h * 0.30).min(14.0 * sc);
    draw_ai_icon(c, p, 2.0 * sc + ir, h / 2.0, ir);
    let x0 = 2.0 * sc + ir * 2.0 + 7.0 * sc;
    let bw = w - x0 - 2.0 * sc;
    let row = h / 3.0;
    let px = (10.0 * sc).min((row * 0.72).floor());
    c.text(&p.name, x0, 0.0, bw, row, px, 600, "Segoe UI", WHITE, DT_LEFT | DT_END_ELLIPSIS);
    if p.metrics.is_empty() {
        c.text(status_text(&p.status), x0, row, bw, row * 2.0, (px * 0.85).floor(), 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
        return;
    }
    if p.metrics.len() >= 2 {
        for (i, m) in p.metrics.iter().take(2).enumerate() {
            let y = (i + 1) as f32 * row;
            let lw = c.text_width(&metric_label(&m.label), (px * 0.85).floor(), 400, "Segoe UI").min(bw * 0.45);
            c.text(&metric_label(&m.label), x0, y, lw + 2.0, row, (px * 0.85).floor(), 400, "Segoe UI", GAINSBORO, DT_LEFT | DT_END_ELLIPSIS);
            let vw = c.text_width("100%", px, 600, "Segoe UI");
            let xb = x0 + lw + 5.0 * sc;
            pill(c, xb, y + row / 2.0, (bw - lw - vw - 9.0 * sc).max(8.0 * sc), (4.0 * sc).max(3.0), m.pct.map(|v| v as f32 / 100.0), severity(m.pct, col));
            c.text(&m.value, x0 + bw - vw, y, vw, row, px, 600, "Segoe UI", severity(m.pct, col), DT_RIGHT);
        }
    } else {
        let m = &p.metrics[0];
        c.text(&m.value, x0, 0.0, bw, row, px, 700, "Segoe UI", severity(m.pct, col), DT_RIGHT);
        pill(c, x0, row * 1.5, bw, (4.5 * sc).max(3.0), m.pct.map(|v| v as f32 / 100.0), severity(m.pct, col));
        let detail = if m.detail.is_empty() { metric_label(&m.label) } else { m.detail.clone() };
        c.text(&detail, x0, row * 2.0, bw, row, (px * 0.82).floor(), 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    }
}

/// Modo icono (barra con muchas apps): icono del proveedor principal y su valor
fn paint_ai_compact(c: &mut Surface, w: f32, h: f32, sc: f32, view: &AiView) {
    let Some(p) = view.shown().into_iter().next() else { return };
    let r = (w.min(h) * 0.27).min(12.0 * sc);
    draw_ai_icon(c, p, w / 2.0, h * 0.36, r);
    let m = p.metrics.first();
    let pct = m.and_then(|m| m.pct);
    let val = m.map(|m| m.value.as_str()).unwrap_or("—");
    c.text(val, 0.0, h * 0.62, w, h * 0.36, (8.5 * sc).min(h * 0.24), 700, "Segoe UI", severity(pct, ai_color(p)), DT_CENTER);
}

/// Modo icono del sistema: anillo con el uso de CPU y el valor debajo
fn paint_system_compact(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State) {
    let r = (w.min(h) * 0.27).min(12.0 * sc);
    let v = s.cpu.max(0.0);
    let col = usage_color(v, CPU_COL);
    c.donut(w / 2.0, h * 0.36, r, (r * 0.32).max(2.5), v / 100.0, col, col.mix(GPU_COL, 0.5), 2.0 * sc);
    c.text("CPU", w / 2.0 - r, h * 0.36 - r, r * 2.0, r * 2.0, (r * 0.62).floor(), 700, "Segoe UI", DIM, DT_CENTER);
    c.text(&format!("{v:.0}%"), 0.0, h * 0.62, w, h * 0.36, (8.5 * sc).min(h * 0.24), 700, "Segoe UI", col, DT_CENTER);
    level_dot(c, w, sc, sys_level(s));
}

/// Modo icono de la red: dos anillos concéntricos (bajada fuera, subida dentro; cada uno respecto
/// a su máximo del último minuto) y la velocidad de bajada debajo
fn paint_ip_compact(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State) {
    let r = (w.min(h) * 0.27).min(12.0 * sc);
    let (cx, cy) = (w / 2.0, h * 0.36);
    let thick = (r * 0.26).max(2.2);
    // escala de raíz: el tráfico ligero también se ve en el anillo
    let frac = |v: f64, hist: &[f32]| {
        let maxv = hist.iter().fold(8_000.0f32, |m, x| m.max(*x)).max(v as f32);
        (v as f32 / maxv).clamp(0.0, 1.0).sqrt()
    };
    c.donut(cx, cy, r, thick, frac(s.down_bps, &s.hist_down), DOWN_COL, DOWN_COL.mix(WHITE, 0.3), 2.0 * sc);
    let r2 = r - thick - 1.5 * sc;
    c.donut(cx, cy, r2, thick, frac(s.up_bps, &s.hist_up), UP_COL, UP_COL.mix(WHITE, 0.3), 0.0);
    c.text(&rate_short(s.down_bps), 0.0, h * 0.62, w, h * 0.36, (8.5 * sc).min(h * 0.24), 700, "Segoe UI", DOWN_COL, DT_CENTER);
    health_dot(c, w, sc);
}

/// Formato de un panel en la barra: "auto" (icono solo si no cabe), "full" (panel) o "icon"
fn panel_mode(cfg: &Config, k: Kind) -> &'static str {
    // el multimedia no tiene modo icono; los demás se condensan solos si no caben
    if k == Kind::Media {
        return "full";
    }
    match cfg.v["PanelStyle"][k.name()].as_str() {
        Some("icon") => "icon",
        Some("full") => "full",
        _ => "auto",
    }
}

/// Ancho (px lógicos) de un panel en modo icono
fn compact_width(_k: Kind) -> f32 {
    44.0
}

/// Alto (px lógicos) de la ventana flotante de IA según proveedores e indicadores
fn ai_popup_h(view: &AiView) -> f32 {
    let cards: f32 = view.list.iter().map(|p| 38.0 + 48.0 * p.metrics.len().max(1) as f32 + 8.0).sum();
    40.0 + cards.max(60.0) + 6.0
}

/// Ventana flotante de IA: una tarjeta por proveedor con icono, plan, indicadores y reinicios
fn paint_popup_ai(c: &mut Surface, w: f32, h: f32, sc: f32, view: &AiView) {
    let _ = h;
    let mut y = popup_frame(c, w, sc, tr("Consumo de IA", "AI usage"), tr("se actualiza cada 2–5 min", "updates every 2–5 min"));
    let pad = 12.0 * sc;
    let (x, cw) = (pad, w - pad * 2.0);
    if view.list.is_empty() {
        c.text(tr("Agrega proveedores desde el menú IA", "Add providers from the AI menu"), x, y, cw, 40.0 * sc, 10.5 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        return;
    }
    for p in &view.list {
        let col = ai_color(p);
        let ch = (38.0 + 48.0 * p.metrics.len().max(1) as f32) * sc;
        card(c, x, y, cw, ch, sc);
        let ir = 11.0 * sc;
        draw_ai_icon(c, p, x + 10.0 * sc + ir, y + 18.0 * sc, ir);
        let nx = x + 10.0 * sc + ir * 2.0 + 8.0 * sc;
        let nw = c.text_width(&p.name, 12.0 * sc, 600, "Segoe UI");
        c.text(&p.name, nx, y + 7.0 * sc, cw * 0.55, 22.0 * sc, 12.0 * sc, 600, "Segoe UI", WHITE, DT_LEFT | DT_END_ELLIPSIS);
        if !p.plan.is_empty() {
            c.text(&format!("plan {}", p.plan), nx + nw + 8.0 * sc, y + 8.0 * sc, cw * 0.3, 22.0 * sc, 9.5 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        }
        let st = status_text(&p.status);
        if !st.is_empty() {
            c.text(st, x, y + 8.0 * sc, cw - 10.0 * sc, 22.0 * sc, 9.0 * sc, 400, "Segoe UI", Rgb::u8(255, 190, 90), DT_RIGHT | DT_END_ELLIPSIS);
        }
        let (ix, iw) = (x + 10.0 * sc, cw - 20.0 * sc);
        if p.metrics.is_empty() {
            c.text("—", ix, y + 38.0 * sc, iw, 20.0 * sc, 10.0 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        }
        for (i, m) in p.metrics.iter().enumerate() {
            let mut note = if m.detail.is_empty() || m.detail == m.value { String::new() } else { m.detail.clone() };
            let rt = reset_text(m);
            if !rt.is_empty() {
                if !note.is_empty() {
                    note.push_str("  ·  ");
                }
                note.push_str(&rt);
            }
            usage_line(c, ix, y + (36.0 + 48.0 * i as f32) * sc, iw, sc, &metric_label(&m.label), m.pct, &m.value, &note, col);
        }
        y += ch + 8.0 * sc;
    }
}

/// Barra vertical: un bloque por proveedor mostrado
fn paint_ai_v(c: &mut Surface, w: f32, h: f32, sc: f32, view: &AiView) {
    let blocks: Vec<VBlock> = view
        .shown()
        .into_iter()
        .map(|p| {
            let m = p.metrics.first();
            VBlock {
                label: p.name.as_str(),
                value: m.map(|m| m.value.clone()).unwrap_or("—".into()),
                sub: String::new(),
                sub_col: DIM,
                col: severity(m.and_then(|m| m.pct), ai_color(p)),
                hist: None,
                frac: m.and_then(|m| m.pct).map(|v| v as f32 / 100.0),
            }
        })
        .collect();
    if !blocks.is_empty() {
        paint_vblocks(c, w, h, sc, &blocks);
    }
}

const CPU_COL: Rgb = Rgb::u8(0, 230, 255);
const GPU_COL: Rgb = Rgb::u8(120, 255, 90);
const NPU_COL: Rgb = Rgb::u8(190, 120, 255);

/// Color de cada GPU según su fabricante (verde NVIDIA, rojo AMD, azul Intel) y violeta para la NPU
fn vendor_color(g: &state::GpuStat) -> Rgb {
    if g.npu {
        return NPU_COL;
    }
    match g.vendor {
        gpu::VENDOR_AMD => Rgb::u8(255, 85, 85),
        gpu::VENDOR_INTEL => Rgb::u8(80, 175, 255),
        _ => GPU_COL,
    }
}
const FPS_COL: Rgb = Rgb::u8(255, 60, 200);
const RAM_COL: Rgb = Rgb::u8(255, 190, 80);
const DIM: Rgb = Rgb::u8(165, 175, 195);

/// Etiqueta tipo "píldora" (fondo tenue del color + texto en el color); devuelve el ancho
fn badge(c: &mut Surface, x: f32, yc: f32, text: &str, px: f32, col: Rgb, sc: f32, min_w: f32) -> f32 {
    let tw = c.text_width(text, px, 700, "Segoe UI");
    let bw = (tw + 8.0 * sc).max(min_w);
    let bh = (px + 4.0 * sc).round();
    c.round_rect(x, yc - bh / 2.0, bw, bh, bh / 2.0, col, 0.18);
    c.text(text, x, yc - bh / 2.0, bw, bh, px, 700, "Segoe UI", col, DT_CENTER);
    bw
}

/// Cifra en negrita + unidad tenue; devuelve el ancho total
#[allow(clippy::too_many_arguments)]
fn value_unit(c: &mut Surface, x: f32, y: f32, h: f32, num: &str, unit: &str, px: f32, col: Rgb, sc: f32) -> f32 {
    let nw = c.text_width(num, px, 600, "Segoe UI");
    c.text(num, x, y, nw + 2.0, h, px, 600, "Segoe UI", col, DT_LEFT);
    let upx = (px * 0.8).floor();
    let uw = c.text_width(unit, upx, 400, "Segoe UI");
    c.text(unit, x + nw + 1.5 * sc, y, uw + 2.0, h, upx, 400, "Segoe UI", DIM, DT_LEFT);
    nw + 1.5 * sc + uw
}

fn paint_system(c: &mut Surface, full_w: f32, h: f32, sc: f32, s: &state::State) {
    let w = (110.0 * sc).min(full_w);
    paint_disk_mini(c, w + 4.0 * sc, h, sc, s);
    let row = h / 3.0;
    let px = (10.0 * sc).min((row * 0.72).floor());
    let bpx = (px * 0.72).floor();
    let bmin = c.text_width("FPS", bpx, 700, "Segoe UI") + 8.0 * sc;
    let x_val = 2.0 * sc + bmin + 5.0 * sc;
    let fps_max = s.hist_fps.iter().fold(60.0f32, |m, v| m.max(*v)) * 1.15;
    // CPU y GPU: gráfica de uso de fondo, cifra, temperatura a la derecha
    let rows = [("CPU", s.cpu, s.cpu_t, &s.hist_cpu, CPU_COL), ("GPU", s.gpu as f32, s.gpu_t, &s.hist_gpu, GPU_COL)];
    for (i, (label, v, temp, hist, col)) in rows.iter().enumerate() {
        let y = i as f32 * row;
        c.sparkline(0.0, y + 1.0, w, row - 2.0, hist, 100.0, *col, 0.9);
        badge(c, 2.0 * sc, y + row / 2.0, label, bpx, *col, sc, bmin);
        if *v >= 0.0 {
            value_unit(c, x_val, y, row, &format!("{v:.0}"), "%", px, usage_color(*v, *col).mix(WHITE, 0.35), sc);
        }
        let tt = if *temp >= 0 { format!("{temp}°") } else { "—".into() };
        c.text(&tt, x_val, y, w - x_val - 2.0 * sc, row, px, 600, "Segoe UI", temp_color(*temp), DT_RIGHT);
    }
    // FPS: gráfica, cifra y aplicación
    let y = 2.0 * row;
    c.sparkline(0.0, y + 1.0, w, row - 2.0, &s.hist_fps, fps_max, FPS_COL, 0.9);
    badge(c, 2.0 * sc, y + row / 2.0, "FPS", bpx, FPS_COL, sc, bmin);
    let fv = if s.fps > 0 { s.fps.to_string() } else { "—".into() };
    let vw = c.text_width(&fv, px, 600, "Segoe UI");
    c.text(&fv, x_val, y, vw + 2.0, row, px, 600, "Segoe UI", FPS_COL.mix(WHITE, 0.35), DT_LEFT);
    if !s.app.is_empty() {
        let ax = x_val + vw + 5.0 * sc;
        c.text(s.app.trim_end_matches(".exe"), ax, y, w - ax - 2.0 * sc, row, (px * 0.85).floor(), 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    }
    level_dot(c, full_w, sc, sys_level(s));
}

// ======================================================================= ventana flotante del sistema
const POPUP_CLASS: PCWSTR = w!("TaskbarPanelsPopup");
const POPUP_W: f32 = 360.0;

const TEMP_COL: Rgb = Rgb::u8(255, 150, 70);

/// Leyenda mínima en la esquina superior derecha de una gráfica: "▬ uso  ▬ temp"
fn chart_legend(c: &mut Surface, right: f32, top: f32, sc: f32, use_col: Rgb) {
    let px = 8.0 * sc;
    let tw = c.text_width(tr("temp", "temp"), px, 400, "Segoe UI");
    let uw = c.text_width(tr("uso", "use"), px, 400, "Segoe UI");
    let seg = 8.0 * sc;
    let x_t = right - tw - 2.0;
    let x_ts = x_t - seg - 3.0 * sc;
    let x_u = x_ts - 9.0 * sc - uw;
    let x_us = x_u - seg - 3.0 * sc;
    let yc = top + 5.0 * sc;
    c.capsule_h(x_us, x_us + seg, yc, 1.2 * sc, use_col, 1.0);
    c.text(tr("uso", "use"), x_u, top, uw + 2.0, 10.0 * sc, px, 400, "Segoe UI", DIM, DT_LEFT);
    c.capsule_h(x_ts, x_ts + seg, yc, 1.2 * sc, TEMP_COL, 1.0);
    c.text("temp", x_t, top, tw + 2.0, 10.0 * sc, px, 400, "Segoe UI", DIM, DT_LEFT);
}

/// Tarjeta: fondo translúcido redondeado
fn card(c: &mut Surface, x: f32, y: f32, w: f32, h: f32, sc: f32) {
    c.round_rect(x, y, w, h, 8.0 * sc, WHITE, 0.06);
    c.round_rect(x, y, w, (h * 0.45).min(28.0 * sc), 8.0 * sc, WHITE, 0.025);
}

/// Encabezado de tarjeta: píldora + nombre tenue + valor grande a la derecha
#[allow(clippy::too_many_arguments)]
fn card_head(c: &mut Surface, x: f32, y: f32, w: f32, sc: f32, label: &str, name: &str, value: &str, unit: &str, col: Rgb) {
    let (hh, bpx, npx, vpx) = (22.0 * sc, 9.0 * sc, 10.5 * sc, 17.0 * sc);
    let bw = badge(c, x + 10.0 * sc, y + 6.0 * sc + hh / 2.0, label, bpx, col, sc, 0.0);
    let uw = c.text_width(unit, vpx * 0.62, 400, "Segoe UI");
    let vw = c.text_width(value, vpx, 600, "Segoe UI");
    let vx = x + w - 10.0 * sc - uw - 2.0 * sc - vw;
    c.text(value, vx, y + 4.0 * sc, vw + 2.0, hh + 4.0 * sc, vpx, 600, "Segoe UI", col.mix(WHITE, 0.3), DT_LEFT);
    c.text(unit, vx + vw + 2.0 * sc, y + 6.0 * sc, uw + 2.0, hh + 2.0 * sc, vpx * 0.62, 400, "Segoe UI", DIM, DT_LEFT);
    let nx = x + 10.0 * sc + bw + 7.0 * sc;
    c.text(name, nx, y + 6.0 * sc, (vx - nx - 6.0 * sc).max(0.0), hh, npx, 400, "Segoe UI", Rgb::u8(215, 220, 232), DT_LEFT | DT_END_ELLIPSIS);
}

// ----------------------------------------------------------------------- ventanas flotantes: comunes
const DISK_C1: Rgb = Rgb::u8(80, 170, 255);
const DISK_C2: Rgb = Rgb::u8(190, 110, 255);

fn fmt_size(b: f64) -> String {
    if b >= 1e12 { format!("{:.2} TB", b / 1e12) } else { format!("{:.0} GB", b / 1e9) }
}

/// Colores del arco según ocupación (rojo/ámbar cuando el disco está casi lleno)
fn disk_cols(used: f32) -> (Rgb, Rgb) {
    if used >= 0.9 {
        (Rgb::u8(255, 90, 110), Rgb::u8(255, 60, 160))
    } else if used >= 0.75 {
        (Rgb::u8(255, 200, 70), Rgb::u8(255, 140, 60))
    } else {
        (DISK_C1, DISK_C2)
    }
}

fn system_drive<'a>(s: &'a state::State) -> Option<&'a state::Drive> {
    let sysd = std::env::var("SystemDrive").ok().and_then(|v| v.chars().next()).unwrap_or('C');
    s.drives.iter().find(|d| d.letter == sysd).or(s.drives.first())
}

/// Sección del panel: anillo del disco del sistema (% usado); el detalle está en la ventana flotante
fn paint_disk_mini(c: &mut Surface, x0: f32, h: f32, sc: f32, s: &state::State) {
    let Some(d) = system_drive(s) else { return };
    let used = (1.0 - d.free / d.total) as f32;
    let dia = (h * 0.80).min(32.0 * sc);
    let r = dia / 2.0;
    let (cx, cy) = (x0 + r, h / 2.0);
    let (c1, c2) = disk_cols(used);
    c.donut(cx, cy, r, (r * 0.30).max(3.0), used, c1, c2, 2.5 * sc);
    c.text(&format!("{:.0}", used * 100.0), cx - r, cy - r, dia, dia, (r * 0.62).floor(), 700, "Segoe UI", WHITE, DT_CENTER);
}

/// Alto (px lógicos) de la tarjeta de discos de la ventana flotante
fn disk_card_h(n: usize) -> f32 {
    34.0 + n.max(1).div_ceil(2) as f32 * 54.0 + 56.0
}

/// Tarjeta "Discos": un anillo por unidad (2 columnas) + lectura/escritura con gráficas
fn paint_disk_card(c: &mut Surface, x: f32, y: f32, cw: f32, sc: f32, s: &state::State) -> f32 {
    let ch = disk_card_h(s.drives.len()) * sc;
    card(c, x, y, cw, ch, sc);
    let total_free: f64 = s.drives.iter().map(|d| d.free).sum();
    let n = s.drives.len();
    let summary = if i18n::es() { format!("{n} unidad{} · {} libres", if n == 1 { "" } else { "es" }, fmt_size(total_free)) } else { format!("{n} drive{} · {} free", if n == 1 { "" } else { "s" }, fmt_size(total_free)) };
    card_head(c, x, y, cw, sc, tr("DISCOS", "DRIVES"), &summary, "", "", DISK_C1);
    let col_w = (cw - 20.0 * sc) / 2.0;
    for (i, d) in s.drives.iter().enumerate() {
        let (cx0, cy0) = (x + 10.0 * sc + (i % 2) as f32 * col_w, y + 34.0 * sc + (i / 2) as f32 * 54.0 * sc);
        let used = (1.0 - d.free / d.total) as f32;
        let (c1, c2) = disk_cols(used);
        let r = 21.0 * sc;
        c.donut(cx0 + r, cy0 + r + 2.0 * sc, r, 5.0 * sc, used, c1, c2, 3.0 * sc);
        c.text(&format!("{:.0}%", used * 100.0), cx0, cy0 + 2.0 * sc, r * 2.0, r * 2.0, 10.0 * sc, 700, "Segoe UI", WHITE, DT_CENTER);
        let tx = cx0 + r * 2.0 + 8.0 * sc;
        let tw = col_w - r * 2.0 - 12.0 * sc;
        let name = if d.label.is_empty() { format!("{}:", d.letter) } else { format!("{}:  {}", d.letter, d.label) };
        c.text(&name, tx, cy0 + 2.0 * sc, tw, 15.0 * sc, 10.5 * sc, 600, "Segoe UI", Rgb::u8(225, 230, 240), DT_LEFT | DT_END_ELLIPSIS);
        c.text(&format!("{} {}", fmt_size(d.free), tr("libres", "free")), tx, cy0 + 17.0 * sc, tw, 14.0 * sc, 9.5 * sc, 600, "Segoe UI", c1.mix(WHITE, 0.3), DT_LEFT | DT_END_ELLIPSIS);
        c.text(&format!("{} {}", tr("de", "of"), fmt_size(d.total)), tx, cy0 + 30.0 * sc, tw, 14.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    }
    // lectura / escritura
    let ry = y + 34.0 * sc + n.max(1).div_ceil(2) as f32 * 54.0 * sc;
    let maxv = s.hist_disk_r.iter().chain(s.hist_disk_w.iter()).fold(1e6f32, |m, v| m.max(*v)) * 1.15;
    for (i, (label, v, hist, col)) in [(tr("LECTURA", "READ"), s.disk_read, &s.hist_disk_r, DOWN_COL), (tr("ESCRITURA", "WRITE"), s.disk_write, &s.hist_disk_w, UP_COL)].into_iter().enumerate() {
        let cx0 = x + 10.0 * sc + i as f32 * col_w;
        let bw = badge(c, cx0, ry + 8.0 * sc, label, 8.0 * sc, col, sc, 0.0);
        c.text(&format!("{}/s", fmt_bytes(v)), cx0 + bw + 6.0 * sc, ry, col_w - bw - 12.0 * sc, 16.0 * sc, 10.5 * sc, 600, "Segoe UI", col.mix(WHITE, 0.35), DT_LEFT);
        c.sparkline(cx0, ry + 20.0 * sc, col_w - 8.0 * sc, 28.0 * sc, hist, maxv, col, 1.0);
    }
    ch
}

/// alto (px lógicos, bits de f32) de la ventana flotante de IA, actualizado al dibujar los paneles
static AI_POPUP_H: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn popup_h(kind: Kind) -> f32 {
    match kind {
        Kind::System => sys_popup_h(&st()),
        Kind::Ip => 324.0 + ROUTER_CARD_H + 8.0 + SPEED_CARD_H + 8.0 + if net::health_on() { HEALTH_CARD_H + 8.0 } else { 0.0 },
        Kind::Usage => f32::from_bits(AI_POPUP_H.load(std::sync::atomic::Ordering::Relaxed)).max(120.0),
        Kind::Media => 244.0,
        Kind::Radio => RADIO_POPUP_H,
    }
}

thread_local! {
    /// estilo de fondo con el que se dibujan las ventanas flotantes
    static POPUP_STYLE: RefCell<String> = RefCell::new("glass".into());
}

/// Fondo según el estilo + título y texto secundario a la derecha; devuelve la y de inicio
fn popup_frame(c: &mut Surface, w: f32, sc: f32, title: &str, right: &str) -> f32 {
    let style = POPUP_STYLE.with(|s| s.borrow().clone());
    c.frosted(sc, &style);
    let pad = 12.0 * sc;
    c.text(title, pad + 2.0 * sc, pad, w - pad * 2.0, 20.0 * sc, 13.0 * sc, 600, "Segoe UI", WHITE, DT_LEFT);
    c.text(right, pad, pad, w - pad * 2.0 - 2.0 * sc, 20.0 * sc, 9.5 * sc, 400, "Segoe UI", DIM, DT_RIGHT | DT_END_ELLIPSIS);
    pad + 28.0 * sc
}

fn fmt_bytes(b: f64) -> String {
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Reparte un texto en dos líneas por palabras (la segunda se corta con "…" al dibujarla)
fn wrap2(c: &Surface, text: &str, px: f32, weight: i32, width: f32) -> (String, String) {
    let mut first = String::new();
    let words: Vec<&str> = text.split(' ').collect();
    let mut i = 0;
    while i < words.len() {
        let cand = if first.is_empty() { words[i].to_string() } else { format!("{first} {}", words[i]) };
        if c.text_width(&cand, px, weight, "Segoe UI") > width && !first.is_empty() {
            break;
        }
        first = cand;
        i += 1;
    }
    (first, words[i..].join(" "))
}

// ----------------------------------------------------------------------- router (ventana de red)
const ROUTER_CARD_H: f32 = 118.0;

/// Router dibujado: antenas, ondas Wi-Fi, cuerpo redondeado con reflejo y LED con halo
#[allow(clippy::too_many_arguments)]
fn draw_router(c: &mut Surface, x: f32, y: f32, w: f32, h: f32, sc: f32, t: f32, online: bool, reply_flash: f32, wifi: bool) {
    let body_y = y + h * 0.46;
    let body_h = h * 0.40;
    let led_ok = Rgb::u8(60, 230, 140);
    let led_bad = Rgb::u8(255, 80, 100);
    let accent = Rgb::u8(110, 170, 255);
    // antenas
    for ax in [x + w * 0.22, x + w * 0.78] {
        c.capsule(ax, y + h * 0.10, body_y + 2.0 * sc, 2.2 * sc, Rgb::u8(120, 128, 155), 1.0, 0.0, 0.0, 0.35);
        c.dot(ax, y + h * 0.10, 2.8 * sc, Rgb::u8(150, 158, 185), 1.0, 0.0, 0.0);
    }
    // ondas Wi-Fi (arcos) animadas sobre el centro
    if wifi && online {
        let (cx, cy) = (x + w / 2.0, y + h * 0.38);
        for (i, r) in [7.0f32, 12.0, 17.0].iter().enumerate() {
            let phase = ((t * 1.6 - i as f32 * 0.35).sin() * 0.5 + 0.5) * 0.6 + 0.25;
            let rr = r * sc;
            // arco superior (de -50° a +50°) dibujado por puntos
            let steps = (rr * 2.2) as i32;
            for k in 0..=steps {
                let a = -0.9 + 1.8 * k as f32 / steps as f32;
                c.dot(cx + rr * a.sin(), cy - rr * a.cos(), 1.1 * sc, accent, phase, 0.0, 0.0);
            }
        }
    }
    // cuerpo
    c.round_rect(x, body_y, w, body_h, 7.0 * sc, Rgb::u8(46, 50, 70), 1.0);
    c.round_rect(x, body_y, w, body_h * 0.5, 7.0 * sc, WHITE, 0.07); // reflejo superior
    c.fill(x + 8.0 * sc, body_y + body_h * 0.62, w - 16.0 * sc, 1.0 * sc.max(1.0), WHITE, 0.08); // línea frontal
    // LED: encendido, Wi-Fi, ping (destella con cada respuesta), internet
    let leds = [
        (led_ok, 1.0),
        (if wifi { accent } else { Rgb::u8(90, 96, 120) }, if wifi { 0.75 + 0.25 * (t * 3.0).sin() } else { 0.4 }),
        (Rgb::u8(255, 210, 90), 0.25 + 0.75 * reply_flash),
        (if online { led_ok } else { led_bad }, 1.0),
    ];
    let ly = body_y + body_h * 0.36;
    for (i, (col, a)) in leds.iter().enumerate() {
        let lx = x + w * (0.24 + 0.17 * i as f32);
        c.dot(lx, ly, 2.0 * sc, *col, *a, 3.0 * sc, 0.45 * a);
    }
}

fn ping_color(ms: Option<u32>) -> Rgb {
    match ms {
        None => Rgb::u8(255, 80, 100),
        Some(v) if v < 10 => Rgb::u8(60, 230, 140),
        Some(v) if v < 50 => Rgb::u8(255, 210, 90),
        _ => Rgb::u8(255, 140, 70),
    }
}

const ROUTER_W: f32 = 84.0;
const ROUTER_H: f32 = 74.0;

/// Router con su estado en vivo: ondas si hay Wi-Fi, LED de ping que destella con cada respuesta
fn draw_router_live(c: &mut Surface, x: f32, y: f32, sc: f32, t: f32) {
    let wifi = net::INFO.lock().unwrap().as_ref().map(|i| i.wifi).unwrap_or(false);
    let (flash, online) = {
        let p = net::PING.lock().unwrap();
        (p.reply_at.map(|r| (1.0 - r.elapsed().as_secs_f32() / 0.35).clamp(0.0, 1.0)).unwrap_or(0.0), p.last.is_some() || p.sent == 0)
    };
    draw_router(c, x, y, ROUTER_W * sc, ROUTER_H * sc, sc, t, online, flash, wifi);
}

/// Tarjeta del router: imagen, puerta de enlace, DNS, adaptador y ping con gráfica
fn paint_router_card(c: &mut Surface, x: f32, y: f32, cw: f32, sc: f32, t: f32) -> f32 {
    let ch = ROUTER_CARD_H * sc;
    card(c, x, y, cw, ch, sc);
    let info = net::INFO.lock().unwrap().clone();
    let p = net::PING.lock().unwrap().clone();
    let (rw, rh) = (ROUTER_W * sc, ROUTER_H * sc);
    // el dibujo del router (ondas, LED) es lo único animado de la ventana: zona aparte
    anim_zone(c, ZONE_ROUTER, x + 12.0 * sc, y + (ch - rh) / 2.0, rw, rh);
    draw_router_live(c, x + 12.0 * sc, y + (ch - rh) / 2.0, sc, t);
    let tx = x + 12.0 * sc + rw + 14.0 * sc;
    let tw = x + cw - 10.0 * sc - tx;
    let Some(i) = info else {
        c.text(tr("Leyendo la red local…", "Reading local network…"), tx, y, tw, ch, 10.5 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        return ch;
    };
    // puerta de enlace
    let bw = badge(c, tx, y + 18.0 * sc, tr("PUERTA DE ENLACE", "GATEWAY"), 8.0 * sc, Rgb::u8(170, 200, 255), sc, 0.0);
    let _ = bw;
    c.text(&i.gateway, tx, y + 26.0 * sc, tw, 22.0 * sc, 15.0 * sc, 600, "Segoe UI", WHITE, DT_LEFT);
    // ping a la derecha de la puerta de enlace
    let pc = ping_color(p.last);
    let pv = match (p.sent, p.last) {
        (0, _) => "…".to_string(),
        (_, Some(ms)) => if ms == 0 { "<1 ms".into() } else { format!("{ms} ms") },
        (_, None) => tr("sin respuesta", "no reply").into(),
    };
    c.text(&pv, tx, y + 26.0 * sc, tw, 22.0 * sc, 13.0 * sc, 700, "Segoe UI", pc, DT_RIGHT);
    // DNS y adaptador
    let dns = if i.dns.is_empty() { "—".to_string() } else { i.dns.iter().take(3).cloned().collect::<Vec<_>>().join("  ·  ") };
    c.text(&format!("DNS  {dns}"), tx, y + 50.0 * sc, tw, 15.0 * sc, 9.5 * sc, 400, "Segoe UI", Rgb::u8(205, 212, 228), DT_LEFT | DT_END_ELLIPSIS);
    let speed = if i.link_bps >= 1_000_000_000 { format!("{:.1} Gb/s", i.link_bps as f64 / 1e9) } else { format!("{} Mb/s", i.link_bps / 1_000_000) };
    let kind = if i.wifi { "Wi-Fi" } else { "Ethernet" };
    let name = if i.adapter.eq_ignore_ascii_case(kind) { format!("{kind} {speed}") } else { format!("{}  ·  {kind} {speed}", i.adapter) };
    c.text(&format!("{name}  ·  {} {}", tr("IP local", "local IP"), i.local_ip), tx, y + 65.0 * sc, tw, 15.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    // gráfica del ping + estadísticas
    let gy = y + 84.0 * sc;
    let ok = p.sent - p.lost;
    let stats = if ok > 0 {
        let loss = p.lost as f32 * 100.0 / p.sent.max(1) as f32;
        if i18n::es() {
            format!("mín {} · media {} · máx {} ms · pérdida {loss:.0}%", p.min, p.sum / ok as u64, p.max)
        } else {
            format!("min {} · avg {} · max {} ms · loss {loss:.0}%", p.min, p.sum / ok as u64, p.max)
        }
    } else if p.sent > 0 {
        if i18n::es() { format!("pérdida 100% ({} enviados)", p.sent) } else { format!("loss 100% ({} sent)", p.sent) }
    } else {
        tr("midiendo…", "measuring…").into()
    };
    c.text(&stats, tx, gy + 12.0 * sc, tw, 14.0 * sc, 8.5 * sc, 400, "Segoe UI", DIM, DT_RIGHT | DT_END_ELLIPSIS);
    let pmax = p.hist.iter().fold(10.0f32, |m, v| m.max(*v)) * 1.2;
    c.sparkline(tx, gy - 2.0 * sc, tw, 16.0 * sc, &p.hist, pmax, pc, 0.9);
    ch
}

// ----------------------------------------------------------------------- salud de la conexión
const HEALTH_CARD_H: f32 = 104.0;
const HEALTH_BAD: Rgb = Rgb::u8(255, 70, 90);

fn level_color(l: net::Level) -> Rgb {
    match l {
        net::Level::Ok => Rgb::u8(60, 230, 140),
        net::Level::Warn => Rgb::u8(255, 200, 60),
        net::Level::Bad => HEALTH_BAD,
        net::Level::Unknown => DIM,
    }
}

/// Diagnóstico en una frase y dónde está el problema
fn health_text(level: net::Level, cause: net::Cause, wifi: bool) -> (&'static str, &'static str) {
    use net::Cause::*;
    match cause {
        NoNetwork => (tr("Sin conexión de red", "No network connection"), tr("Ningún adaptador conectado con puerta de enlace", "No connected adapter with a gateway")),
        RouterDown => (tr("El router no responde", "Router not responding"), tr("El problema está entre este equipo y el router", "The problem is between this PC and the router")),
        Local => (
            if wifi { tr("Wi-Fi inestable", "Unstable Wi-Fi") } else { tr("Red local inestable", "Unstable local network") },
            tr("El router responde lento o pierde paquetes", "The router replies slowly or drops packets"),
        ),
        NoInternet => (tr("Sin acceso a Internet", "No Internet access"), tr("El router responde, pero Internet no (proveedor)", "The router replies but the Internet does not (provider)")),
        InternetLoss => (tr("Internet con pérdidas", "Internet dropping packets"), tr("La red local va bien: las pérdidas son del proveedor", "Local network is fine: the loss is on the provider side")),
        InternetSlow => (tr("Internet con latencia alta", "High Internet latency"), tr("La red local va bien: la lentitud es del proveedor", "Local network is fine: the delay is on the provider side")),
        Dns => (tr("DNS lento", "Slow DNS"), tr("El servidor DNS tarda en responder", "The DNS server is slow to reply")),
        None if level == net::Level::Ok => (tr("Conexión estable", "Connection stable"), tr("Router, DNS e Internet responden bien", "Router, DNS and Internet respond well")),
        None => (tr("Midiendo la conexión…", "Measuring connection…"), ""),
    }
}

/// Punto de salud de la conexión en la esquina superior derecha del panel de red: discreto si
/// todo va bien, con halo si hay degradación o corte
fn health_dot(c: &mut Surface, w: f32, sc: f32) {
    if !net::health_on() {
        return;
    }
    let (level, _) = net::HEALTH.lock().unwrap().verdict();
    let col = level_color(level);
    let (cx, cy) = (w - 4.5 * sc, 4.5 * sc);
    match level {
        net::Level::Unknown => {}
        net::Level::Ok => c.dot(cx, cy, 1.8 * sc, col, 0.9, 0.0, 0.0),
        net::Level::Warn => c.dot(cx, cy, 2.4 * sc, col, 1.0, 3.0 * sc, 0.5),
        net::Level::Bad => c.dot(cx, cy, 2.6 * sc, col, 1.0, 4.0 * sc, 0.7),
    }
}

/// Tarjeta de salud: diagnóstico y, por tramo (router, DNS, Internet), línea de tiempo de los
/// últimos 2 minutos (altura = latencia en escala logarítmica; rojo a toda altura = sin respuesta)
fn paint_health_card(c: &mut Surface, x: f32, y: f32, cw: f32, sc: f32) -> f32 {
    let ch = HEALTH_CARD_H * sc;
    card(c, x, y, cw, ch, sc);
    let h = net::HEALTH.lock().unwrap().clone();
    let (level, cause) = h.verdict();
    let col = level_color(level);
    let (title, detail) = health_text(level, cause, h.wifi);
    let bw = badge(c, x + 10.0 * sc, y + 17.0 * sc, tr("CONEXIÓN", "CONNECTION"), 9.0 * sc, Rgb::u8(170, 200, 255), sc, 0.0);
    c.dot(x + 10.0 * sc + bw + 12.0 * sc, y + 17.0 * sc, 3.0 * sc, col, 1.0, 3.0 * sc, 0.5);
    let ix = x + 10.0 * sc + bw + 22.0 * sc;
    c.text(title, ix, y + 5.0 * sc, x + cw - 10.0 * sc - ix, 24.0 * sc, 14.0 * sc, 600, "Segoe UI", col.mix(WHITE, 0.35), DT_LEFT | DT_END_ELLIPSIS);
    c.text(detail, x + 10.0 * sc, y + 31.0 * sc, cw - 20.0 * sc, 15.0 * sc, 9.5 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    // color de una muestra según los umbrales del tramo
    let sample_col = |ms: f32, t1: f32, t2: f32| {
        if ms < t1 { Rgb::u8(60, 230, 140) } else if ms < t2 { Rgb::u8(255, 210, 90) } else { Rgb::u8(255, 140, 70) }
    };
    let hops = [("Router", &h.router, 10.0f32, 50.0f32), ("DNS", &h.dns, 60.0, 150.0), ("Internet", &h.internet, 60.0, 150.0)];
    let (lx, lw, vw, rh) = (x + 10.0 * sc, 50.0 * sc, 92.0 * sc, 16.0 * sc);
    let (gx, gw) = (lx + lw, cw - 20.0 * sc - lw - vw - 6.0 * sc);
    for (k, (name, hop, t1, t2)) in hops.iter().enumerate() {
        let ry = y + 50.0 * sc + k as f32 * rh;
        c.text(name, lx, ry, lw, rh, 9.5 * sc, 400, "Segoe UI", Rgb::u8(205, 212, 228), DT_LEFT);
        let (base, maxh) = (ry + rh - 3.0 * sc, rh - 5.0 * sc);
        c.fill(gx, base, gw, sc.max(1.0), WHITE, 0.10);
        let bw = gw / net::HEALTH_SAMPLES as f32;
        for (i, v) in hop.hist.iter().enumerate() {
            if *v < 0.0 {
                continue;
            }
            let (hh, scol) = if *v == 0.0 {
                (maxh, HEALTH_BAD)
            } else {
                (maxh * ((1.0 + v).ln() / 301.0f32.ln()).clamp(0.14, 1.0), sample_col(*v, *t1, *t2))
            };
            c.fill(gx + i as f32 * bw, base - hh, (bw - 1.0).max(1.0), hh, scol, 0.95);
        }
        let (txt, vcol): (String, Rgb) = if hop.target.is_empty() {
            // sin DNS propio: lo resuelve el router, que ya se mide en su fila
            (if k == 1 && !h.router.target.is_empty() { "= router".into() } else { "—".into() }, DIM)
        } else {
            match hop.recent(10) {
                None => ("…".into(), DIM),
                Some(_) if !hop.ever => (tr("no admite ping", "no ping reply").into(), DIM),
                Some((loss, _)) if loss >= 0.99 => (tr("sin respuesta", "no reply").into(), HEALTH_BAD),
                Some((loss, ms)) => {
                    let lat = if ms < 1.5 { "<1 ms".to_string() } else { format!("{:.0} ms", ms - 0.5) };
                    if loss > 0.0 {
                        let lcol = if loss >= 0.3 { HEALTH_BAD } else { Rgb::u8(255, 200, 60) };
                        (format!("{lat} · {:.0}% {}", loss * 100.0, tr("perd.", "loss")), lcol)
                    } else {
                        (lat, sample_col(ms, *t1, *t2))
                    }
                }
            }
        };
        c.text(&txt, x + cw - 10.0 * sc - vw, ry, vw, rh, 9.5 * sc, 600, "Segoe UI", vcol, DT_RIGHT);
    }
    ch
}

// ----------------------------------------------------------------------- prueba de velocidad (ventana de red)
const SPEED_CARD_H: f32 = 124.0;
const ACT_SPEEDTEST: u32 = 1;
const ACT_SPEED_PROVIDER: u32 = 2;

thread_local! {
    /// zonas pulsables de la ventana flotante (en coordenadas de la ventana) y la que tiene el cursor
    static POPUP_HITS: RefCell<Vec<(f32, f32, f32, f32, u32)>> = const { RefCell::new(Vec::new()) };
    static POPUP_HOVER: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Zona pulsable bajo el cursor. Cada zona pertenece a una ventana y las de la otra siguen en la
/// lista: solo se miran las de la ventana abierta (`radio`), o una zona vieja de la radio taparía
/// el botón del speed test.
fn popup_hit(x: f32, y: f32, radio: bool) -> u32 {
    POPUP_HITS.with(|h| h.borrow().iter().find(|r| (r.4 >= ACT_RADIO_TOGGLE) == radio && x >= r.0 && x < r.2 && y >= r.1 && y < r.3).map(|r| r.4).unwrap_or(0))
}

fn fmt_mbps(v: f64) -> String {
    if v >= 1000.0 { format!("{:.2} Gb/s", v / 1000.0) } else if v >= 100.0 { format!("{v:.0} Mb/s") } else { format!("{v:.1} Mb/s") }
}

/// Card de speed test: medidor circular en vivo, resultados y botón para iniciar
fn paint_speed_card(c: &mut Surface, x: f32, y: f32, cw: f32, sc: f32) -> f32 {
    use speedtest::Phase;
    let ch = SPEED_CARD_H * sc;
    card(c, x, y, cw, ch, sc);
    let s = speedtest::SPEED.lock().unwrap().clone();
    let busy = matches!(s.phase, Phase::Latency | Phase::Download | Phase::Upload);
    // ---- medidor (escala logarítmica hasta 2,5 Gb/s)
    let r = 38.0 * sc;
    let (gx, gy) = (x + 14.0 * sc + r, y + ch / 2.0);
    let (val, col) = match s.phase {
        Phase::Download => (s.live_mbps, DOWN_COL),
        Phase::Upload => (s.live_mbps, UP_COL),
        _ => (s.down_mbps.unwrap_or(0.0), DOWN_COL),
    };
    let frac = ((1.0 + val).ln() / (1.0 + 2500.0f64).ln()) as f32;
    c.donut(gx, gy, r, 7.0 * sc, frac, col, col.mix(Rgb::u8(150, 70, 255), 0.6), 3.0 * sc);
    let num = if val >= 1000.0 { format!("{:.2}", val / 1000.0) } else if val >= 100.0 { format!("{val:.0}") } else if val > 0.0 { format!("{val:.1}") } else { "—".into() };
    c.text(&num, gx - r, gy - 15.0 * sc, r * 2.0, 22.0 * sc, 17.0 * sc, 700, "Segoe UI", WHITE, DT_CENTER);
    c.text(if val >= 1000.0 { "Gb/s" } else { "Mb/s" }, gx - r, gy + 6.0 * sc, r * 2.0, 14.0 * sc, 8.5 * sc, 400, "Segoe UI", DIM, DT_CENTER);
    // ---- cabecera
    let tx = gx + r + 16.0 * sc;
    let tw = x + cw - 10.0 * sc - tx;
    badge(c, tx, y + 18.0 * sc, "SPEED TEST", 8.0 * sc, Rgb::u8(120, 220, 255), sc, 0.0);
    // selector de proveedor (clic → siguiente); durante la prueba muestra el que se está usando
    let prov = if busy { speedtest::provider_name(&s.provider) } else { speedtest::provider_name(speedtest::selected()) };
    let chip = format!("{prov}  ⇄");
    let chip_w = c.text_width(&chip, 8.5 * sc, 600, "Segoe UI") + 14.0 * sc;
    let (cx0, cy0, ch0) = (x + cw - 10.0 * sc - chip_w, y + 9.0 * sc, 17.0 * sc);
    let chip_hover = !busy && POPUP_HOVER.with(|h| h.get()) == ACT_SPEED_PROVIDER;
    c.round_rect(cx0, cy0, chip_w, ch0, ch0 / 2.0, Rgb::u8(120, 220, 255), if chip_hover { 0.32 } else { 0.14 });
    c.text(&chip, cx0, cy0, chip_w, ch0, 8.5 * sc, 600, "Segoe UI", if busy { DIM } else { Rgb::u8(170, 230, 255) }, DT_CENTER);
    POPUP_HITS.with(|h| {
        let mut h = h.borrow_mut();
        h.retain(|r| r.4 != ACT_SPEED_PROVIDER);
        if !busy {
            h.push((cx0, cy0, cx0 + chip_w, cy0 + ch0, ACT_SPEED_PROVIDER));
        }
    });
    // servidor y hora del último resultado (si es del proveedor elegido)
    let meta = match (&s.finished_at, s.server.is_empty()) {
        (Some(t), false) => format!("{}  ·  {}", s.server, util::fmt_local(*t)),
        (Some(t), true) => util::fmt_local(*t),
        (None, false) => s.server.clone(),
        _ => String::new(),
    };
    // ---- resultados
    let row = |c: &mut Surface, yy: f32, label: &str, value: String, col: Rgb| {
        c.text(label, tx, yy, tw, 16.0 * sc, 10.0 * sc, 400, "Segoe UI", Rgb::u8(205, 212, 228), DT_LEFT);
        c.text(&value, tx, yy, tw, 16.0 * sc, 11.0 * sc, 700, "Segoe UI", col, DT_RIGHT);
    };
    let dv = s.down_mbps.map(fmt_mbps).unwrap_or(if s.phase == Phase::Download { fmt_mbps(s.live_mbps) } else { "—".into() });
    let uv = s.up_mbps.map(fmt_mbps).unwrap_or(if s.phase == Phase::Upload { fmt_mbps(s.live_mbps) } else { "—".into() });
    row(c, y + 30.0 * sc, &format!("↓  {}", tr("Bajada", "Download")), dv, DOWN_COL.mix(WHITE, 0.3));
    row(c, y + 47.0 * sc, &format!("↑  {}", tr("Subida", "Upload")), uv, UP_COL.mix(WHITE, 0.3));
    let pv = match (s.ping_ms, s.jitter_ms) {
        (Some(p), Some(j)) => format!("{p:.0} ms  ·  jitter {j:.0} ms"),
        _ => "—".into(),
    };
    row(c, y + 64.0 * sc, &format!("◷  {}", tr("Latencia", "Latency")), pv, Rgb::u8(255, 210, 90));
    // ---- botón o progreso
    let (bx, by, bw, bh) = (tx, y + ch - 32.0 * sc, tw, 24.0 * sc);
    if busy {
        let label = match s.phase {
            Phase::Latency => tr("Midiendo latencia…", "Measuring latency…"),
            Phase::Download => tr("Midiendo bajada…", "Measuring download…"),
            _ => tr("Midiendo subida…", "Measuring upload…"),
        };
        c.text(label, bx, by - 2.0 * sc, bw, 14.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        c.text(&meta, bx + bw * 0.52, by - 2.0 * sc, bw * 0.48, 14.0 * sc, 8.0 * sc, 400, "Segoe UI", DIM, DT_RIGHT | DT_END_ELLIPSIS);
        c.capsule_h(bx, bx + bw, by + 17.0 * sc, 2.5 * sc, WHITE, 0.14);
        c.capsule_h(bx, bx + (bw * s.progress).max(5.0 * sc), by + 17.0 * sc, 2.5 * sc, col, 1.0);
        POPUP_HITS.with(|h| h.borrow_mut().retain(|r| r.4 != ACT_SPEEDTEST));
    } else {
        let hover = POPUP_HOVER.with(|h| h.get()) == ACT_SPEEDTEST;
        // botón sólido: en modo claro, azul de acento oscuro con texto blanco (sin adaptar)
        let light = render::is_light();
        let fill = if light { Rgb::u8(0, 95, 184) } else { Rgb::u8(60, 160, 255) };
        c.raw(|c| c.round_rect(bx, by, bw, bh, bh / 2.0, fill, if hover || light { 0.95 } else { 0.75 }));
        let label = match s.phase {
            Phase::Idle => tr("Iniciar prueba", "Start test"),
            Phase::Error => tr("Reintentar (error)", "Retry (error)"),
            _ => tr("Probar de nuevo", "Test again"),
        };
        c.raw(|c| c.text(label, bx, by, bw, bh, 10.0 * sc, 600, "Segoe UI", WHITE, DT_CENTER));
        let note = if s.phase == Phase::Error && !s.error.is_empty() { s.error.clone() } else if meta.is_empty() { tr("~20 s · usa datos según la velocidad", "~20 s · data use grows with speed").to_string() } else { meta.clone() };
        c.text(&note, bx, by - 15.0 * sc, bw, 13.0 * sc, 8.0 * sc, 400, "Segoe UI", DIM, DT_RIGHT | DT_END_ELLIPSIS);
        POPUP_HITS.with(|h| {
            let mut h = h.borrow_mut();
            h.retain(|r| r.4 != ACT_SPEEDTEST);
            h.push((bx, by, bx + bw, by + bh, ACT_SPEEDTEST));
        });
    }
    ch
}

// ----------------------------------------------------------------------- Red
/// `anim`: entre dos pintados completos solo se repinta el dibujo del router (salvo durante un
/// speed test, que anima su propia tarjeta)
fn paint_popup_ip(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, t: f32, anim: bool) {
    if anim && !speedtest::running() && paint_anim(c, sc, s, t) {
        return;
    }
    SYS_ANIM.with(|a| *a.borrow_mut() = ((c.w, c.h), Vec::new()));
    let mut y = popup_frame(c, w, sc, tr("Red", "Network"), &format!("{}: ↓ {}  ↑ {}", tr("sesión", "session"), fmt_bytes(s.sess_down), fmt_bytes(s.sess_up)));
    let pad = 12.0 * sc;
    let (x, cw) = (pad, w - pad * 2.0);
    // Salud de la conexión: diagnóstico y línea de tiempo por tramo
    if net::health_on() {
        y += paint_health_card(c, x, y, cw, sc) + 8.0 * sc;
    }
    // IP pública
    let ih = 64.0 * sc;
    card(c, x, y, cw, ih, sc);
    let online = s.ip.contains('.') || s.ip.contains(':');
    let dot = if online { Rgb::u8(60, 230, 120) } else { Rgb::u8(255, 70, 90) };
    let bw = badge(c, x + 10.0 * sc, y + 17.0 * sc, tr("IP PÚBLICA", "PUBLIC IP"), 9.0 * sc, Rgb::u8(170, 200, 255), sc, 0.0);
    c.dot(x + 10.0 * sc + bw + 12.0 * sc, y + 17.0 * sc, 3.0 * sc, dot, 1.0, 3.0 * sc, 0.5);
    // bandera del país de la IP a la izquierda de la IP
    let mut ix = x + 10.0 * sc + bw + 20.0 * sc;
    if online {
        let fw = draw_flag(c, ix, y + 17.0 * sc, 14.0 * sc, sc);
        if fw > 0.0 {
            ix += fw + 7.0 * sc;
        }
    }
    c.text(&s.ip, ix, y + 4.0 * sc, x + cw - 10.0 * sc - ix, 26.0 * sc, 16.0 * sc, 600, "Segoe UI", WHITE, DT_LEFT);
    let isp = [s.isp.as_str(), s.city.as_str()].iter().filter(|v| !v.is_empty()).copied().collect::<Vec<_>>().join("   ·   ");
    c.text(if isp.is_empty() { tr("ISP desconocido", "Unknown ISP") } else { &isp }, x + 10.0 * sc, y + 34.0 * sc, cw - 20.0 * sc, 22.0 * sc, 10.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    y += ih + 8.0 * sc;
    // Router: puerta de enlace, DNS y ping (solo mientras esta ventana está abierta)
    y += paint_router_card(c, x, y, cw, sc, t) + 8.0 * sc;
    // Speed test (bajo demanda)
    y += paint_speed_card(c, x, y, cw, sc) + 8.0 * sc;
    // Descarga / subida
    let maxv = s.hist_down.iter().chain(s.hist_up.iter()).fold(8_000.0f32, |m, v| m.max(*v)) * 1.15;
    let ch = (h - y - pad - 8.0 * sc) / 2.0;
    let items = [(tr("BAJADA", "DOWNLOAD"), s.down_bps, &s.hist_down, DOWN_COL, s.sess_down), (tr("SUBIDA", "UPLOAD"), s.up_bps, &s.hist_up, UP_COL, s.sess_up)];
    for (label, bps, hist, col, total) in items {
        card(c, x, y, cw, ch, sc);
        let (num, unit) = rate_parts(bps);
        let max = hist.iter().fold(0f32, |m, v| m.max(*v)) as f64;
        let (mn, mu) = rate_parts(max);
        card_head(c, x, y, cw, sc, label, &format!("{} {mn} {mu}", tr("máx. 60 s:", "60 s max:")), &num, unit, col);
        c.text(&format!("{} {}", tr("total de la sesión:", "session total:"), fmt_bytes(total)), x + 10.0 * sc, y + 30.0 * sc, cw - 20.0 * sc, 14.0 * sc, 9.5 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        c.sparkline(x + 10.0 * sc, y + 48.0 * sc, cw - 20.0 * sc, (ch - 54.0 * sc).max(4.0), hist, maxv, col, 1.0);
        y += ch + 8.0 * sc;
    }
}

// ----------------------------------------------------------------------- Consumo
#[allow(clippy::too_many_arguments)]
fn usage_line(c: &mut Surface, x: f32, y: f32, w: f32, sc: f32, label: &str, pct: Option<f64>, value: &str, note: &str, col: Rgb) {
    c.text(label, x, y, w * 0.6, 16.0 * sc, 10.5 * sc, 400, "Segoe UI", Rgb::u8(215, 220, 232), DT_LEFT);
    c.text(value, x, y, w, 16.0 * sc, 12.0 * sc, 600, "Segoe UI", severity(pct, col), DT_RIGHT);
    let by = y + 22.0 * sc;
    if let Some(p) = pct {
        c.capsule_h(x, x + w, by, 2.5 * sc, WHITE, 0.12);
        c.capsule_h(x, x + (w * (p as f32 / 100.0).clamp(0.0, 1.0)).max(5.0 * sc), by, 2.5 * sc, severity(pct, col), 1.0);
    }
    c.text(note, x, y + 27.0 * sc, w, 14.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
}

// ----------------------------------------------------------------------- Multimedia
fn paint_popup_media(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, disp: &mut Vec<f32>, t: f32, style: &str) {
    let art = media::ART.lock().unwrap().clone();
    let accent = art.as_ref().map(|a| Rgb::u8(a.accent.0, a.accent.1, a.accent.2)).unwrap_or(Rgb::u8(0, 230, 255));
    let app = if s.media_app.is_empty() { "—".to_string() } else { s.media_app.clone() };
    let y0 = popup_frame(c, w, sc, if s.playing { tr("Reproduciendo", "Now playing") } else { tr("En pausa", "Paused") }, &app);
    let pad = 12.0 * sc;
    // carátula grande (o vinilo)
    let a_sz = 112.0 * sc;
    let (ax, ay) = (pad, y0);
    c.capsule(ax + a_sz / 2.0, ay + a_sz * 0.2, ay + a_sz * 0.8, a_sz * 0.3, accent, 0.0, a_sz * 0.18, 0.22, 0.0);
    match art.as_ref().filter(|a| !a.px.is_empty()) {
        Some(a) => c.image_rounded(ax, ay, a_sz, 10.0 * sc, &a.px, media::ART_SIZE, media::ART_SIZE, 1.0),
        None => c.vinyl(ax + a_sz / 2.0, ay + a_sz / 2.0, a_sz / 2.0, t * if s.playing { 3.5 } else { 0.0 }, accent, 1.0),
    }
    // textos
    let tx = ax + a_sz + 14.0 * sc;
    let tw = w - tx - pad;
    let tpx = 14.0 * sc;
    let (l1, l2) = wrap2(c, &s.title, tpx, 600, tw);
    c.text(&l1, tx, ay, tw, 20.0 * sc, tpx, 600, "Segoe UI", WHITE, DT_LEFT | DT_END_ELLIPSIS);
    c.text(&l2, tx, ay + 19.0 * sc, tw, 20.0 * sc, tpx, 600, "Segoe UI", WHITE, DT_LEFT | DT_END_ELLIPSIS);
    c.text(&s.artist, tx, ay + 44.0 * sc, tw, 18.0 * sc, 11.0 * sc, 400, "Segoe UI", Rgb::u8(205, 210, 225), DT_LEFT | DT_END_ELLIPSIS);
    c.text(&s.media_album, tx, ay + 62.0 * sc, tw, 16.0 * sc, 10.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    let state_txt = if s.playing { tr("▶  reproduciendo", "▶  playing") } else { tr("❚❚  en pausa", "❚❚  paused") };
    badge(c, tx, ay + a_sz - 10.0 * sc, state_txt, 8.5 * sc, accent, sc, 0.0);
    // progreso con tiempos
    let py = ay + a_sz + 14.0 * sc;
    let tl = *media::TIMELINE.lock().unwrap();
    let (x0, x1) = (pad, w - pad);
    c.capsule_h(x0, x1, py, 2.5 * sc, WHITE, 0.14);
    if tl.dur > 1.0 {
        let pos = (tl.pos + if s.playing { tl.at.map(|a| a.elapsed().as_secs_f64()).unwrap_or(0.0) } else { 0.0 }).min(tl.dur);
        let fx = x0 + (x1 - x0) * (pos / tl.dur) as f32;
        c.capsule_h(x0, fx.max(x0 + 5.0 * sc), py, 2.5 * sc, accent, 1.0);
        c.dot(fx, py, 4.5 * sc, WHITE, 1.0, 3.0 * sc, 0.35);
        c.text(&mmss(pos), x0, py + 5.0 * sc, 80.0 * sc, 14.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        c.text(&mmss(tl.dur), x1 - 80.0 * sc, py + 5.0 * sc, 80.0 * sc, 14.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_RIGHT);
    }
    // espectro ancho
    let target = media::LEVELS.lock().unwrap().clone();
    if disp.len() != target.len() {
        *disp = vec![0.0; target.len()];
    }
    for i in 0..target.len() {
        let k = if target[i] > disp[i] { 0.45 } else { 0.12 };
        disp[i] += (target[i] - disp[i]) * k;
    }
    let pulse = (disp.iter().take(6).sum::<f32>() / 6.0).clamp(0.0, 1.0);
    let vy = py + 22.0 * sc;
    let d = disp.clone();
    let (vh, ph) = ((h - vy - pad).max(8.0), (t / 14.0).rem_euclid(1.0));
    spectrum(c, style, x0, vy, x1 - x0, vh, sc, &d, &d, ph, pulse);
}

// ======================================================================= radio
const AIR_COL: Rgb = Rgb::u8(255, 58, 70);
const ACT_RADIO_TOGGLE: u32 = 10;
const ACT_RADIO_VOL_DOWN: u32 = 11;
const ACT_RADIO_VOL_UP: u32 = 12;
const ACT_RADIO_PAGE_UP: u32 = 13;
const ACT_RADIO_PAGE_DOWN: u32 = 14;
const ACT_RADIO_SEARCH: u32 = 15;
const ACT_RADIO_CLEAR: u32 = 16;
const ACT_RADIO_BACK: u32 = 17;
const ACT_RADIO_CONTINENT: u32 = 20; // +índice de radio::CONTINENTS
const ACT_RADIO_COUNTRY: u32 = 500; // +índice del país en el selector
const ACT_RADIO_STATION: u32 = 1000; // +índice de la emisora en la lista visible
const RADIO_POPUP_W: f32 = 430.0;
const RADIO_ROWS: usize = 10;
const RADIO_ROW_H: f32 = 30.0;
const RADIO_CARD_H: f32 = 108.0;
const RADIO_NAV_H: f32 = 24.0;
const RADIO_SEARCH_H: f32 = 28.0;
/// países por fila en el selector
const RADIO_COLS: usize = 2;
const RADIO_POPUP_H: f32 = 40.0 + RADIO_CARD_H + 8.0 + RADIO_NAV_H + 8.0 + RADIO_SEARCH_H + 8.0 + (RADIO_ROWS as f32 * RADIO_ROW_H + 8.0) + 6.0 + 20.0 + 10.0;

thread_local! {
    /// primera fila visible de la lista (emisoras o países; se acota al pintar)
    static RADIO_SCROLL: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// el cuadro de búsqueda tiene el teclado (la ventana se activó al pulsarlo)
    static RADIO_FOCUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// última tecla o clic en la ventana de radio (el foco solo la mantiene abierta mientras se usa)
    static RADIO_USED: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Segundos que la ventana de radio sigue abierta con el cursor fuera mientras tiene el teclado
const RADIO_FOCUS_HOLD: u64 = 6;

fn radio_scroll(d: i32) {
    RADIO_SCROLL.with(|s| s.set((s.get() as i32 + d).max(0) as usize));
}

fn radio_top() {
    RADIO_SCROLL.with(|s| s.set(0));
}

/// Elige el país `i` del selector y pasa a su lista de emisoras
fn radio_pick_country(i: usize) {
    if let Some((code, _)) = radio::visible_countries().get(i) {
        radio::set_country(code);
        radio_top();
    }
}

/// Tecla en el cuadro de búsqueda: escribe, borra (Retroceso), limpia (Esc) o elige el primer
/// resultado (Intro): el primer país en el selector o la primera emisora de la lista
fn radio_char(ch: u32) {
    let (mut q, picking) = {
        let r = radio::radio();
        (r.query.clone(), r.country.is_empty())
    };
    match ch {
        8 => {
            q.pop();
        }
        27 => q.clear(),
        13 if picking => return radio_pick_country(0),
        13 => return radio::play_index(0),
        c if c >= 32 => match char::from_u32(c) {
            Some(c) if q.chars().count() < 40 => q.push(c),
            _ => return,
        },
        _ => return,
    }
    radio::set_query(&q);
    radio_top();
}

/// Clic en una zona de la ventana de radio
fn radio_click(hit: u32) {
    match hit {
        ACT_RADIO_TOGGLE => radio::toggle(),
        ACT_RADIO_VOL_DOWN => radio::volume_step(-0.1),
        ACT_RADIO_VOL_UP => radio::volume_step(0.1),
        ACT_RADIO_PAGE_UP => radio_scroll(-(RADIO_ROWS as i32)),
        ACT_RADIO_PAGE_DOWN => radio_scroll(RADIO_ROWS as i32),
        ACT_RADIO_CLEAR => {
            radio::set_query("");
            radio_top();
        }
        ACT_RADIO_BACK => {
            radio::set_country("");
            radio_top();
        }
        h if h >= ACT_RADIO_STATION => radio::play_index((h - ACT_RADIO_STATION) as usize),
        h if h >= ACT_RADIO_COUNTRY => radio_pick_country((h - ACT_RADIO_COUNTRY) as usize),
        h if h >= ACT_RADIO_CONTINENT && ((h - ACT_RADIO_CONTINENT) as usize) < radio::CONTINENTS.len() => {
            radio::set_continent((h - ACT_RADIO_CONTINENT) as usize);
            radio_top();
        }
        _ => {}
    }
}

/// Nota musical roja (siempre en su color, también en modo claro); al aire lleva un halo que late
fn draw_note(c: &mut Surface, cx: f32, cy: f32, size: f32, on_air: bool, t: f32) {
    c.raw(|c| {
        if on_air {
            let beat = 0.5 + 0.5 * (t * 3.0).sin();
            c.dot(cx, cy, size * 0.1, AIR_COL, 0.0, size * 0.85, 0.2 + 0.2 * beat);
        }
        let col = if on_air { AIR_COL } else { Rgb::u8(214, 52, 64) };
        c.text("\u{EC4F}", cx - size, cy - size, size * 2.0, size * 2.0, size, 400, "Segoe Fluent Icons", col, DT_CENTER);
    });
}

/// Letrero de estado: «ON AIR» en rojo sólido (parpadea cada segundo) mientras suena; si no, una
/// píldora tenue con el estado. Devuelve el ancho.
fn on_air_sign(c: &mut Surface, x: f32, yc: f32, px: f32, sc: f32, n: &radio::Now, t: f32, min_w: f32) -> f32 {
    match n.status {
        radio::Status::Playing => {
            let label = "ON AIR";
            let tw = c.text_width(label, px, 700, "Segoe UI");
            let (bw, bh) = ((tw + 10.0 * sc).max(min_w), (px + 5.0 * sc).round());
            let lit = if (t as u32) % 2 == 0 { 1.0 } else { 0.78 };
            c.raw(|c| {
                c.round_rect(x - 1.5 * sc, yc - bh / 2.0 - 1.5 * sc, bw + 3.0 * sc, bh + 3.0 * sc, 4.0 * sc, AIR_COL, 0.28 * lit);
                c.round_rect(x, yc - bh / 2.0, bw, bh, 3.0 * sc, AIR_COL, lit);
                c.text(label, x, yc - bh / 2.0, bw, bh, px, 700, "Segoe UI", WHITE, DT_CENTER);
            });
            bw
        }
        // con ancho fijo (panel de la barra) el letrero largo no cabe: puntos suspensivos
        radio::Status::Connecting if min_w > 0.0 => badge(c, x, yc, "• • •", px, RAM_COL, sc, min_w),
        radio::Status::Connecting => badge(c, x, yc, tr("CONECTANDO…", "CONNECTING…"), px, RAM_COL, sc, 0.0),
        radio::Status::Error => badge(c, x, yc, "ERROR", px, HEALTH_BAD, sc, min_w),
        radio::Status::Stopped => badge(c, x, yc, "RADIO", px, DIM, sc, min_w),
    }
}

/// El nombre de la emisora no cabe en algún panel de radio y se desplaza: hay que animarlo
static RADIO_BANNER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Panel en la barra: nota + letrero «ON AIR» (el panel termina donde termina el letrero) y, debajo,
/// el nombre de la emisora como banner que se desplaza si no cabe
fn paint_radio(c: &mut Surface, w: f32, h: f32, sc: f32, n: &radio::Now, t: f32) {
    let isz = (h * 0.44).min(19.0 * sc);
    draw_note(c, isz + 2.0 * sc, h / 2.0, isz, n.on_air(), t);
    let tx = isz * 2.0 + 6.0 * sc;
    let tw = (w - tx - 3.0 * sc).max(0.0);
    let px = (11.5 * sc).min((h * 0.32).floor());
    on_air_sign(c, tx, h * 0.29, (px * 0.8).floor(), sc, n, t, tw);
    let name: &str = match &n.station {
        Some(s) => &s.name,
        None => tr("Elige una emisora", "Pick a station"),
    };
    let col = if n.on_air() { WHITE } else { Rgb::u8(190, 198, 216) };
    let (ny, nh) = (h * 0.5, h * 0.44);
    let nw = c.text_width(name, px, 600, "Segoe UI");
    if nw <= tw {
        c.text(name, tx, ny, tw, nh, px, 600, "Segoe UI", col, DT_LEFT);
    } else {
        RADIO_BANNER.store(true, std::sync::atomic::Ordering::Relaxed);
        let (speed, gap, wait) = (24.0 * sc, 26.0 * sc, 1.5);
        let ph = t % ((nw + gap) / speed + wait);
        let off = if ph < wait { 0.0 } else { (ph - wait) * speed };
        let (c0, c1, fade) = (tx, tx + tw, 5.0 * sc);
        c.text_clip(name, tx - off, ny, nh, px, 600, "Segoe UI", col, c0, c1, fade);
        c.text_clip(name, tx - off + nw + gap, ny, nh, px, 600, "Segoe UI", col, c0, c1, fade);
    }
}

/// Modo icono: nota musical roja; un punto indica que está al aire (rojo) o conectando (ámbar)
fn paint_radio_compact(c: &mut Surface, w: f32, h: f32, sc: f32, n: &radio::Now, t: f32) {
    let size = (w.min(h) * 0.42).min(19.0 * sc);
    draw_note(c, w / 2.0, h / 2.0, size, n.on_air(), t);
    let (cx, cy) = (w - 5.0 * sc, 5.0 * sc);
    match n.status {
        radio::Status::Playing if (t as u32) % 2 == 0 => c.raw(|c| c.dot(cx, cy, 2.6 * sc, AIR_COL, 1.0, 4.0 * sc, 0.7)),
        radio::Status::Playing => c.raw(|c| c.dot(cx, cy, 2.6 * sc, AIR_COL, 0.75, 0.0, 0.0)),
        radio::Status::Connecting => c.dot(cx, cy, 2.4 * sc, RAM_COL, 1.0, 3.0 * sc, 0.5),
        _ => {}
    }
}

/// Ventana de radio: emisora actual (estado, datos, reproducir/detener y volumen) y, debajo, primero
/// el selector de país (América / Europa) y después la lista de emisoras del país elegido (clic
/// para reproducir). El buscador filtra países o emisoras; rueda o flechas para desplazar.
fn paint_popup_radio(c: &mut Surface, w: f32, _h: f32, sc: f32, t: f32) {
    let (n, continent, country, loading, failed, query, searching) = {
        let r = radio::radio();
        (r.now.clone(), r.continent.min(radio::CONTINENTS.len() - 1), r.country.clone(), r.loading.contains(&r.country), r.failed.contains(&r.country), r.query.clone(), r.searching)
    };
    let picking = country.is_empty();
    let list = if picking { Vec::new() } else { radio::visible() };
    let countries = if picking { radio::visible_countries() } else { Vec::new() };
    let right = match n.since {
        Some(s) if n.on_air() => format!("{}  {}", tr("al aire", "on air"), mmss(s.elapsed().as_secs_f64())),
        _ if picking => format!("{} {}", countries.len(), tr("países", "countries")),
        _ => format!("{} {}", list.len(), tr("emisoras", "stations")),
    };
    let y0 = popup_frame(c, w, sc, "Radio", &right);
    let pad = 12.0 * sc;
    let cw = w - pad * 2.0;
    let hover = POPUP_HOVER.with(|h| h.get());
    let mut hits: Vec<(f32, f32, f32, f32, u32)> = Vec::new();
    let soft = Rgb::u8(205, 212, 228);
    let icon = "Segoe Fluent Icons";

    // ---- emisora actual
    let ch = RADIO_CARD_H * sc;
    card(c, pad, y0, cw, ch, sc);
    let ir = 25.0 * sc;
    let (icx, icy) = (pad + 12.0 * sc + ir, y0 + 39.0 * sc);
    c.raw(|c| c.dot(icx, icy, ir, AIR_COL, if n.on_air() { 0.2 } else { 0.1 }, 0.0, 0.0));
    draw_note(c, icx, icy, ir * 0.9, n.on_air(), t);
    let bsz = 38.0 * sc;
    let (bcx, bcy) = (pad + cw - 12.0 * sc - bsz / 2.0, icy);
    let tx = icx + ir + 12.0 * sc;
    let tw = bcx - bsz / 2.0 - 8.0 * sc - tx;
    on_air_sign(c, tx, y0 + 17.0 * sc, 9.0 * sc, sc, &n, t, 0.0);
    match &n.station {
        Some(s) => {
            c.text(&s.name, tx, y0 + 27.0 * sc, tw, 22.0 * sc, 15.0 * sc, 600, "Segoe UI", WHITE, DT_LEFT | DT_END_ELLIPSIS);
            let place = [s.place.as_str(), s.country.as_str()].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(", ");
            let fmt = if s.bitrate > 0 { format!("{} {} kbps", s.codec, s.bitrate) } else { s.codec.clone() };
            let info = [place, fmt].into_iter().filter(|x| !x.trim().is_empty()).collect::<Vec<_>>().join("  ·  ");
            c.text(&info, tx, y0 + 49.0 * sc, tw, 16.0 * sc, 10.5 * sc, 400, "Segoe UI", soft, DT_LEFT | DT_END_ELLIPSIS);
            let (line, col) = if n.status == radio::Status::Error { (n.error.clone(), HEALTH_BAD) } else { (s.tags.clone(), DIM) };
            c.text(&line, tx, y0 + 65.0 * sc, tw, 15.0 * sc, 9.5 * sc, 400, "Segoe UI", col, DT_LEFT | DT_END_ELLIPSIS);
            // botón reproducir / detener (sólido, con sus colores también en modo claro)
            let active = matches!(n.status, radio::Status::Playing | radio::Status::Connecting);
            let fill = if active { AIR_COL } else if render::is_light() { Rgb::u8(0, 95, 184) } else { Rgb::u8(60, 160, 255) };
            c.raw(|c| {
                c.dot(bcx, bcy, bsz / 2.0, fill, if hover == ACT_RADIO_TOGGLE { 1.0 } else { 0.8 }, 0.0, 0.0);
                c.text(if active { "\u{E71A}" } else { "\u{E768}" }, bcx - bsz / 2.0, bcy - bsz / 2.0, bsz, bsz, 15.0 * sc, 400, icon, WHITE, DT_CENTER);
            });
            hits.push((bcx - bsz / 2.0, bcy - bsz / 2.0, bcx + bsz / 2.0, bcy + bsz / 2.0, ACT_RADIO_TOGGLE));
        }
        None => {
            c.text(tr("Elige una emisora", "Pick a station"), tx, y0 + 27.0 * sc, tw, 22.0 * sc, 15.0 * sc, 600, "Segoe UI", WHITE, DT_LEFT);
            c.text(tr("Primero el país y luego la emisora", "First the country, then the station"), tx, y0 + 49.0 * sc, tw, 16.0 * sc, 10.5 * sc, 400, "Segoe UI", soft, DT_LEFT);
        }
    }
    // volumen:  🔈  −  ▬▬▬▬▬  +  80 %
    let vy = y0 + ch - 15.0 * sc;
    let br = 9.0 * sc;
    c.text("\u{E767}", pad + 12.0 * sc, vy - 8.0 * sc, 16.0 * sc, 16.0 * sc, 11.5 * sc, 400, icon, DIM, DT_CENTER);
    let minus_x = pad + 12.0 * sc + 22.0 * sc + br;
    let plus_x = pad + cw - 12.0 * sc - 38.0 * sc - br;
    for (x, label, act) in [(minus_x, "−", ACT_RADIO_VOL_DOWN), (plus_x, "+", ACT_RADIO_VOL_UP)] {
        c.round_rect(x - br, vy - br, br * 2.0, br * 2.0, br, WHITE, if hover == act { 0.24 } else { 0.1 });
        c.text(label, x - br, vy - br - 1.0 * sc, br * 2.0, br * 2.0, 11.5 * sc, 600, "Segoe UI", WHITE, DT_CENTER);
        hits.push((x - br - 2.0 * sc, vy - br - 2.0 * sc, x + br + 2.0 * sc, vy + br + 2.0 * sc, act));
    }
    let (v0, v1) = (minus_x + br + 8.0 * sc, plus_x - br - 8.0 * sc);
    c.capsule_h(v0, v1, vy, 2.0 * sc, WHITE, 0.14);
    c.capsule_h(v0, v0 + ((v1 - v0) * n.volume).max(4.0 * sc), vy, 2.0 * sc, AIR_COL.mix(WHITE, 0.25), 1.0);
    c.text(&format!("{:.0} %", n.volume * 100.0), pad + cw - 12.0 * sc - 38.0 * sc, vy - 8.0 * sc, 38.0 * sc, 16.0 * sc, 10.0 * sc, 600, "Segoe UI", soft, DT_RIGHT);

    // ---- navegación: continentes (selector de país) o «‹ Países» + país elegido (emisoras)
    let (ty, th) = (y0 + ch + 8.0 * sc, RADIO_NAV_H * sc);
    if picking {
        let gap = 6.0 * sc;
        let nc = radio::CONTINENTS.len() as f32;
        let tabw = (cw - gap * (nc - 1.0)) / nc;
        for (i, k) in radio::CONTINENTS.iter().enumerate() {
            let x = pad + i as f32 * (tabw + gap);
            let act = ACT_RADIO_CONTINENT + i as u32;
            let label = tr(k.0, k.1);
            // con búsqueda se muestran países de todos los continentes: ninguno queda marcado
            if i == continent && query.trim().is_empty() {
                c.raw(|c| {
                    c.round_rect(x, ty, tabw, th, th / 2.0, AIR_COL, 0.9);
                    c.text(label, x, ty, tabw, th, 10.5 * sc, 600, "Segoe UI", WHITE, DT_CENTER);
                });
            } else {
                c.round_rect(x, ty, tabw, th, th / 2.0, WHITE, if hover == act { 0.2 } else { 0.08 });
                c.text(label, x, ty, tabw, th, 10.5 * sc, 600, "Segoe UI", soft, DT_CENTER);
            }
            hits.push((x, ty, x + tabw, ty + th, act));
        }
    } else {
        let back = format!("‹  {}", tr("Países", "Countries"));
        let bw = c.text_width(&back, 10.5 * sc, 600, "Segoe UI") + 22.0 * sc;
        c.round_rect(pad, ty, bw, th, th / 2.0, WHITE, if hover == ACT_RADIO_BACK { 0.24 } else { 0.1 });
        c.text(&back, pad, ty, bw, th, 10.5 * sc, 600, "Segoe UI", soft, DT_CENTER);
        hits.push((pad, ty, pad + bw, ty + th, ACT_RADIO_BACK));
        let code_w = badge(c, pad + bw + 10.0 * sc, ty + th / 2.0, &country, 8.5 * sc, AIR_COL.mix(WHITE, 0.2), sc, 0.0);
        let nx = pad + bw + 10.0 * sc + code_w + 7.0 * sc;
        c.text(&radio::country_label(&country), nx, ty, pad + cw - nx, th, 12.5 * sc, 600, "Segoe UI", WHITE, DT_LEFT | DT_END_ELLIPSIS);
    }

    // ---- búsqueda: filtra países o emisoras al escribir (y consulta el directorio del país)
    let (sy, sh) = (ty + th + 8.0 * sc, RADIO_SEARCH_H * sc);
    let focus = RADIO_FOCUS.with(|f| f.get());
    c.round_rect(pad, sy, cw, sh, sh / 2.0, WHITE, if focus { 0.16 } else if hover == ACT_RADIO_SEARCH { 0.12 } else { 0.07 });
    if focus {
        c.raw(|c| c.round_rect(pad + 10.0 * sc, sy + sh - 2.0 * sc, cw - 20.0 * sc, 1.5 * sc, 1.0, AIR_COL, 0.9));
    }
    c.text("\u{E721}", pad + 6.0 * sc, sy, 22.0 * sc, sh, 10.5 * sc, 400, icon, DIM, DT_CENTER);
    let (qx, qw) = (pad + 30.0 * sc, cw - 60.0 * sc);
    if query.is_empty() {
        let hint = match (picking, focus) {
            (true, _) => tr("Buscar país…", "Search country…"),
            (false, true) => tr("Escribe el nombre, la ciudad o el género…", "Type a name, city or genre…"),
            (false, false) => tr("Buscar emisora…", "Search station…"),
        };
        c.text(hint, qx, sy, qw, sh, 11.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    } else {
        c.text(&query, qx, sy, qw, sh, 11.0 * sc, 600, "Segoe UI", WHITE, DT_LEFT);
        let (xx, xr) = (pad + cw - 15.0 * sc, 8.5 * sc);
        c.round_rect(xx - xr, sy + sh / 2.0 - xr, xr * 2.0, xr * 2.0, xr, WHITE, if hover == ACT_RADIO_CLEAR { 0.24 } else { 0.1 });
        c.text("\u{E711}", xx - xr, sy + sh / 2.0 - xr, xr * 2.0, xr * 2.0, 7.5 * sc, 400, icon, soft, DT_CENTER);
        hits.push((xx - xr - 3.0 * sc, sy, pad + cw, sy + sh, ACT_RADIO_CLEAR));
    }
    if focus && (t * 2.0) as u32 % 2 == 0 {
        let cx = qx + if query.is_empty() { 0.0 } else { c.text_width(&query, 11.0 * sc, 600, "Segoe UI").min(qw) + 1.5 * sc };
        c.line(cx, sy + 7.0 * sc, cx, sy + sh - 7.0 * sc, 1.2 * sc, WHITE, 0.9);
    }
    hits.push((pad, sy, pad + cw - 28.0 * sc, sy + sh, ACT_RADIO_SEARCH));

    // ---- lista: países (en columnas) o emisoras
    let (ly, rh) = (sy + sh + 8.0 * sc, RADIO_ROW_H * sc);
    let lh = RADIO_ROWS as f32 * rh + 8.0 * sc;
    card(c, pad, ly, cw, lh, sc);
    // filas en total (los países van de `RADIO_COLS` en `RADIO_COLS`) y desplazamiento acotado
    let total = if picking { countries.len().div_ceil(RADIO_COLS) } else { list.len() };
    let max_scroll = total.saturating_sub(RADIO_ROWS);
    let scroll = RADIO_SCROLL.with(|s| {
        s.set(s.get().min(max_scroll));
        s.get()
    });
    if total == 0 {
        let msg = if !query.trim().is_empty() {
            if searching { tr("Buscando en el directorio…", "Searching the directory…") } else { tr("Sin resultados", "No results") }
        } else if failed && !loading {
            tr("Sin conexión con el directorio de emisoras", "Cannot reach the station directory")
        } else if !picking && !loading {
            tr("No hay emisoras de este país en el directorio", "The directory has no stations for this country")
        } else {
            tr("Cargando emisoras…", "Loading stations…")
        };
        c.text(msg, pad, ly, cw, lh, 11.0 * sc, 400, "Segoe UI", DIM, DT_CENTER);
    }
    if picking {
        let colw = (cw - 12.0 * sc) / RADIO_COLS as f32;
        for row in 0..RADIO_ROWS {
            for col in 0..RADIO_COLS {
                let idx = (scroll + row) * RADIO_COLS + col;
                let Some((code, name)) = countries.get(idx) else { break };
                let (x, y) = (pad + 3.0 * sc + col as f32 * colw, ly + 4.0 * sc + row as f32 * rh);
                let act = ACT_RADIO_COUNTRY + idx as u32;
                if hover == act {
                    c.round_rect(x, y + 1.0, colw - 3.0 * sc, rh - 2.0, 6.0 * sc, WHITE, 0.12);
                }
                badge(c, x + 7.0 * sc, y + rh / 2.0, code, 8.5 * sc, DIM, sc, 24.0 * sc);
                c.text(name, x + 38.0 * sc, y, colw - 44.0 * sc, rh, 11.5 * sc, 600, "Segoe UI", Rgb::u8(228, 232, 242), DT_LEFT | DT_END_ELLIPSIS);
                hits.push((x, y, x + colw - 3.0 * sc, y + rh, act));
            }
        }
    }
    for row in 0..RADIO_ROWS {
        let idx = scroll + row;
        let Some(s) = list.get(idx) else { break };
        let y = ly + 4.0 * sc + row as f32 * rh;
        let act = ACT_RADIO_STATION + idx as u32;
        let current = n.station.as_ref().is_some_and(|c| c.url == s.url);
        if current {
            c.raw(|c| c.round_rect(pad + 3.0 * sc, y + 1.0, cw - 10.0 * sc, rh - 2.0, 6.0 * sc, AIR_COL, 0.2));
        } else if hover == act {
            c.round_rect(pad + 3.0 * sc, y + 1.0, cw - 10.0 * sc, rh - 2.0, 6.0 * sc, WHITE, 0.1);
        }
        // marcador: ecualizador si suena, nota si es la elegida, número en las demás
        let (mx, myc) = (pad + 18.0 * sc, y + rh / 2.0);
        if current && n.on_air() {
            for k in 0..3 {
                let bh = (0.3 + 0.7 * (t * (5.0 + k as f32 * 1.9) + k as f32 * 1.3).sin().abs()) * 6.5 * sc;
                let bx = mx + (k as f32 - 1.0) * 4.0 * sc;
                c.raw(|c| c.line(bx, myc - bh, bx, myc + bh, 2.2 * sc, AIR_COL, 1.0));
            }
        } else if current {
            draw_note(c, mx, myc, 8.5 * sc, false, t);
        } else {
            c.text(&(idx + 1).to_string(), pad + 5.0 * sc, y, 26.0 * sc, rh, 9.0 * sc, 400, "Segoe UI", DIM, DT_CENTER);
        }
        let nx = pad + 35.0 * sc;
        let dw = cw * 0.32;
        let nw = cw - (nx - pad) - dw - 14.0 * sc;
        c.text(&s.name, nx, y, nw, rh, 12.0 * sc, if current { 700 } else { 600 }, "Segoe UI", if current { WHITE } else { Rgb::u8(228, 232, 242) }, DT_LEFT | DT_END_ELLIPSIS);
        let detail = match (s.place.is_empty(), s.bitrate > 0) {
            (false, true) => format!("{} · {}k", s.place, s.bitrate),
            (false, false) => s.place.clone(),
            (true, true) => format!("{} {}k", s.codec, s.bitrate),
            (true, false) => s.codec.clone(),
        };
        c.text(&detail, pad + cw - dw - 10.0 * sc, y, dw, rh, 9.5 * sc, 400, "Segoe UI", DIM, DT_RIGHT | DT_END_ELLIPSIS);
        hits.push((pad, y, pad + cw - 6.0 * sc, y + rh, act));
    }
    // barra de desplazamiento
    if max_scroll > 0 {
        let (sx, s0, s1) = (pad + cw - 4.5 * sc, ly + 8.0 * sc, ly + lh - 8.0 * sc);
        c.line(sx, s0, sx, s1, 2.0 * sc, WHITE, 0.1);
        let frac = RADIO_ROWS as f32 / total as f32;
        let th0 = s0 + (s1 - s0) * (1.0 - frac) * scroll as f32 / max_scroll as f32;
        c.line(sx, th0, sx, th0 + (s1 - s0) * frac, 2.0 * sc, WHITE, 0.45);
    }

    // ---- pie: posición y flechas de página
    let fy = ly + lh + 6.0 * sc;
    if total > 0 {
        let per = if picking { RADIO_COLS } else { 1 };
        let count = if picking { countries.len() } else { list.len() };
        let note = if picking {
            tr("elige un país", "pick a country")
        } else if searching {
            tr("buscando más en el directorio…", "searching the directory…")
        } else {
            tr("rueda del mouse para desplazar", "mouse wheel to scroll")
        };
        let pos = format!("{}–{} / {}   ·   {note}", scroll * per + 1, ((scroll + RADIO_ROWS) * per).min(count), count);
        c.text(&pos, pad + 2.0 * sc, fy, cw - 56.0 * sc, 20.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    }
    for (i, (glyph, act, on)) in [("\u{E70E}", ACT_RADIO_PAGE_UP, scroll > 0), ("\u{E70D}", ACT_RADIO_PAGE_DOWN, scroll < max_scroll)].iter().enumerate() {
        let x = pad + cw - 48.0 * sc + i as f32 * 26.0 * sc;
        c.round_rect(x, fy, 22.0 * sc, 20.0 * sc, 6.0 * sc, WHITE, if *on && hover == *act { 0.24 } else { 0.08 });
        c.text(glyph, x, fy, 22.0 * sc, 20.0 * sc, 9.0 * sc, 400, icon, if *on { soft } else { GRAY }, DT_CENTER);
        if *on {
            hits.push((x, fy, x + 22.0 * sc, fy + 20.0 * sc, *act));
        }
    }
    POPUP_HITS.with(|h| {
        let mut h = h.borrow_mut();
        h.retain(|r| r.4 < ACT_RADIO_TOGGLE);
        h.extend(hits);
    });
}

// ----------------------------------------------------------------------- ventiladores y energía (ventana de sistema)
const FAN_CARD_H: f32 = 56.0;
const POWER_CARD_H: f32 = 62.0;

const ZONE_FAN: u8 = 0;
const ZONE_POWER: u8 = 1;
const ZONE_ROUTER: u8 = 2;

/// Zona animada de una ventana flotante: qué es, dónde va y el fondo que tenía debajo
struct AnimZone {
    kind: u8,
    x: f32,
    y: f32,
    cw: f32,
    rect: (i32, i32, i32, i32),
    under: Vec<u32>,
}

thread_local! {
    /// Zonas animadas (ventiladores, energía, router) del último pintado completo, con el tamaño
    /// del lienzo: entre dos pintados completos solo se repintan estas zonas, no la ventana entera
    static SYS_ANIM: RefCell<((i32, i32), Vec<AnimZone>)> = const { RefCell::new(((0, 0), Vec::new())) };
}

/// Guarda el fondo de una tarjeta animada antes de dibujarla
fn anim_zone(c: &mut Surface, kind: u8, x: f32, y: f32, cw: f32, ch: f32) {
    let (rx, ry, rw, rh, under) = c.copy_rect(x.floor() as i32, y.floor() as i32, cw.ceil() as i32 + 2, ch.ceil() as i32 + 2);
    let size = (c.w, c.h);
    SYS_ANIM.with(|a| {
        let mut a = a.borrow_mut();
        if a.0 != size {
            *a = (size, Vec::new());
        }
        a.1.push(AnimZone { kind, x, y, cw, rect: (rx, ry, rw, rh), under });
    });
}

/// Rectángulos (px de la ventana) de las zonas animadas, para invalidar solo esas
fn sys_anim_rects() -> Vec<RECT> {
    SYS_ANIM.with(|a| a.borrow().1.iter().map(|z| RECT { left: z.rect.0, top: z.rect.1, right: z.rect.0 + z.rect.2, bottom: z.rect.1 + z.rect.3 }).collect())
}

/// Repinta solo las zonas animadas sobre su fondo guardado; false si no hay nada guardado para
/// este lienzo (hace falta un pintado completo)
fn paint_anim(c: &mut Surface, sc: f32, s: &state::State, t: f32) -> bool {
    let zones = SYS_ANIM.with(|a| {
        let mut a = a.borrow_mut();
        if a.0 == (c.w, c.h) { std::mem::take(&mut a.1) } else { Vec::new() }
    });
    if zones.is_empty() {
        return false;
    }
    for z in &zones {
        c.put_rect(z.rect.0, z.rect.1, z.rect.2, z.rect.3, &z.under);
        match z.kind {
            ZONE_FAN => {
                paint_fan_card(c, z.x, z.y, z.cw, sc, s, t);
            }
            ZONE_POWER => {
                paint_power_card(c, z.x, z.y, z.cw, sc, s, t);
            }
            _ => draw_router_live(c, z.x, z.y, sc, t),
        }
    }
    SYS_ANIM.with(|a| a.borrow_mut().1 = zones);
    true
}

thread_local! {
    /// último instante dibujado y ángulo acumulado de cada rotor (CPU, GPU): al integrar, un cambio
    /// de rpm cambia la velocidad sin saltos de posición
    static FAN_SPIN: std::cell::Cell<(f32, [f32; 2])> = const { std::cell::Cell::new((0.0, [0.0; 2])) };
}

fn has_fans(s: &state::State) -> bool {
    s.fan_c >= 0 || s.fan_g >= 0
}

/// Rotor de cinco aspas curvas dentro de un aro
fn draw_fan(c: &mut Surface, cx: f32, cy: f32, r: f32, ang: f32, col: Rgb, sc: f32) {
    use std::f32::consts::{PI, TAU};
    c.ellipse_ring(cx, cy, r, r, 0.0, (1.3 * sc).max(1.0), col, 0.35);
    for k in 0..5 {
        let a0 = ang + k as f32 * TAU / 5.0;
        for j in 0..9 {
            let f = j as f32 / 8.0;
            let (rad, a) = (r * (0.24 + 0.56 * f), a0 + f * 0.95);
            c.dot(cx + rad * a.cos(), cy + rad * a.sin(), r * (0.07 + 0.11 * (PI * f).sin()), col, 0.9, 0.0, 0.0);
        }
    }
    c.dot(cx, cy, r * 0.20, col.mix(WHITE, 0.45), 1.0, 0.0, 0.0);
}

/// Ventiladores de CPU y GPU: rotor que gira en proporción a las rpm reales y su valor
fn paint_fan_card(c: &mut Surface, x: f32, y: f32, cw: f32, sc: f32, s: &state::State, t: f32) -> f32 {
    let ch = FAN_CARD_H * sc;
    card(c, x, y, cw, ch, sc);
    // 3000 rpm reales no se pueden mostrar (parecería parado): se escala a 1,5 vueltas por segundo
    let (last, mut ang) = FAN_SPIN.with(|f| f.get());
    let dt = (t - last).clamp(0.0, 0.2);
    let fans = [("CPU", s.fan_c, CPU_COL), ("GPU", s.fan_g, GPU_COL)];
    for (i, (_, rpm, _)) in fans.iter().enumerate() {
        ang[i] = (ang[i] + dt * std::f32::consts::TAU * (*rpm as f32 / 2000.0).clamp(0.0, 2.5)).rem_euclid(std::f32::consts::TAU);
    }
    FAN_SPIN.with(|f| f.set((t, ang)));
    let hw = cw / 2.0;
    let r = 17.0 * sc;
    for (i, (name, rpm, col)) in fans.iter().enumerate() {
        let x0 = x + i as f32 * hw + 12.0 * sc;
        let col = if *rpm >= 0 { *col } else { GRAY };
        draw_fan(c, x0 + r, y + ch / 2.0, r, ang[i], col, sc);
        let tx = x0 + r * 2.0 + 10.0 * sc;
        c.text(&format!("{} {name}", tr("VENTILADOR", "FAN")), tx, y + 9.0 * sc, hw - (tx - x0) - 8.0 * sc, 14.0 * sc, 8.5 * sc, 700, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
        let (num, unit) = match *rpm {
            v if v < 0 => ("—".to_string(), ""),
            0 => (tr("parado", "stopped").to_string(), ""),
            v => (v.to_string(), "rpm"),
        };
        value_unit(c, tx, y + 24.0 * sc, 22.0 * sc, &num, unit, 15.0 * sc, col.mix(WHITE, 0.3), sc);
    }
    ch
}

fn hm(secs: i32) -> String {
    format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60)
}

/// Energía: batería (nivel, carga animada, potencia y autonomía) y plan de energía activo con
/// flechas que avanzan más o menos rápido según el plan
fn paint_power_card(c: &mut Surface, x: f32, y: f32, cw: f32, sc: f32, s: &state::State, t: f32) -> f32 {
    let ch = POWER_CARD_H * sc;
    card(c, x, y, cw, ch, sc);
    let split = x + cw * 0.54;
    // ---- batería
    let (bx, by, bw, bh) = (x + 12.0 * sc, y + 12.0 * sc, 40.0 * sc, 19.0 * sc);
    let frac = (s.bat_pct as f32 / 100.0).clamp(0.0, 1.0);
    let col = if !s.bat_present {
        GRAY
    } else if s.bat_charging {
        Rgb::u8(60, 230, 140)
    } else if s.bat_pct <= 15 {
        Rgb::u8(255, 70, 90)
    } else if s.bat_pct <= 30 {
        Rgb::u8(255, 200, 60)
    } else {
        Rgb::u8(110, 215, 150)
    };
    c.round_rect(bx, by, bw, bh, 4.5 * sc, WHITE, 0.16);
    c.round_rect(bx + bw + 1.5 * sc, by + bh * 0.28, 3.0 * sc, bh * 0.44, 1.5 * sc, WHITE, 0.30);
    let (ix, iw, ih) = (bx + 2.5 * sc, bw - 5.0 * sc, bh - 5.0 * sc);
    if s.bat_present {
        let fw = (iw * frac).max(4.0 * sc);
        c.round_rect(ix, by + 2.5 * sc, fw, ih, 2.5 * sc, col, 1.0);
        if s.bat_charging {
            // brillo que recorre la carga de izquierda a derecha
            let p = (t * 0.7).rem_euclid(1.4) - 0.2;
            let (sx, sw) = (ix + fw * p, fw * 0.22);
            let (a, b) = (sx.max(ix), (sx + sw).min(ix + fw));
            if b > a {
                c.round_rect(a, by + 2.5 * sc, b - a, ih, 2.0 * sc, WHITE, 0.45);
            }
        }
    }
    if s.bat_ac {
        // enchufado: rayo sobre la batería
        c.raw(|c| c.text("\u{E945}", bx, by, bw, bh, 11.0 * sc, 400, "Segoe Fluent Icons", WHITE, DT_CENTER));
    }
    let tx = bx + bw + 12.0 * sc;
    let tw = split - tx - 6.0 * sc;
    if s.bat_present {
        value_unit(c, tx, y + 8.0 * sc, 26.0 * sc, &s.bat_pct.to_string(), "%", 17.0 * sc, col.mix(WHITE, 0.3), sc);
    } else {
        c.text(tr("Sin batería", "No battery"), tx, y + 8.0 * sc, tw, 26.0 * sc, 12.0 * sc, 600, "Segoe UI", GAINSBORO, DT_LEFT | DT_END_ELLIPSIS);
    }
    let watts = if s.bat_watts.abs() >= 0.5 { format!("  ·  {:.0} W", s.bat_watts.abs()) } else { String::new() };
    let det = if !s.bat_present {
        tr("corriente alterna", "AC power").to_string()
    } else if s.bat_charging {
        format!("{}{watts}", tr("cargando", "charging"))
    } else if s.bat_ac {
        tr("conectado, sin cargar", "plugged in, not charging").to_string()
    } else if s.bat_secs > 0 {
        format!("{}{watts}", hm(s.bat_secs))
    } else {
        format!("{}{watts}", tr("con batería", "on battery"))
    };
    c.text(&det, x + 12.0 * sc, y + 38.0 * sc, split - x - 16.0 * sc, 15.0 * sc, 9.5 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    // ---- plan de energía
    c.fill(split, y + 10.0 * sc, sc.max(1.0), ch - 20.0 * sc, WHITE, 0.10);
    let (pcol, speed) = match s.plan_kind {
        0 => (Rgb::u8(80, 220, 140), 1.6f32),
        2 => (Rgb::u8(255, 150, 70), 7.0),
        _ => (Rgb::u8(110, 170, 255), 3.6),
    };
    let (ax, cy, cs) = (split + 12.0 * sc, y + ch / 2.0, 6.0 * sc);
    for i in 0..3 {
        let a = 0.22 + 0.78 * (t * speed - i as f32 * 1.0).sin().max(0.0);
        let x0 = ax + i as f32 * 8.5 * sc;
        c.line(x0, cy - cs, x0 + cs, cy, 2.2 * sc, pcol, a);
        c.line(x0 + cs, cy, x0, cy + cs, 2.2 * sc, pcol, a);
    }
    let px = ax + 3.0 * 8.5 * sc + 8.0 * sc;
    let pw = x + cw - 10.0 * sc - px;
    c.text(tr("PLAN DE ENERGÍA", "POWER PLAN"), px, y + 11.0 * sc, pw, 14.0 * sc, 8.5 * sc, 700, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    let mode = match s.power_mode {
        -1 => tr("Mejor eficiencia", "Best efficiency"),
        1 => tr("Mejor rendimiento", "Best performance"),
        _ => "",
    };
    let name = if s.plan_name.is_empty() { "—" } else { s.plan_name.as_str() };
    c.text(name, px, y + 25.0 * sc, pw, 16.0 * sc, 10.5 * sc, 600, "Segoe UI", pcol.mix(WHITE, 0.35), DT_LEFT | DT_END_ELLIPSIS);
    if !mode.is_empty() {
        c.text(mode, px, y + 40.0 * sc, pw, 14.0 * sc, 8.5 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    }
    ch
}

// ----------------------------------------------------------------------- estado del sistema (avisos)
#[derive(Clone, Copy, PartialEq, Debug)]
enum SysAlert {
    CpuTemp(i32),
    GpuTemp(i32),
    /// % de memoria en uso
    Ram(i32),
    /// unidad del sistema y GB libres
    Disk(char, i32),
    Battery(i32),
}

/// Temperatura alta sostenida: al menos 25 de los últimos 30 s por encima del umbral (un pico al
/// abrir un programa no cuenta)
fn sustained(hist: &[f32], thr: f32) -> bool {
    hist.len() >= 30 && hist[hist.len() - 30..].iter().filter(|v| **v >= thr).count() >= 25
}

/// Cosas que merecen atención, cada una con su gravedad (Warn = ámbar, Bad = rojo)
fn sys_alerts(s: &state::State) -> Vec<(net::Level, SysAlert)> {
    use net::Level::{Bad, Warn};
    let mut v = Vec::new();
    // (aviso, grave): la CPU de un portátil trabaja a más temperatura que la GPU
    for (hist, now, warn, bad, cpu) in [(&s.hist_cpu_t, s.cpu_t, 95.0, 100.0, true), (&s.hist_gpu_t, s.gpu_t, 86.0, 91.0, false)] {
        let level = if sustained(hist, bad) { Some(Bad) } else if sustained(hist, warn) { Some(Warn) } else { None };
        if let Some(l) = level {
            v.push((l, if cpu { SysAlert::CpuTemp(now) } else { SysAlert::GpuTemp(now) }));
        }
    }
    if s.ram_total > 0.0 {
        let pct = (s.ram_used / s.ram_total * 100.0).round() as i32;
        if pct >= 90 {
            v.push((if pct >= 96 { Bad } else { Warn }, SysAlert::Ram(pct)));
        }
    }
    if let Some(d) = system_drive(s).filter(|d| d.total > 0.0) {
        let free = d.free / d.total;
        if free <= 0.10 {
            v.push((if free <= 0.05 { Bad } else { Warn }, SysAlert::Disk(d.letter, (d.free / 1e9).round() as i32)));
        }
    }
    if s.bat_present && !s.bat_ac && s.bat_pct <= 15 {
        v.push((if s.bat_pct <= 7 { Bad } else { Warn }, SysAlert::Battery(s.bat_pct)));
    }
    v
}

/// Estado global: el aviso más grave; Unknown hasta que llegan las primeras lecturas
fn sys_level(s: &state::State) -> net::Level {
    let a = sys_alerts(s);
    if a.iter().any(|(l, _)| *l == net::Level::Bad) {
        net::Level::Bad
    } else if !a.is_empty() {
        net::Level::Warn
    } else if s.cpu < 0.0 {
        net::Level::Unknown
    } else {
        net::Level::Ok
    }
}

fn alert_text(a: SysAlert) -> String {
    match a {
        SysAlert::CpuTemp(t) if i18n::es() => format!("Temperatura de la CPU alta de forma sostenida ({t} °C)"),
        SysAlert::CpuTemp(t) => format!("CPU temperature has been high for a while ({t} °C)"),
        SysAlert::GpuTemp(t) if i18n::es() => format!("Temperatura de la GPU alta de forma sostenida ({t} °C)"),
        SysAlert::GpuTemp(t) => format!("GPU temperature has been high for a while ({t} °C)"),
        SysAlert::Ram(p) if i18n::es() => format!("Memoria casi llena ({p} % en uso)"),
        SysAlert::Ram(p) => format!("Memory almost full ({p}% in use)"),
        SysAlert::Disk(l, gb) if i18n::es() => format!("Disco {l}: casi lleno ({gb} GB libres)"),
        SysAlert::Disk(l, gb) => format!("Drive {l}: almost full ({gb} GB free)"),
        SysAlert::Battery(p) if i18n::es() => format!("Batería baja ({p} %)"),
        SysAlert::Battery(p) => format!("Battery low ({p}%)"),
    }
}

/// Punto de estado en la esquina superior derecha de un panel: discreto en verde, con halo en
/// ámbar o rojo
fn level_dot(c: &mut Surface, w: f32, sc: f32, level: net::Level) {
    let col = level_color(level);
    let (cx, cy) = (w - 4.5 * sc, 4.5 * sc);
    match level {
        net::Level::Unknown => {}
        net::Level::Ok => c.dot(cx, cy, 1.8 * sc, col, 0.9, 0.0, 0.0),
        net::Level::Warn => c.dot(cx, cy, 2.4 * sc, col, 1.0, 3.0 * sc, 0.5),
        net::Level::Bad => c.dot(cx, cy, 2.6 * sc, col, 1.0, 4.0 * sc, 0.7),
    }
}

/// Alto (px lógicos) de la franja de avisos de la ventana de sistema (0 si no hay ninguno)
fn alerts_h(n: usize) -> f32 {
    if n == 0 { 0.0 } else { 14.0 + 18.0 * n as f32 }
}

fn paint_alerts(c: &mut Surface, x: f32, y: f32, cw: f32, sc: f32, alerts: &[(net::Level, SysAlert)]) -> f32 {
    let ch = alerts_h(alerts.len()) * sc;
    card(c, x, y, cw, ch, sc);
    for (i, (level, a)) in alerts.iter().enumerate() {
        let ry = y + 7.0 * sc + i as f32 * 18.0 * sc;
        let col = level_color(*level);
        c.dot(x + 15.0 * sc, ry + 9.0 * sc, 3.0 * sc, col, 1.0, 3.0 * sc, 0.5);
        c.text(&alert_text(*a), x + 26.0 * sc, ry, cw - 36.0 * sc, 18.0 * sc, 10.5 * sc, 600, "Segoe UI", col.mix(WHITE, 0.35), DT_LEFT | DT_END_ELLIPSIS);
    }
    ch
}

// ----------------------------------------------------------------------- aplicaciones que más consumen
const APPS_ROWS: usize = 5;
const APPS_CARD_H: f32 = 34.0 + 19.0 * APPS_ROWS as f32 + 6.0;

/// Las aplicaciones que más CPU y memoria usan (procesos del mismo programa sumados)
fn paint_apps_card(c: &mut Surface, x: f32, y: f32, cw: f32, sc: f32) -> f32 {
    let ch = APPS_CARD_H * sc;
    card(c, x, y, cw, ch, sc);
    let bw = badge(c, x + 10.0 * sc, y + 17.0 * sc, "APPS", 9.0 * sc, Rgb::u8(170, 200, 255), sc, 0.0);
    c.text(tr("lo que más consume ahora", "top consumers right now"), x + 10.0 * sc + bw + 8.0 * sc, y + 6.0 * sc, cw * 0.5, 22.0 * sc, 10.0 * sc, 400, "Segoe UI", GAINSBORO, DT_LEFT | DT_END_ELLIPSIS);
    let (mw, pw, barw) = (58.0 * sc, 34.0 * sc, 70.0 * sc);
    let xm = x + cw - 10.0 * sc - mw; // memoria
    let xp = xm - pw; // % de CPU
    let xb = xp - barw - 4.0 * sc; // barra de CPU
    c.text("CPU", xb, y + 6.0 * sc, barw + pw, 22.0 * sc, 8.5 * sc, 700, "Segoe UI", DIM, DT_RIGHT);
    c.text(tr("MEMORIA", "MEMORY"), xm, y + 6.0 * sc, mw, 22.0 * sc, 8.5 * sc, 700, "Segoe UI", DIM, DT_RIGHT);
    let top = sys::TOP.lock().unwrap().clone();
    if top.is_empty() {
        c.text(tr("midiendo…", "measuring…"), x + 10.0 * sc, y + 34.0 * sc, cw - 20.0 * sc, 19.0 * sc, 10.0 * sc, 400, "Segoe UI", DIM, DT_LEFT);
        return ch;
    }
    for (i, a) in top.iter().take(APPS_ROWS).enumerate() {
        let (ry, rh) = (y + (34.0 + 19.0 * i as f32) * sc, 19.0 * sc);
        let name = if a.procs > 1 { format!("{}  ×{}", a.name, a.procs) } else { a.name.clone() };
        c.text(&name, x + 10.0 * sc, ry, xb - x - 18.0 * sc, rh, 10.0 * sc, 400, "Segoe UI", Rgb::u8(215, 220, 232), DT_LEFT | DT_END_ELLIPSIS);
        let col = usage_color(a.cpu, CPU_COL);
        c.capsule_h(xb, xb + barw, ry + rh / 2.0, 2.0 * sc, WHITE, 0.12);
        if a.cpu >= 0.5 {
            c.capsule_h(xb, xb + (barw * (a.cpu / 100.0).clamp(0.0, 1.0)).max(4.0 * sc), ry + rh / 2.0, 2.0 * sc, col, 1.0);
        }
        let pct = if a.cpu < 0.5 { "0%".to_string() } else if a.cpu < 10.0 { format!("{:.1}%", a.cpu) } else { format!("{:.0}%", a.cpu) };
        c.text(&pct, xp, ry, pw, rh, 10.0 * sc, 600, "Segoe UI", col.mix(WHITE, 0.3), DT_RIGHT);
        let mem = if a.mem >= 1e9 { format!("{:.1} GB", a.mem / 1e9) } else { format!("{:.0} MB", a.mem / 1e6) };
        c.text(&mem, xm, ry, mw, rh, 10.0 * sc, 600, "Segoe UI", RAM_COL.mix(WHITE, 0.3), DT_RIGHT);
    }
    ch
}

// ----------------------------------------------------------------------- distribución de la ventana de sistema
/// Ancho de la ventana flotante: la de sistema usa dos columnas de tarjetas
fn popup_w(kind: Kind) -> f32 {
    match kind {
        Kind::System => POPUP_W * 2.0 - 16.0,
        Kind::Radio => RADIO_POPUP_W,
        _ => POPUP_W,
    }
}

const FPS_CARD_MIN: f32 = 84.0;

/// Alto mínimo (con su separación) de la columna izquierda de la ventana de sistema: CPU, GPU/NPU
/// y ventiladores
fn sys_left_h(s: &state::State) -> f32 {
    92.0 * (1 + s.gpus.len().max(1)) as f32 + if has_fans(s) { FAN_CARD_H + 8.0 } else { 0.0 }
}

/// Alto de la ventana de sistema. Columna derecha: memoria, discos, energía, aplicaciones y FPS.
/// Si la izquierda es más larga, la tarjeta de FPS crece; si es más corta, crecen las gráficas de
/// CPU y GPU: las dos columnas terminan siempre a la misma altura.
fn sys_popup_h(s: &state::State) -> f32 {
    let right = 54.0 + disk_card_h(s.drives.len()) + 8.0 + POWER_CARD_H + 8.0 + APPS_CARD_H + 8.0 + FPS_CARD_MIN + 8.0;
    let n = sys_alerts(s).len();
    40.0 + if n > 0 { alerts_h(n) + 8.0 } else { 0.0 } + sys_left_h(s).max(right) + 4.0
}

/// `anim`: entre dos pintados completos (uno por segundo, con los datos nuevos) solo se repintan
/// las tarjetas animadas
fn paint_popup_system(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, t: f32, anim: bool) {
    if anim && paint_anim(c, sc, s, t) {
        return;
    }
    SYS_ANIM.with(|a| *a.borrow_mut() = ((c.w, c.h), Vec::new()));
    let alerts = sys_alerts(s);
    let summary = match alerts.len() {
        0 if s.cpu < 0.0 => String::new(),
        0 => tr("todo en orden", "all good").to_string(),
        1 => tr("1 aviso", "1 alert").to_string(),
        n => format!("{n} {}", tr("avisos", "alerts")),
    };
    let mut y = popup_frame(c, w, sc, tr("Sistema", "System"), &summary);
    let pad = 12.0 * sc;
    // avisos a todo el ancho; debajo, dos columnas de tarjetas
    if !alerts.is_empty() {
        y += paint_alerts(c, pad, y, w - pad * 2.0, sc, &alerts) + 8.0 * sc;
    }
    let cw = (w - pad * 2.0 - 8.0 * sc) / 2.0;
    let mut x = pad;
    let y0 = y;
    let small = 9.5 * sc;
    let fan = |v: i32| if v >= 0 { format!("{v} rpm") } else { "—".into() };
    let temp = |t: i32| if t >= 0 { format!("{t} °C") } else { "— °C".into() };

    // ---- CPU (las tarjetas con gráfica de la columna izquierda se reparten el alto sobrante)
    let spare = (h - y0 - 4.0 * sc - sys_left_h(s) * sc).max(0.0);
    let ch = 84.0 * sc + (spare / (1 + s.gpus.len().max(1)) as f32).floor();
    card(c, x, y, cw, ch, sc);
    card_head(c, x, y, cw, sc, "CPU", &s.cpu_name, &format!("{:.0}", s.cpu.max(0.0)), "%", CPU_COL);
    let ghz = if s.cpu_mhz > 0.0 { format!("≈{:.2} GHz", s.cpu_mhz / 1000.0) } else { "— GHz".into() };
    let det = format!("{ghz}   ·   {}   ·   {} {}", temp(s.cpu_t), tr("ventilador", "fan"), fan(s.fan_c));
    c.text(&det, x + 10.0 * sc, y + 30.0 * sc, cw - 20.0 * sc, 14.0 * sc, small, 400, "Segoe UI", temp_color(s.cpu_t).mix(DIM, 0.55), DT_LEFT | DT_END_ELLIPSIS);
    let (gx, gy, gw, gh) = (x + 10.0 * sc, y + 48.0 * sc, cw - 20.0 * sc, ch - 54.0 * sc);
    c.sparkline(gx, gy, gw, gh, &s.hist_cpu, 100.0, CPU_COL, 1.0);
    c.trend_line(gx, gy, gw, gh, &s.hist_cpu_t, 105.0, TEMP_COL, 0.95);
    chart_legend(c, gx + gw, gy, sc, CPU_COL);
    y += ch + 8.0 * sc;

    // ---- GPU (todas) y NPU
    let n_gpu = s.gpus.iter().filter(|g| !g.npu).count();
    if s.gpus.is_empty() {
        card(c, x, y, cw, ch, sc);
        card_head(c, x, y, cw, sc, "GPU", &s.gpu_name, "—", "%", GPU_COL);
        y += ch + 8.0 * sc;
    }
    let mut gi = 0;
    for (i, g) in s.gpus.iter().enumerate() {
        let col = vendor_color(g);
        let label = if g.npu { "NPU".to_string() } else { gi += 1; if n_gpu > 1 { format!("GPU {gi}") } else { "GPU".into() } };
        card(c, x, y, cw, ch, sc);
        let val = if g.usage >= 0 { g.usage.to_string() } else { "—".into() };
        card_head(c, x, y, cw, sc, &label, &g.name, &val, "%", col);
        let mut parts: Vec<String> = Vec::new();
        if !g.npu || g.temp >= 0 {
            parts.push(temp(g.temp));
        }
        if g.mem_total > 0.0 && !g.npu {
            let kind = if g.integrated { tr("compartida", "shared") } else { "VRAM" };
            parts.push(format!("{kind} {:.1}/{:.0} GB", g.mem_used, g.mem_total));
        } else if g.mem_used > 0.01 {
            parts.push(format!("{} {:.1} GB", tr("memoria", "memory"), g.mem_used));
        }
        if g.power > 0.0 {
            parts.push(format!("{:.0} W", g.power));
        }
        if g.mhz > 0 {
            parts.push(format!("{:.2} GHz", g.mhz as f32 / 1000.0));
        }
        if g.fan >= 0 {
            parts.push(fan(g.fan));
        }
        let det = parts.join("   ·   ");
        c.text(&det, x + 10.0 * sc, y + 30.0 * sc, cw - 20.0 * sc, 14.0 * sc, small, 400, "Segoe UI", temp_color(g.temp).mix(DIM, 0.55), DT_LEFT | DT_END_ELLIPSIS);
        let (gx, gy) = (x + 10.0 * sc, y + 48.0 * sc);
        c.sparkline(gx, gy, gw, gh, &g.hist, 100.0, col, 1.0);
        if g.temp >= 0 || g.hist_t.iter().any(|v| *v > 0.0) {
            c.trend_line(gx, gy, gw, gh, &g.hist_t, 105.0, TEMP_COL, 0.95);
            chart_legend(c, gx + gw, gy, sc, col);
        }
        let _ = i;
        y += ch + 8.0 * sc;
    }

    // ---- Ventiladores (si el equipo los expone)
    if has_fans(s) {
        anim_zone(c, ZONE_FAN, x, y, cw, FAN_CARD_H * sc);
        paint_fan_card(c, x, y, cw, sc, s, t);
    }

    // ---- columna derecha
    x = pad + cw + 8.0 * sc;
    y = y0;

    // ---- Memoria
    let rh = 46.0 * sc;
    card(c, x, y, cw, rh, sc);
    let frac = if s.ram_total > 0.0 { s.ram_used / s.ram_total } else { 0.0 };
    card_head(c, x, y, cw, sc, "RAM", &format!("{:.1} {} {:.1} GB", s.ram_used, tr("de", "of"), s.ram_total), &format!("{:.0}", frac * 100.0), "%", RAM_COL);
    let by = y + 36.0 * sc;
    c.capsule_h(x + 10.0 * sc, x + cw - 10.0 * sc, by, 2.5 * sc, WHITE, 0.12);
    c.capsule_h(x + 10.0 * sc, x + 10.0 * sc + ((cw - 20.0 * sc) * frac).max(5.0 * sc), by, 2.5 * sc, RAM_COL, 1.0);
    y += rh + 8.0 * sc;

    // ---- Discos
    y += paint_disk_card(c, x, y, cw, sc, s) + 8.0 * sc;

    // ---- Energía: batería y plan
    anim_zone(c, ZONE_POWER, x, y, cw, POWER_CARD_H * sc);
    y += paint_power_card(c, x, y, cw, sc, s, t) + 8.0 * sc;

    // ---- Aplicaciones que más consumen
    y += paint_apps_card(c, x, y, cw, sc) + 8.0 * sc;

    // ---- FPS: hasta el final de la ventana
    let fh = h - y - pad;
    card(c, x, y, cw, fh, sc);
    let app = if s.app.is_empty() { tr("sin juego en primer plano", "no game in the foreground").to_string() } else { s.app.trim_end_matches(".exe").to_string() };
    card_head(c, x, y, cw, sc, "FPS", &app, &if s.fps > 0 { s.fps.to_string() } else { "—".into() }, "fps", FPS_COL);
    let ft = if s.fps > 0 { format!("{} {:.1} ms", tr("tiempo por frame", "frame time"), 1000.0 / s.fps as f32) } else { tr("PresentMon mide la ventana en primer plano", "PresentMon measures the foreground window").into() };
    c.text(&ft, x + 10.0 * sc, y + 30.0 * sc, cw - 20.0 * sc, 14.0 * sc, small, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    let fmax = s.hist_fps.iter().fold(60.0f32, |m, v| m.max(*v)) * 1.15;
    c.sparkline(x + 10.0 * sc, y + 48.0 * sc, cw - 20.0 * sc, (fh - 54.0 * sc).max(4.0), &s.hist_fps, fmax, FPS_COL, 1.0);
}

/// Desenfoque "vidrio esmerilado" detrás de la ventana: ACCENT_ENABLE_ACRYLICBLURBEHIND con un tinte
/// casi transparente (SetWindowCompositionAttribute, no documentada pero estable desde Windows 10)
unsafe fn set_accent(h: HWND, state: i32, gradient: u32) {
    #[repr(C)]
    struct AccentPolicy {
        state: i32,
        flags: i32,
        gradient: u32, // AABBGGRR
        anim: i32,
    }
    #[repr(C)]
    struct CompAttrData {
        attrib: i32,
        data: *mut core::ffi::c_void,
        size: usize,
    }
    type SetWca = unsafe extern "system" fn(HWND, *mut CompAttrData) -> BOOL;
    let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return };
    let Some(f) = windows::Win32::System::LibraryLoader::GetProcAddress(user32, windows::core::s!("SetWindowCompositionAttribute")) else { return };
    let f: SetWca = std::mem::transmute(f);
    let mut ap = AccentPolicy { state, flags: 0, gradient, anim: 0 };
    let mut d = CompAttrData { attrib: 19, data: &mut ap as *mut _ as *mut _, size: std::mem::size_of::<AccentPolicy>() }; // WCA_ACCENT_POLICY
    let _ = f(h, &mut d);
}

const POPUP_STYLES: [(&str, &str); 5] = [
    ("glass", "Vidrio esmerilado|Frosted glass"),
    ("clear", "Cristal transparente|Clear glass"),
    ("acrylic", "Acrílico de Windows|Windows acrylic"),
    ("mica", "Mica|Mica"),
    ("dark", "Opaco|Solid"),
];

fn popup_style(cfg: &Config) -> String {
    let s = cfg.v["PopupStyle"].as_str().unwrap_or("glass");
    if POPUP_STYLES.iter().any(|(k, _)| *k == s) { s.to_string() } else { "glass".into() }
}

/// Aplica el fondo de DWM según el estilo: desenfoque con tinte (acento), desenfoque sin tinte,
/// acrílico o mica del sistema, u opaco. Sigue el modo claro/oscuro de Windows.
unsafe fn apply_popup_style(h: HWND, style: &str) {
    use windows::Win32::Graphics::Dwm::*;
    let light = render::is_light();
    let dark: i32 = if light { 0 } else { 1 };
    let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
    // (estado del acento, tinte AABBGGRR, fondo del sistema DWMSBT)
    let (accent, gradient, backdrop): (i32, u32, i32) = match style {
        "clear" => (3, 0, 1),           // ACCENT_ENABLE_BLURBEHIND, DWMSBT_NONE
        "acrylic" => (0, 0, 3),         // DWMSBT_TRANSIENTWINDOW
        "mica" => (0, 0, 2),            // DWMSBT_MAINWINDOW
        "dark" => (0, 0, 1),            // sin desenfoque
        _ if light => (4, 0x30FA_F8F6, 1), // ACCENT_ENABLE_ACRYLICBLURBEHIND, tinte claro ~19 %
        _ => (4, 0x1A18_1210, 1),       // ACCENT_ENABLE_ACRYLICBLURBEHIND, tinte azulado ~10 %
    };
    set_accent(h, accent, gradient);
    let _ = DwmSetWindowAttribute(h, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as *const _, 4);
}

/// Crea (una vez) la ventana flotante con desenfoque, esquinas redondeadas y modo oscuro
unsafe fn ensure_popup(app: &mut App) -> HWND {
    if !app.popup.is_invalid() {
        return app.popup;
    }
    use windows::Win32::Graphics::Dwm::*;
    let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
    let Ok(h) = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        POPUP_CLASS,
        w!(""),
        WS_POPUP,
        0, 0, 10, 10,
        HWND::default(),
        HMENU::default(),
        hinst,
        None,
    ) else {
        return HWND::default();
    };
    let round: i32 = 2; // DWMWCP_ROUND
    let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
    let m = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
    let _ = DwmExtendFrameIntoClientArea(h, &m);
    app.popup = h;
    h
}

unsafe fn paint_popup_window(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    // pintado parcial (solo las zonas animadas invalidadas) o completo
    let p = ps.rcPaint;
    let mut full = p.left <= 0 && p.top <= 0 && p.right >= rc.right && p.bottom >= rc.bottom;
    let s = st().clone();
    with_app(|app| {
        if app.popup_surf.as_ref().map(|x| x.w != rc.right || x.h != rc.bottom).unwrap_or(true) {
            app.popup_surf = Surface::new(rc.right, rc.bottom);
            full = true;
        }
        render::set_light(app.light_of(app.popup_tb));
        let sc = app.popup_scale;
        let tt = app.t0.elapsed().as_secs_f32();
        let style = spectrum_style(&app.cfg);
        let (wf, hf) = (rc.right as f32, rc.bottom as f32);
        if let (Some(surf), Some(kind)) = (app.popup_surf.as_mut(), app.popup_kind) {
            match kind {
                Kind::System => paint_popup_system(surf, wf, hf, sc, &s, tt, !full),
                Kind::Ip => paint_popup_ip(surf, wf, hf, sc, &s, tt, !full),
                Kind::Usage => paint_popup_ai(surf, wf, hf, sc, &AiView::from_cfg(&app.cfg)),
                Kind::Media => paint_popup_media(surf, wf, hf, sc, &s, &mut app.popup_disp, tt, style),
                Kind::Radio => paint_popup_radio(surf, wf, hf, sc, tt),
            }
            surf.blit(hdc);
        }
    });
    let _ = EndPaint(hwnd, &ps);
}

/// Registro de diagnóstico (solo si existe %APPDATA%\TaskbarPanels\debug.flag)
fn dbg_on() -> bool {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| util::appdata_dir().join("debug.flag").exists());
    *ON
}

fn dbg(msg: &str) {
    if dbg_on() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(util::appdata_dir().join("debug.log")) {
            let _ = writeln!(f, "{:?} {msg}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
        }
    }
}

thread_local! {
    /// desde cuándo el cursor está fuera del panel y de su ventana flotante
    static POPUP_LEFT: RefCell<Option<Instant>> = const { RefCell::new(None) };
}

/// Ventana flotante: aparece si el cursor lleva 300 ms sobre un panel; sigue abierta mientras el
/// cursor esté sobre el panel o sobre la propia ventana; cambia al instante entre paneles.
unsafe fn update_popup() {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    with_app(|app| {
        let inside = |r: &RECT| pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom;
        let mut over: Option<(RECT, f32, isize, Kind)> = None;
        // lejos de las barras (casi siempre) no hace falta consultar la posición de cada panel
        let near = app.bars.iter().any(|b| inside(&b.rect));
        for p in app.panels.iter().filter(|_| near) {
            if !IsWindowVisible(p.hwnd).as_bool() {
                continue;
            }
            let mut r = RECT::default();
            let _ = GetWindowRect(p.hwnd, &mut r);
            if inside(&r) {
                over = Some((r, p.scale, p.tb_key, p.kind));
            }
        }
        let on_popup = app.popup_shown && {
            let mut r = RECT::default();
            let _ = GetWindowRect(app.popup, &mut r);
            inside(&r)
        };
        if over.is_some() || on_popup {
            POPUP_LEFT.with(|l| *l.borrow_mut() = None);
        }
        match over {
            Some((r, sc, tb_key, kind)) => {
                let since = *app.hover_since.get_or_insert_with(Instant::now);
                let switch = app.popup_shown && app.popup_kind != Some(kind);
                if (since.elapsed().as_millis() >= 300 && !app.popup_shown) || switch {
                    let h = ensure_popup(app);
                    if h.is_invalid() {
                        return;
                    }
                    let style = popup_style(&app.cfg);
                    app.popup_tb = tb_key;
                    let light = app.light_of(tb_key);
                    render::set_light(light);
                    if style != app.popup_style || app.popup_light != Some(light) {
                        app.popup_light = Some(light);
                        apply_popup_style(h, &style);
                        POPUP_STYLE.with(|s| *s.borrow_mut() = style.clone());
                        app.popup_style = style;
                    }
                    app.popup_scale = sc;
                    app.popup_kind = Some(kind);
                    if kind == Kind::Radio {
                        // catálogo del país elegido: se descarga la primera vez y se renueva a diario
                        let country = radio::radio().country.clone();
                        radio::ensure_list(&country);
                    }
                    let (pw, ph) = ((popup_w(kind) * sc) as i32, (popup_h(kind) * sc) as i32);
                    let bar = app.bars.iter().find(|b| b.key == tb_key);
                    let mon = bar.map(|b| b.mon).unwrap_or(RECT { left: r.left - pw, top: r.top - ph - 40, right: r.right + pw, bottom: r.bottom + ph + 40 });
                    let gap = (10.0 * sc) as i32;
                    let x = ((r.left + r.right) / 2 - pw / 2).clamp(mon.left + 8, (mon.right - pw - 8).max(mon.left + 8));
                    let at_top = bar.map(|b| b.at_top).unwrap_or(false);
                    let (vertical, side_left) = bar.map(|b| (b.vertical, b.side_left)).unwrap_or((false, false));
                    // barra vertical → la ventana se abre hacia el centro de la pantalla, junto al panel
                    let x = if vertical { if side_left { r.right + gap } else { r.left - pw - gap } } else { x };
                    let x = x.clamp(mon.left + 8, (mon.right - pw - 8).max(mon.left + 8));
                    // barra arriba → hacia abajo; barra abajo → hacia arriba; vertical → centrada en el panel
                    let y = if vertical { (r.top + r.bottom) / 2 - ph / 2 } else if at_top { r.bottom + gap } else { r.top - ph - gap };
                    let y = y.clamp(mon.top + 8, (mon.bottom - ph - 8).max(mon.top + 8));
                    let r2 = SetWindowPos(h, HWND_TOPMOST, x, y, pw, ph, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    dbg(&format!("MOSTRAR {kind:?} en ({x},{y}) {pw}x{ph} → {r2:?}"));
                    let _ = InvalidateRect(h, None, FALSE);
                    app.popup_shown = true;
                }
            }
            None if on_popup => {} // el cursor pasó del panel a la ventana: se mantiene
            None => {
                app.hover_since = None;
                // la ventana de radio se maneja con el ratón: al cruzar el hueco entre el panel y la
                // ventana no se cierra al instante (0,4 s de margen)
                let left = POPUP_LEFT.with(|l| *l.borrow_mut().get_or_insert_with(Instant::now));
                // y mientras se escribe en su buscador sigue abierta aunque el cursor se salga. El foco se
                // comprueba de verdad (si la activación falló o se perdió sin WM_KILLFOCUS, la marca
                // quedaba puesta y la ventana no se cerraba nunca) y caduca al dejar de usarla
                let focus = RADIO_FOCUS.with(|f| f.get()) && GetForegroundWindow() == app.popup;
                RADIO_FOCUS.with(|f| f.set(focus));
                let recent = |t: Instant| t.elapsed().as_secs() < RADIO_FOCUS_HOLD;
                let typing = focus && (recent(left) || RADIO_USED.with(|u| u.get()).is_some_and(recent));
                let grace = app.popup_kind == Some(Kind::Radio) && (left.elapsed().as_millis() < 400 || typing);
                if app.popup_shown && !grace {
                    dbg("OCULTAR");
                    if app.popup_kind == Some(Kind::Radio) && !radio::radio().query.is_empty() {
                        radio::set_query(""); // la próxima vez se abre con la lista completa
                    }
                    let _ = ShowWindow(app.popup, SW_HIDE);
                    app.popup_shown = false;
                    app.popup_kind = None;
                    app.popup_hidden_at = Some(Instant::now());
                }
            }
        }
        // ventana cerrada hace más de 15 s: liberar su lienzo y el fondo precalculado (~4 MB a 150 %)
        if !app.popup_shown && app.popup_hidden_at.map(|t| t.elapsed().as_secs() >= 15).unwrap_or(false) {
            app.popup_surf = None;
            SYS_ANIM.with(|a| *a.borrow_mut() = ((0, 0), Vec::new()));
            app.popup_disp = Vec::new();
            app.popup_hidden_at = None;
            schedule_trim(5);
        }
        net::set_active(app.popup_shown && app.popup_kind == Some(Kind::Ip));
        sys::set_procs_active(app.popup_shown && app.popup_kind == Some(Kind::System));
        // multimedia: animación a 60 fps (espectro, progreso); red: ~15 fps (LED y ondas del router);
        // sistema: ~20 fps, pero solo las tarjetas animadas (ventiladores y energía), no toda la ventana
        let frame = (app.t0.elapsed().as_millis() / 16) as u64;
        if app.popup_shown {
            match app.popup_kind {
                Some(Kind::Media) => {
                    let _ = InvalidateRect(app.popup, None, FALSE);
                }
                Some(Kind::Ip) if frame % 4 == 0 => {
                    if speedtest::running() {
                        let _ = InvalidateRect(app.popup, None, FALSE);
                    } else {
                        for r in sys_anim_rects() {
                            let _ = InvalidateRect(app.popup, Some(&r), FALSE);
                        }
                    }
                }
                Some(Kind::System) if frame % 3 == 0 => {
                    for r in sys_anim_rects() {
                        let _ = InvalidateRect(app.popup, Some(&r), FALSE);
                    }
                }
                // radio: ~8 fps (ecualizador, halo y cambios de estado al conectar)
                Some(Kind::Radio) if frame % 8 == 0 => {
                    let _ = InvalidateRect(app.popup, None, FALSE);
                }
                _ => {}
            }
        }
    });
}

// Paleta neón cíclica: cian → azul eléctrico → violeta → magenta → rosa → cian
const NEON: [Rgb; 6] = [
    Rgb::u8(0, 255, 240), Rgb::u8(40, 140, 255), Rgb::u8(150, 70, 255),
    Rgb::u8(255, 40, 220), Rgb::u8(255, 70, 140), Rgb::u8(0, 255, 240),
];

fn neon_at(t: f32) -> Rgb {
    let t = t.rem_euclid(1.0) * 5.0;
    let i = (t.floor() as usize).min(4);
    NEON[i].mix(NEON[i + 1], t - i as f32)
}

/// Posición y ancho del panel multimedia:
///  · iconos centrados → a la izquierda, sin pasar del botón Inicio
///  · iconos a la izquierda → justo a la derecha del último icono, sin invadir los paneles de la derecha
/// None si el espacio libre es demasiado estrecho.
/// ancho mínimo del multimedia en barra horizontal (px lógicos); con menos espacio se oculta
const MEDIA_MIN_W: f32 = 320.0;

fn media_place(tb: &Taskbar, right_limit: i32) -> Option<(i32, i32)> {
    let sc = tb.scale;
    // margen de 16 px lógicos con los iconos para que nunca se monte sobre ellos
    let (x, avail) = if tb.left_aligned {
        let x = tb.tasks_right + (16.0 * sc) as i32;
        (x, (right_limit - x) as f32 - 12.0 * sc)
    } else {
        // tampoco debe pasar de los paneles de la derecha (pueden arrastrarse hasta aquí)
        let x = tb.left + (12.0 * sc) as i32;
        let lim = ((tb.start_left - x) as f32 - 16.0 * sc).min(right_limit.saturating_sub(x) as f32 - 12.0 * sc);
        (x, lim)
    };
    if avail < MEDIA_MIN_W * sc {
        return None; // sin espacio suficiente: mejor ocultarlo que taparlos
    }
    Some((x, (Kind::Media.width() * sc).min(avail) as i32))
}

/// Interpolación Catmull-Rom de los niveles en la posición t ∈ [0, n-1]
fn catmull(v: &[f32], t: f32) -> f32 {
    let n = v.len() as i32;
    let i = t.floor() as i32;
    let f = t - i as f32;
    let g = |k: i32| v[k.clamp(0, n - 1) as usize];
    let (p0, p1, p2, p3) = (g(i - 1), g(i), g(i + 1), g(i + 2));
    0.5 * (2.0 * p1 + (-p0 + p2) * f + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * f * f + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * f * f * f)
}

/// Onda líquida simétrica: relleno degradado, borde luminoso, halo y envolvente de picos
#[allow(clippy::too_many_arguments)]
fn spectrum_wave(c: &mut Surface, x0: f32, y0: f32, w: f32, h: f32, sc: f32, lv: &[f32], peaks: &[f32], phase: f32, pulse: f32) {
    let mid = y0 + h / 2.0;
    let amp = h / 2.0 - 2.0;
    let glow_r = 3.5 * sc;
    let n = lv.len() as f32 - 1.0;
    // línea central tenue
    c.fill(x0, mid - 0.5, w, 1.0, Rgb::u8(120, 140, 255), 0.18);
    for px in 0..w as i32 {
        let u = px as f32 / w;
        let taper = 0.35 + 0.65 * (std::f32::consts::PI * u).sin().powf(0.6); // extremos más suaves
        let v = catmull(lv, u * n).clamp(0.0, 1.0) * taper;
        let pk = catmull(peaks, u * n).clamp(0.0, 1.0) * taper;
        let hgt = (v * amp).max(0.6);
        let col = neon_at(u * 0.85 + phase);
        let x = x0 as i32 + px;
        let y_from = (mid - hgt - glow_r * 2.2).floor() as i32;
        let y_to = (mid + hgt + glow_r * 2.2).ceil() as i32;
        let pk_h = pk * amp;
        for py in y_from..y_to {
            let dy = (py as f32 + 0.5 - mid).abs();
            let d = dy - hgt;
            if d < 0.0 {
                // relleno: más intenso cerca del borde, translúcido en el centro
                let k = dy / hgt;
                c.blend(x, py, col, 0.16 + 0.55 * k * k);
            } else {
                c.blend(x, py, col, (0.38 + 0.35 * pulse) * (-(d / glow_r).powi(2)).exp());
            }
            // borde luminoso
            let e = (1.0 - d.abs() / (0.9 * sc.max(1.0))).clamp(0.0, 1.0);
            if e > 0.0 {
                c.blend(x, py, col.mix(WHITE, 0.45), e * 0.95);
            }
            // envolvente de picos: línea fina blanca que cae lentamente
            let pe = (1.0 - (dy - pk_h - 1.5).abs() / 0.8).clamp(0.0, 1.0);
            if pe > 0.0 && pk_h > hgt + 1.0 {
                c.blend(x, py, WHITE, pe * 0.45);
            }
        }
    }
}

/// Barras cápsula: núcleo brillante, halo y puntos de pico flotantes
#[allow(clippy::too_many_arguments)]
fn spectrum_bars(c: &mut Surface, x0: f32, y0: f32, w: f32, h: f32, sc: f32, lv: &[f32], peaks: &[f32], phase: f32, pulse: f32) {
    let bars = 40usize;
    let n = lv.len() as f32 - 1.0;
    let slot = w / bars as f32;
    let bw = (slot * 0.56).max(2.0);
    let r = bw / 2.0;
    let mid = y0 + h / 2.0;
    let amp = h / 2.0 - r - 1.0;
    for i in 0..bars {
        let u = (i as f32 + 0.5) / bars as f32;
        let v = catmull(lv, u * n).clamp(0.0, 1.0);
        let pk = catmull(peaks, u * n).clamp(0.0, 1.0);
        let col = neon_at(u * 0.85 + phase);
        let cx = x0 + (i as f32 + 0.5) * slot;
        let hh = (v * amp).max(0.0);
        c.capsule(cx, mid - hh - r, mid + hh + r, r, col, 1.0, 3.0 * sc, 0.30 + 0.35 * pulse, 0.55);
        let ph = pk * amp + r * 2.2;
        if pk > v + 0.03 {
            c.capsule(cx, mid - ph - r * 0.7, mid - ph + r * 0.7, r * 0.7, WHITE, 0.75, 0.0, 0.0, 0.0);
            c.capsule(cx, mid + ph - r * 0.7, mid + ph + r * 0.7, r * 0.7, WHITE, 0.75, 0.0, 0.0, 0.0);
        }
    }
}

/// Estilos de espectro: (clave en SpectrumStyle, nombre en español, nombre en inglés)
const SPECTRUM_STYLES: [(&str, &str, &str); 5] = [
    ("wave", "Onda líquida", "Liquid wave"),
    ("bars", "Barras cápsula", "Capsule bars"),
    ("led", "LED segmentado", "Segmented LED"),
    ("mirror", "Barras con reflejo", "Mirrored bars"),
    ("line", "Línea neón", "Neon line"),
];

/// Estilo de espectro de la configuración (desconocido → onda)
fn spectrum_style(cfg: &Config) -> &'static str {
    let k = cfg.v["SpectrumStyle"].as_str().unwrap_or("wave");
    SPECTRUM_STYLES.iter().find(|s| s.0 == k).map(|s| s.0).unwrap_or("wave")
}

/// Dibuja el espectro con el estilo elegido
#[allow(clippy::too_many_arguments)]
fn spectrum(c: &mut Surface, style: &str, x0: f32, y0: f32, w: f32, h: f32, sc: f32, lv: &[f32], peaks: &[f32], phase: f32, pulse: f32) {
    if lv.len() < 2 || w < 4.0 || h < 4.0 {
        return;
    }
    match style {
        "bars" => spectrum_bars(c, x0, y0, w, h, sc, lv, peaks, phase, pulse),
        "led" => spectrum_led(c, x0, y0, w, h, sc, lv, peaks, phase, pulse),
        "mirror" => spectrum_mirror(c, x0, y0, w, h, sc, lv, peaks, phase, pulse),
        "line" => spectrum_line(c, x0, y0, w, h, sc, lv, peaks, phase, pulse),
        _ => spectrum_wave(c, x0, y0, w, h, sc, lv, peaks, phase, pulse),
    }
}

/// LED segmentado: columnas de segmentos de abajo hacia arriba con los colores neón del resto de
/// estilos (un tono por columna que rota con el tiempo), más claros cuanto más altos, y el segmento
/// de pico retenido
#[allow(clippy::too_many_arguments)]
fn spectrum_led(c: &mut Surface, x0: f32, y0: f32, w: f32, h: f32, sc: f32, lv: &[f32], peaks: &[f32], phase: f32, pulse: f32) {
    let n = lv.len() as f32 - 1.0;
    let cols = ((w / (7.0 * sc)).floor() as usize).clamp(8, 32);
    let slot = w / cols as f32;
    let cw = (slot * 0.74).max(2.0);
    let gap = (1.0 * sc).max(1.0);
    let seg = (2.6 * sc).max(2.0);
    let nseg = ((h + gap) / (seg + gap)).floor().max(3.0) as usize;
    for i in 0..cols {
        let u = (i as f32 + 0.5) / cols as f32;
        let v = catmull(lv, u * n).clamp(0.0, 1.0);
        let pk = catmull(peaks, u * n).clamp(0.0, 1.0);
        let lit = (v * nseg as f32).round() as usize;
        let peak = ((pk * nseg as f32).round() as usize).min(nseg).saturating_sub(1);
        let x = x0 + i as f32 * slot + (slot - cw) / 2.0;
        let neon = neon_at(u * 0.85 + phase);
        for s in 0..nseg {
            let f = s as f32 / (nseg - 1).max(1) as f32;
            let col = neon.mix(WHITE, 0.32 * f * f);
            let y = y0 + h - (s + 1) as f32 * (seg + gap) + gap;
            if s < lit {
                // halo suave alrededor de los segmentos encendidos (más intenso con el pulso del bajo)
                c.round_rect(x - 1.0 * sc, y - 0.6 * sc, cw + 2.0 * sc, seg + 1.2 * sc, 1.4 * sc, neon, 0.10 + 0.12 * pulse);
                c.round_rect(x, y, cw, seg, 0.8 * sc, col, 1.0);
            } else if s == peak && pk > v + 0.02 {
                c.round_rect(x, y, cw, seg, 0.8 * sc, col.mix(WHITE, 0.5), 0.9);
            } else {
                c.round_rect(x, y, cw, seg, 0.8 * sc, col, 0.09);
            }
        }
    }
}

/// Barras con reflejo: barras finas desde una línea base, reflejo difuminado debajo y punto de pico
#[allow(clippy::too_many_arguments)]
fn spectrum_mirror(c: &mut Surface, x0: f32, y0: f32, w: f32, h: f32, sc: f32, lv: &[f32], peaks: &[f32], phase: f32, pulse: f32) {
    let n = lv.len() as f32 - 1.0;
    let bars = ((w / (5.5 * sc)).floor() as usize).clamp(10, 48);
    let slot = w / bars as f32;
    let bw = (slot * 0.62).max(1.5);
    let base = y0 + h * 0.68;
    let up = h * 0.66 - 2.0 * sc;
    let down = h * 0.30;
    c.fill(x0, base, w, (0.8 * sc).max(1.0), WHITE, 0.10 + 0.10 * pulse);
    for i in 0..bars {
        let u = (i as f32 + 0.5) / bars as f32;
        let v = catmull(lv, u * n).clamp(0.0, 1.0);
        let pk = catmull(peaks, u * n).clamp(0.0, 1.0);
        let col = neon_at(u * 0.85 + phase);
        let x = x0 + i as f32 * slot + (slot - bw) / 2.0;
        let hh = (v * up).max(1.0 * sc);
        // barra con el extremo superior más claro
        c.round_rect(x, base - hh, bw, hh, bw * 0.35, col, 0.95);
        c.round_rect(x, base - hh, bw, (hh * 0.25).min(3.0 * sc), bw * 0.35, col.mix(WHITE, 0.55), 0.9);
        // reflejo: bandas con opacidad decreciente
        let rh = v * down;
        let steps = 5;
        for k in 0..steps {
            let a = 0.26 * (1.0 - k as f32 / steps as f32);
            c.fill(x, base + 1.0 * sc + rh * k as f32 / steps as f32, bw, rh / steps as f32, col, a);
        }
        // punto de pico flotante
        if pk > v + 0.03 {
            c.fill(x, base - pk * up - 2.2 * sc, bw, (1.2 * sc).max(1.0), WHITE, 0.8);
        }
    }
}

/// Línea neón: curva luminosa con resplandor, relleno degradado bajo la curva y envolvente de picos
#[allow(clippy::too_many_arguments)]
fn spectrum_line(c: &mut Surface, x0: f32, y0: f32, w: f32, h: f32, sc: f32, lv: &[f32], peaks: &[f32], phase: f32, pulse: f32) {
    let n = lv.len() as f32 - 1.0;
    let bottom = y0 + h;
    let amp = h - 3.0 * sc;
    let ycurve = |data: &[f32], u: f32| {
        let taper = 0.55 + 0.45 * (std::f32::consts::PI * u).sin().max(0.0).powf(0.5); // sin(π) < 0 por redondeo → NaN
        bottom - 1.0 * sc - catmull(data, u * n).clamp(0.0, 1.0) * taper * amp
    };
    // relleno bajo la curva (columna a columna) con degradado hacia abajo
    for px in 0..w as i32 {
        let u = (px as f32 + 0.5) / w;
        let y = ycurve(lv, u);
        let col = neon_at(u * 0.85 + phase);
        let span = (bottom - y).max(1.0);
        for py in y.floor() as i32..bottom.ceil() as i32 {
            let t = ((py as f32 + 0.5 - y) / span).clamp(0.0, 1.0);
            c.blend(x0 as i32 + px, py, col, 0.30 * (1.0 - t).powf(1.4));
        }
    }
    // resplandor, línea y envolvente de picos (segmentos cada 2 px)
    let step = (2.0 * sc).max(2.0);
    let mut prev: Option<(f32, f32, f32)> = None;
    let mut x = 0.0;
    while x <= w {
        let u = (x / w).min(1.0);
        let (y, yp) = (ycurve(lv, u), ycurve(peaks, u));
        if let Some((xa, ya, ypa)) = prev {
            let col = neon_at(u * 0.85 + phase);
            c.line(x0 + xa, ya, x0 + x, y, 5.0 * sc, col, 0.16 + 0.14 * pulse);
            c.line(x0 + xa, ya, x0 + x, y, 1.7 * sc, col.mix(WHITE, 0.35), 1.0);
            if ypa < ya - 1.5 || yp < y - 1.5 {
                c.line(x0 + xa, ypa, x0 + x, yp, 0.9 * sc, WHITE, 0.35);
            }
        }
        prev = Some((x, y, yp));
        x += step;
    }
}

/// Distribución del panel multimedia (compartida por el dibujo y la detección de clics)
struct MediaLayout {
    art_x: f32,
    art_y: f32,
    art: f32,
    cy: f32,
    /// (centro x, centro y, radio) de anterior, reproducir/pausa, siguiente
    btn: [(f32, f32, f32); 3],
    text_x: f32,
    text_w: f32,
    viz_x: f32,
    viz_w: f32,
    title_y: f32,
    viz_y: f32,
    viz_h: f32,
}

fn media_layout(w: f32, h: f32, sc: f32) -> MediaLayout {
    let art = (h * 0.78).min(56.0 * sc).floor();
    let art_x = (8.0 * sc).round();
    let art_y = ((h - art) / 2.0).floor();
    let c0 = art_x + art + art * 0.36 + 10.0 * sc; // reserva para el vinilo que asoma
    let pr = (h * 0.27).min(16.0 * sc); // botón principal
    let sr = pr * 0.78; // botones secundarios
    let prev = c0 + sr;
    let play = prev + sr + 6.0 * sc + pr;
    let next = play + pr + 6.0 * sc + sr;
    let text_x = next + sr + 12.0 * sc;
    let viz_w = (190.0 * sc).min(w * 0.26);
    let text_w = (w - text_x - viz_w - 14.0 * sc).max(40.0 * sc);
    MediaLayout {
        art_x,
        art_y,
        art,
        cy: h / 2.0,
        btn: [(prev, h / 2.0, sr), (play, h / 2.0, pr), (next, h / 2.0, sr)],
        text_x,
        text_w,
        viz_x: text_x + text_w + 8.0 * sc,
        viz_w,
        title_y: h * 0.06,
        viz_y: h * 0.06,
        viz_h: h * 0.88,
    }
}

fn luminance(c: Rgb) -> f32 {
    0.299 * c.0 + 0.587 * c.1 + 0.114 * c.2
}

#[allow(clippy::too_many_arguments)]
fn paint_media(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, p: &mut Panel, style: &str, t: f32) {
    let l = media_layout(w, h, sc);
    let dt = (t - p.last_t).clamp(0.0, 0.1);
    p.last_t = t;

    // ---------------- disco: carátula + vinilo que asoma y gira mientras suena
    let art = media::ART.lock().unwrap().clone();
    let has_art = art.as_ref().map(|a| !a.px.is_empty()).unwrap_or(false);
    let accent = art.as_ref().map(|a| Rgb::u8(a.accent.0, a.accent.1, a.accent.2)).unwrap_or(Rgb::u8(0, 230, 255));
    let target = if s.playing { 1.0 } else { 0.0 };
    p.vinyl += (target - p.vinyl) * (1.0 - (-dt * 6.0).exp()); // deslizamiento suave
    if s.playing {
        p.spin = (p.spin + dt * 3.5) % std::f32::consts::TAU; // ~33 rpm
    }
    let (acx, acy) = (l.art_x + l.art / 2.0, l.art_y + l.art / 2.0);
    // halo del color del álbum detrás del disco
    c.capsule(acx + l.art * 0.18 * p.vinyl, acy - l.art * 0.30, acy + l.art * 0.30, l.art * 0.2, accent, 0.0, l.art * 0.22, 0.10 + 0.20 * p.vinyl, 0.0);
    let disc_r = l.art / 2.0 * 0.97;
    if has_art {
        if p.vinyl > 0.01 {
            c.vinyl(acx + l.art * 0.36 * p.vinyl, acy, disc_r, p.spin, accent, p.vinyl.min(1.0));
        }
        let a = art.as_ref().unwrap();
        c.image_rounded(l.art_x, l.art_y, l.art, (6.0 * sc).round(), &a.px, media::ART_SIZE, media::ART_SIZE, 1.0);
    } else {
        c.vinyl(acx, acy, disc_r, p.spin, accent, 1.0); // sin carátula: solo el vinilo
    }

    // ---------------- botones
    let glyph_px = |r: f32| (r * 0.95).floor();
    for (i, (bx, by, br)) in l.btn.iter().enumerate() {
        let _ = by;
        let hover = p.hover == i as i32;
        if i == 1 {
            // botón principal: círculo con el color del álbum, halo al pasar el mouse
            let fill = if hover { accent.mix(WHITE, 0.15) } else { accent };
            c.dot(*bx, l.cy, *br, fill, 1.0, 4.0 * sc, if hover { 0.55 } else { 0.25 });
            let ink = if luminance(fill) > 150.0 { Rgb::u8(16, 16, 22) } else { WHITE };
            if s.playing {
                // pausa: dos barras sólidas redondeadas
                let (bw, bh) = (br * 0.22, br * 0.78);
                for dx in [-br * 0.22, br * 0.22] {
                    c.capsule(bx + dx, l.cy - bh / 2.0, l.cy + bh / 2.0, bw / 2.0, ink, 1.0, 0.0, 0.0, 0.0);
                }
            } else {
                // reproducir: triángulo sólido (PlaySolid), desplazado para centrarlo ópticamente
                c.text("\u{F5B0}", bx - br + 1.0 * sc, l.cy - br, br * 2.0, br * 2.0, glyph_px(*br), 400, "Segoe Fluent Icons", ink, DT_CENTER);
            }
        } else {
            if hover {
                c.dot(*bx, l.cy, *br, WHITE, 0.10, 0.0, 0.0);
            }
            let col = if hover { WHITE } else { Rgb::u8(190, 192, 205) };
            let g = if i == 0 { "\u{E892}" } else { "\u{E893}" };
            c.text(g, bx - br, l.cy - br, br * 2.0, br * 2.0, glyph_px(*br) * 0.9, 400, "Segoe Fluent Icons", col, DT_CENTER);
        }
    }

    // ---------------- título (se desplaza si no cabe), artista y progreso
    let (ty, th) = (h * 0.06, h * 0.40);
    let tpx = (12.7 * sc).min((h * 0.27).floor());
    let tw = c.text_width(&s.title, tpx, 600, "Segoe UI");
    let title_col = Rgb::u8(245, 245, 248);
    if tw <= l.text_w {
        c.text(&s.title, l.text_x, ty, l.text_w, th, tpx, 600, "Segoe UI", title_col, DT_LEFT);
    } else {
        let (speed, gap, wait) = (28.0 * sc, 48.0 * sc, 2.5);
        let cycle = (tw + gap) / speed;
        let ph = t % (cycle + wait);
        let off = if ph < wait { 0.0 } else { (ph - wait) * speed };
        let (c0, c1, fade) = (l.text_x, l.text_x + l.text_w, 12.0 * sc);
        c.text_clip(&s.title, l.text_x - off, ty, th, tpx, 600, "Segoe UI", title_col, c0, c1, fade);
        c.text_clip(&s.title, l.text_x - off + tw + gap, ty, th, tpx, 600, "Segoe UI", title_col, c0, c1, fade);
    }
    let apx = (11.0 * sc).min((h * 0.23).floor());
    c.text(&s.artist, l.text_x, h * 0.44, l.text_w, h * 0.30, apx, 400, "Segoe UI", Rgb::u8(165, 170, 185), DT_LEFT | DT_END_ELLIPSIS);
    let tl = *media::TIMELINE.lock().unwrap();
    if tl.dur > 1.0 {
        let pos = tl.pos + if s.playing { tl.at.map(|a| a.elapsed().as_secs_f64()).unwrap_or(0.0) } else { 0.0 };
        let frac = (pos / tl.dur).clamp(0.0, 1.0) as f32;
        let (py, ph) = (h * 0.85, (2.5 * sc).max(2.0));
        c.capsule_h(l.text_x, l.text_x + l.text_w, py, ph / 2.0, WHITE, 0.14);
        if frac > 0.0 {
            c.capsule_h(l.text_x, l.text_x + (l.text_w * frac).max(ph), py, ph / 2.0, accent, 1.0);
        }
    }

    // ---------------- espectro
    let target = media::LEVELS.lock().unwrap().clone();
    if p.disp.len() != target.len() {
        p.disp = vec![0.0; target.len()];
        p.caps = vec![0.0; target.len()];
    }
    for i in 0..target.len() {
        // constantes pensadas para 60 fps, ajustadas al tiempo real entre fotogramas
        let k: f32 = if target[i] > p.disp[i] { 0.45 } else { 0.12 };
        p.disp[i] += (target[i] - p.disp[i]) * (1.0 - (1.0 - k).powf(dt * 60.0));
        p.caps[i] = (p.caps[i] - 0.006 * dt * 60.0).max(p.disp[i]);
    }
    let pulse = (p.disp.iter().take(6).sum::<f32>() / 6.0).clamp(0.0, 1.0);
    let phase = (t / 14.0).rem_euclid(1.0);
    let (disp, caps) = (p.disp.clone(), p.caps.clone());
    spectrum(c, style, l.viz_x, h * 0.06, l.viz_w, h * 0.88, sc, &disp, &caps, phase, pulse);
}

// ======================================================================= barras verticales (izquierda/derecha)
/// Alto (px lógicos) de cada panel apilado en una barra vertical
fn v_height(k: Kind, show_ip: bool, ai_rows: usize) -> f32 {
    match k {
        Kind::Ip => if show_ip { 96.0 } else { 66.0 },
        Kind::Usage => 6.0 + 30.0 * ai_rows.max(1) as f32,
        Kind::System => 144.0,
        Kind::Media => 0.0, // depende del ancho (media_v_height)
        Kind::Radio => 48.0,
    }
}

/// Velocidad abreviada para barras estrechas: "12.4M", "153K", "880" (bits/s)
fn rate_short(bytes_per_s: f64) -> String {
    let b = bytes_per_s * 8.0;
    if b >= 1e9 {
        format!("{:.1}G", b / 1e9)
    } else if b >= 1e6 {
        if b >= 1e8 { format!("{:.0}M", b / 1e6) } else { format!("{:.1}M", b / 1e6) }
    } else if b >= 1e3 {
        format!("{:.0}K", b / 1e3)
    } else {
        format!("{b:.0}")
    }
}

struct VBlock<'a> {
    label: &'a str,
    value: String,
    sub: String,
    sub_col: Rgb,
    col: Rgb,
    hist: Option<(&'a [f32], f32)>,
    frac: Option<f32>,
}

/// Bloques apilados: etiqueta pequeña de color (y dato secundario a la derecha), valor grande
/// ajustado al ancho, y gráfica de fondo o barra de progreso
fn paint_vblocks(c: &mut Surface, w: f32, h: f32, sc: f32, blocks: &[VBlock]) {
    let bh = h / blocks.len() as f32;
    let m = 3.0 * sc;
    for (i, b) in blocks.iter().enumerate() {
        let y = i as f32 * bh;
        if let Some((hist, max)) = b.hist {
            c.sparkline(0.0, y + 1.0, w, bh - 2.0, hist, max, b.col, 0.9);
        }
        let lpx = (8.0 * sc).min((bh * 0.28).floor());
        c.text(b.label, m, y + 2.0 * sc, w - 2.0 * m, lpx + 4.0 * sc, lpx, 700, "Segoe UI", b.col, DT_LEFT | DT_END_ELLIPSIS);
        if !b.sub.is_empty() {
            c.text(&b.sub, m, y + 2.0 * sc, w - 2.0 * m, lpx + 4.0 * sc, lpx, 600, "Segoe UI", b.sub_col, DT_RIGHT);
        }
        let mut vpx = (13.0 * sc).min((bh * 0.42).floor());
        let tw = c.text_width(&b.value, vpx, 600, "Segoe UI");
        if tw > w - 2.0 * m {
            vpx = (vpx * (w - 2.0 * m) / tw).max(7.0 * sc).floor();
        }
        let vy = y + lpx + 4.0 * sc;
        let vh = bh - lpx - 4.0 * sc - if b.frac.is_some() { 5.0 * sc } else { 1.0 * sc };
        c.text(&b.value, m, vy, w - 2.0 * m, vh, vpx, 600, "Segoe UI", b.col.mix(WHITE, 0.35), DT_LEFT | DT_END_ELLIPSIS);
        if let Some(f) = b.frac {
            let by = y + bh - 4.0 * sc;
            c.capsule_h(m, w - m, by, 1.5 * sc, WHITE, 0.14);
            c.capsule_h(m, m + ((w - 2.0 * m) * f.clamp(0.0, 1.0)).max(3.0 * sc), by, 1.5 * sc, b.col, 1.0);
        }
    }
}

fn paint_ip_v(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, show_ip: bool) {
    let maxv = s.hist_down.iter().chain(s.hist_up.iter()).fold(8_000.0f32, |m, v| m.max(*v)) * 1.15;
    let mut blocks = vec![
        VBlock { label: "↓", value: rate_short(s.down_bps), sub: String::new(), sub_col: DIM, col: DOWN_COL, hist: Some((&s.hist_down, maxv)), frac: None },
        VBlock { label: "↑", value: rate_short(s.up_bps), sub: String::new(), sub_col: DIM, col: UP_COL, hist: Some((&s.hist_up, maxv)), frac: None },
    ];
    if show_ip {
        // si la IP completa no cabe legible, se muestra el final
        let fits = c.text_width(&s.ip, 7.5 * sc, 600, "Segoe UI") <= w - 6.0 * sc;
        let ip = if fits || !s.ip.contains('.') { s.ip.clone() } else { format!("…{}", s.ip.splitn(3, '.').nth(2).unwrap_or(&s.ip)) };
        blocks.push(VBlock { label: "IP", value: ip, sub: String::new(), sub_col: DIM, col: GAINSBORO, hist: None, frac: None });
    }
    paint_vblocks(c, w, h, sc, &blocks);
    health_dot(c, w, sc);
    if show_ip {
        // punto de estado con halo, alineado con la etiqueta "IP"
        let bh = h / 3.0;
        let lpx = (8.0 * sc).min((bh * 0.28).floor());
        let online = s.ip.contains('.') || s.ip.contains(':');
        let col = if online { Rgb::u8(60, 230, 120) } else { Rgb::u8(255, 70, 90) };
        c.dot(w - 7.0 * sc, 2.0 * bh + 2.0 * sc + (lpx + 4.0 * sc) / 2.0, 2.4 * sc, col, 1.0, 2.5 * sc, 0.5);
    }
}

fn paint_system_v(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State) {
    let t = |v: i32| if v >= 0 { format!("{v}°") } else { String::new() };
    let fps_max = s.hist_fps.iter().fold(60.0f32, |m, v| m.max(*v)) * 1.15;
    let blocks = [
        VBlock { label: "CPU", value: format!("{:.0}%", s.cpu.max(0.0)), sub: t(s.cpu_t), sub_col: temp_color(s.cpu_t), col: CPU_COL, hist: Some((&s.hist_cpu, 100.0)), frac: None },
        VBlock { label: "GPU", value: format!("{}%", s.gpu.max(0)), sub: t(s.gpu_t), sub_col: temp_color(s.gpu_t), col: GPU_COL, hist: Some((&s.hist_gpu, 100.0)), frac: None },
        VBlock { label: "FPS", value: if s.fps > 0 { s.fps.to_string() } else { "—".into() }, sub: String::new(), sub_col: DIM, col: FPS_COL, hist: Some((&s.hist_fps, fps_max)), frac: None },
        {
            let (used, letter) = system_drive(s).map(|d| ((1.0 - d.free / d.total) as f32, d.letter)).unwrap_or((0.0, 'C'));
            VBlock {
                label: if letter == 'C' { tr("DISCO C:", "DISK C:") } else { tr("DISCO", "DISK") },
                value: format!("{:.0}%", used * 100.0),
                sub: rate_short((s.disk_read + s.disk_write) / 8.0),
                sub_col: DIM,
                col: disk_cols(used).0,
                hist: None,
                frac: Some(used),
            }
        },
    ];
    paint_vblocks(c, w, h, sc, &blocks);
    level_dot(c, w, sc, sys_level(s));
}

/// Distribución vertical del multimedia: carátula cuadrada arriba, botones en fila, título y mini espectro
fn media_layout_v(w: f32, _h: f32, sc: f32) -> MediaLayout {
    let m = 4.0 * sc;
    let art = (w - 2.0 * m).floor();
    let mut pr = (13.0 * sc).min(w * 0.17);
    let mut sr = pr * 0.78;
    let total = 2.0 * pr + 4.0 * sr + 8.0 * sc;
    if total > w - 2.0 * m {
        let k = (w - 2.0 * m) / total;
        pr *= k;
        sr *= k;
    }
    let by = m + art + 6.0 * sc + pr;
    let play = w / 2.0;
    let title_y = by + pr + 5.0 * sc;
    MediaLayout {
        art_x: m,
        art_y: m,
        art,
        cy: by,
        btn: [(play - pr - 4.0 * sc - sr, by, sr), (play, by, pr), (play + pr + 4.0 * sc + sr, by, sr)],
        text_x: m,
        text_w: w - 2.0 * m,
        viz_x: m,
        viz_w: w - 2.0 * m,
        title_y,
        viz_y: title_y + 18.0 * sc,
        viz_h: 16.0 * sc,
    }
}

fn media_v_height(w: f32, sc: f32) -> f32 {
    let l = media_layout_v(w, 0.0, sc);
    l.viz_y + l.viz_h + 4.0 * sc
}

#[allow(clippy::too_many_arguments)]
fn paint_media_v(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, p: &mut Panel, style: &str, t: f32) {
    let l = media_layout_v(w, h, sc);
    let dt = (t - p.last_t).clamp(0.0, 0.1);
    p.last_t = t;
    if s.playing {
        p.spin = (p.spin + dt * 3.5) % std::f32::consts::TAU;
    }
    let art = media::ART.lock().unwrap().clone();
    let accent = art.as_ref().map(|a| Rgb::u8(a.accent.0, a.accent.1, a.accent.2)).unwrap_or(Rgb::u8(0, 230, 255));
    match art.as_ref().filter(|a| !a.px.is_empty()) {
        Some(a) => c.image_rounded(l.art_x, l.art_y, l.art, (6.0 * sc).round(), &a.px, media::ART_SIZE, media::ART_SIZE, 1.0),
        None => c.vinyl(l.art_x + l.art / 2.0, l.art_y + l.art / 2.0, l.art / 2.0, p.spin, accent, 1.0),
    }
    for (i, (bx, by, br)) in l.btn.iter().enumerate() {
        let hover = p.hover == i as i32;
        if i == 1 {
            let fill = if hover { accent.mix(WHITE, 0.15) } else { accent };
            c.dot(*bx, *by, *br, fill, 1.0, 3.0 * sc, if hover { 0.55 } else { 0.25 });
            let ink = if luminance(fill) > 150.0 { Rgb::u8(16, 16, 22) } else { WHITE };
            if s.playing {
                let (bw, bh) = (br * 0.22, br * 0.78);
                for dx in [-br * 0.22, br * 0.22] {
                    c.capsule(bx + dx, by - bh / 2.0, by + bh / 2.0, bw / 2.0, ink, 1.0, 0.0, 0.0, 0.0);
                }
            } else {
                c.text("\u{F5B0}", bx - br + 1.0 * sc, by - br, br * 2.0, br * 2.0, (br * 0.95).floor(), 400, "Segoe Fluent Icons", ink, DT_CENTER);
            }
        } else {
            if hover {
                c.dot(*bx, *by, *br, WHITE, 0.10, 0.0, 0.0);
            }
            let col = if hover { WHITE } else { Rgb::u8(190, 192, 205) };
            let g = if i == 0 { "\u{E892}" } else { "\u{E893}" };
            c.text(g, bx - br, by - br, br * 2.0, br * 2.0, (br * 0.86).floor(), 400, "Segoe Fluent Icons", col, DT_CENTER);
        }
    }
    // título desplazable
    let tpx = (10.0 * sc).floor();
    let tw = c.text_width(&s.title, tpx, 600, "Segoe UI");
    if tw <= l.text_w {
        c.text(&s.title, l.text_x, l.title_y, l.text_w, 16.0 * sc, tpx, 600, "Segoe UI", WHITE, DT_CENTER);
    } else {
        let (speed, gap, wait) = (22.0 * sc, 30.0 * sc, 2.0);
        let ph = t % ((tw + gap) / speed + wait);
        let off = if ph < wait { 0.0 } else { (ph - wait) * speed };
        let (c0, c1, fade) = (l.text_x, l.text_x + l.text_w, 6.0 * sc);
        c.text_clip(&s.title, l.text_x - off, l.title_y, 16.0 * sc, tpx, 600, "Segoe UI", WHITE, c0, c1, fade);
        c.text_clip(&s.title, l.text_x - off + tw + gap, l.title_y, 16.0 * sc, tpx, 600, "Segoe UI", WHITE, c0, c1, fade);
    }
    // mini espectro
    let target = media::LEVELS.lock().unwrap().clone();
    if p.disp.len() != target.len() {
        p.disp = vec![0.0; target.len()];
        p.caps = vec![0.0; target.len()];
    }
    for i in 0..target.len() {
        // constantes pensadas para 60 fps, ajustadas al tiempo real entre fotogramas
        let k: f32 = if target[i] > p.disp[i] { 0.45 } else { 0.12 };
        p.disp[i] += (target[i] - p.disp[i]) * (1.0 - (1.0 - k).powf(dt * 60.0));
        p.caps[i] = (p.caps[i] - 0.006 * dt * 60.0).max(p.disp[i]);
    }
    let pulse = (p.disp.iter().take(6).sum::<f32>() / 6.0).clamp(0.0, 1.0);
    let (d, cp) = (p.disp.clone(), p.caps.clone());
    spectrum(c, style, l.viz_x, l.viz_y, l.viz_w, l.viz_h, sc, &d, &cp, (t / 14.0).rem_euclid(1.0), pulse);
}

/// Apila los paneles en una barra vertical: red/consumo/sistema hacia arriba desde la bandeja,
/// multimedia bajo los iconos de aplicaciones
unsafe fn place_vertical(panels: &mut [Panel], b: &Taskbar, cfg: &Config, hide: bool, show_ip: bool, usage: usize, media_active: bool) {
    let sc = b.scale;
    let m = (4.0 * sc) as i32;
    let x = b.rect.left + m;
    let w = (b.rect.right - b.rect.left) - 2 * m;
    let mut bottom = b.tray_top - (8.0 * sc) as i32;
    for k in CHAIN {
        let Some(p) = panels.iter_mut().find(|p| p.kind == k && p.tb_key == b.key) else { continue };
        p.scale = sc;
        p.vertical = true;
        // en barras verticales, icono por defecto; el diseño vertical detallado solo si se eligió "Panel"
        p.compact = cfg.v["PanelStyle"][k.name()].as_str() != Some("full");
        if hide || w < 20 {
            let _ = ShowWindow(p.hwnd, SW_HIDE);
            continue;
        }
        let h = if p.compact { (w as f32).min(48.0 * sc) as i32 } else { (v_height(k, show_ip, usage) * sc) as i32 };
        let y = bottom - h;
        bottom = y - (8.0 * sc) as i32;
        place_panel(p, x, y, w, h);
    }
    if let Some(p) = panels.iter_mut().find(|p| p.kind == Kind::Media && p.tb_key == b.key) {
        p.scale = sc;
        p.vertical = true;
        let need = media_v_height(w as f32, sc) as i32;
        // al inicio (arriba) de la barra si cabe antes de los iconos; si no, bajo los iconos
        let at_start = b.rect.top + (8.0 * sc) as i32;
        let top = if at_start + need + (10.0 * sc) as i32 <= b.tasks_top { at_start } else { b.tasks_bottom + (10.0 * sc) as i32 };
        if hide || !media_active || w < 20 || bottom - top < need {
            let _ = ShowWindow(p.hwnd, SW_HIDE); // sin reproducción o sin espacio entre los iconos y los paneles
        } else {
            place_panel(p, x, top, w, need);
        }
    }
}

/// Dibuja el panel en su lienzo y lo publica con alfa por píxel
unsafe fn render(p: &mut Panel, s: &state::State, style: &str, view: &AiView, show_ip: bool, t: f32) {
    if !IsWindowVisible(p.hwnd).as_bool() {
        p.surf = None; // oculto (p. ej. multimedia sin reproducción): no retener su lienzo
        return;
    }
    let mut rc = RECT::default();
    let _ = GetClientRect(p.hwnd, &mut rc);
    let (w, h) = (rc.right, rc.bottom);
    if w <= 0 || h <= 0 {
        return;
    }
    if p.surf.as_ref().map(|s| s.w != w || s.h != h).unwrap_or(true) {
        p.surf = Surface::new(w, h);
    }
    let Some(mut surf) = p.surf.take() else { return };
    surf.clear();
    let (wf, hf, sc) = (w as f32, h as f32, p.scale);
    match (p.kind, p.vertical) {
        // modo icono (barra horizontal sin espacio o barra vertical por defecto)
        (Kind::Usage, _) if p.compact => paint_ai_compact(&mut surf, wf, hf, sc, view),
        (Kind::System, _) if p.compact => paint_system_compact(&mut surf, wf, hf, sc, s),
        (Kind::Ip, _) if p.compact => paint_ip_compact(&mut surf, wf, hf, sc, s),
        (Kind::Ip, false) => paint_ip(&mut surf, wf, hf, sc, s, t, show_ip),
        (Kind::Usage, false) => paint_ai(&mut surf, wf, hf, sc, view),
        (Kind::System, false) => paint_system(&mut surf, wf, hf, sc, s),
        (Kind::Media, false) => paint_media(&mut surf, wf, hf, sc, s, p, style, t),
        (Kind::Ip, true) => paint_ip_v(&mut surf, wf, hf, sc, s, show_ip),
        (Kind::Usage, true) => paint_ai_v(&mut surf, wf, hf, sc, view),
        (Kind::System, true) => paint_system_v(&mut surf, wf, hf, sc, s),
        (Kind::Media, true) => paint_media_v(&mut surf, wf, hf, sc, s, p, style, t),
        (Kind::Radio, v) if p.compact || v => paint_radio_compact(&mut surf, wf, hf, sc, &radio::now(), t),
        (Kind::Radio, _) => paint_radio(&mut surf, wf, hf, sc, &radio::now(), t),
    }
    surf.present(p.hwnd);
    p.surf = Some(surf);
}

unsafe fn render_kind(media_only: bool) {
    // se llama 60 veces por segundo para el multimedia: sin ningún panel multimedia visible no hay
    // nada que animar (ni estado que copiar); solo se sueltan sus lienzos
    if media_only {
        let visible = with_app(|app| {
            let mut any = false;
            for p in app.panels.iter_mut().filter(|p| p.kind == Kind::Media) {
                if IsWindowVisible(p.hwnd).as_bool() {
                    any = true;
                } else {
                    p.surf = None;
                }
            }
            any
        });
        if visible != Some(true) {
            return;
        }
    }
    let s = st().clone();
    with_app(|app| {
        let style = spectrum_style(&app.cfg);
        let view = AiView::from_cfg(&app.cfg);
        AI_POPUP_H.store(ai_popup_h(&view).to_bits(), std::sync::atomic::Ordering::Relaxed);
        let show_ip = app.cfg.v["IpShowAddress"].as_bool().unwrap_or(true);
        let t = app.t0.elapsed().as_secs_f32();
        let lights = app.bar_light.clone();
        for p in app.panels.iter_mut() {
            if (p.kind == Kind::Media) == media_only {
                render::set_light(lights.get(&p.tb_key).copied().unwrap_or_else(render::system_light));
                render(p, &s, style, &view, show_ip, t);
            }
        }
    });
}

/// Redibuja solo los paneles de radio (banner en movimiento). `paint_radio` vuelve a activar
/// `RADIO_BANNER` mientras algún nombre siga sin caber.
unsafe fn render_radio() {
    RADIO_BANNER.store(false, std::sync::atomic::Ordering::Relaxed);
    let n = radio::now();
    with_app(|app| {
        let t = app.t0.elapsed().as_secs_f32();
        let lights = app.bar_light.clone();
        for p in app.panels.iter_mut().filter(|p| p.kind == Kind::Radio && !p.compact && !p.vertical) {
            if !IsWindowVisible(p.hwnd).as_bool() {
                continue;
            }
            let mut rc = RECT::default();
            let _ = GetClientRect(p.hwnd, &mut rc);
            let Some(mut surf) = p.surf.take().filter(|s| s.w == rc.right && s.h == rc.bottom) else { continue };
            render::set_light(lights.get(&p.tb_key).copied().unwrap_or_else(render::system_light));
            surf.clear();
            paint_radio(&mut surf, rc.right as f32, rc.bottom as f32, p.scale, &n, t);
            surf.present(p.hwnd);
            p.surf = Some(surf);
        }
    });
}

// ======================================================================= sincronización de ventanas
/// Crea el panel como ventana hija de la barra de tareas: así forma parte de la barra y sigue visible
/// cuando Windows la sube de capa (menú Inicio abierto, clics en la barra). Si Windows no lo permite,
/// se crea como ventana flotante "siempre encima" (modo anterior).
unsafe fn create_panel(app: &mut App, kind: Kind, tb: &Taskbar) {
    let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
    let child = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_NOACTIVATE,
        CLASS,
        w!(""),
        WS_CHILD | WS_CLIPSIBLINGS,
        0, 0, 10, 10,
        tb.hwnd,
        HMENU::default(),
        hinst,
        None,
    );
    let (hwnd, parent) = match child {
        Ok(h) => (h, tb.hwnd),
        Err(e) => {
            dbg(&format!("panel {kind:?}: no se pudo crear como hija de la barra ({e}); ventana flotante"));
            let Ok(h) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                CLASS,
                w!(""),
                WS_POPUP,
                0, 0, 10, 10,
                HWND::default(),
                HMENU::default(),
                hinst,
                None,
            ) else {
                return;
            };
            (h, HWND::default())
        }
    };
    app.panels.push(Panel { hwnd, parent, kind, tb_key: tb.key, scale: tb.scale, caps: Vec::new(), disp: Vec::new(), hover: -1, surf: None, vertical: false, compact: false, vinyl: 0.0, spin: 0.0, last_t: 0.0 });
}

/// Coloca y muestra un panel en coordenadas de pantalla: si es hija de la barra, se convierten a
/// coordenadas de la barra y se pone por encima del contenido de la barra (XAML)
unsafe fn place_panel(p: &Panel, x: i32, y: i32, w: i32, h: i32) {
    if p.parent.is_invalid() {
        let _ = SetWindowPos(p.hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    } else {
        let mut pt = POINT { x, y };
        let _ = ScreenToClient(p.parent, &mut pt);
        let _ = SetWindowPos(p.hwnd, HWND_TOP, pt.x, pt.y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

fn monitors_for_config(bars: &[Taskbar], cfg: &Config) -> Vec<(String, String, bool)> {
    bars.iter().map(|b| (b.monitor_id.clone(), cfg.monitor_name(&b.monitor_id), b.primary)).collect()
}

unsafe fn sync(app: &mut App) {
    app.cfg.refresh();
    app.bars = host::taskbars(app.cfg.margin());
    // Ventanas deseadas: (tipo, barra) habilitadas y seleccionadas
    let mut wanted: Vec<(Kind, usize)> = Vec::new();
    for k in KINDS {
        if !app.cfg.enabled(k.name()) {
            continue;
        }
        for (i, b) in app.bars.iter().enumerate() {
            if app.cfg.selected(k.name(), b.primary, &b.monitor_id) {
                wanted.push((k, i));
            }
        }
    }
    // Cerrar las sobrantes (y olvidar las que Windows destruyó junto con su barra, p. ej. al
    // reiniciarse Explorer: se recrean abajo en la barra nueva)
    let bars = app.bars.clone();
    app.panels.retain(|p| {
        let alive = IsWindow(p.hwnd).as_bool() && (p.parent.is_invalid() || IsWindow(p.parent).as_bool());
        let keep = alive && wanted.iter().any(|(k, i)| *k == p.kind && bars[*i].key == p.tb_key && (p.parent.is_invalid() || bars[*i].hwnd == p.parent));
        if !keep {
            let _ = DestroyWindow(p.hwnd);
        }
        keep
    });
    // Crear las nuevas
    for (k, i) in &wanted {
        if !app.panels.iter().any(|p| p.kind == *k && p.tb_key == bars[*i].key) {
            create_panel(app, *k, &bars[*i]);
        }
    }
    // Posicionar: a la derecha en cadena (IP ← Consumo ← Sistema), multimedia a la izquierda
    let show_ip = app.cfg.v["IpShowAddress"].as_bool().unwrap_or(true);
    let usage = AiView::from_cfg(&app.cfg).shown().len();
    // Multimedia visible mientras haya una sesión (reproduciendo o en pausa); 3 s de gracia
    if { let s = st(); s.playing || !s.media_app.is_empty() } {
        app.last_playing = Some(Instant::now());
    }
    let media_active = app.last_playing.map(|t| t.elapsed().as_secs_f32() < 3.0).unwrap_or(false);
    for b in &bars {
        // La condición de ocultar debe mantenerse ~0,8 s: evita parpadeos por ventanas pasajeras
        let reason = host::hide_reason(b);
        let hide = match &reason {
            Some(_) => app.hide_since.entry(b.key).or_insert_with(Instant::now).elapsed().as_millis() >= 800,
            None => {
                app.hide_since.remove(&b.key);
                false
            }
        };
        if hide != app.hidden.contains(&b.key) {
            let kind = if b.primary { "principal" } else { "secundaria" };
            match (&reason, hide) {
                (Some(r), true) => dbg(&format!("barra {} ({kind}): OCULTAR — {r}", b.monitor_id)),
                _ => dbg(&format!("barra {} ({kind}): MOSTRAR", b.monitor_id)),
            }
            if hide { app.hidden.insert(b.key); } else { app.hidden.remove(&b.key); }
        }
        if b.vertical {
            place_vertical(&mut app.panels, b, &app.cfg, hide, show_ip, usage, media_active);
            continue;
        }
        // ¿Choca la cadena de paneles de la derecha con los iconos de apps? → los automáticos en modo icono
        // (formato elegido por panel: "icon" fijo, "full" fijo, "auto" = icono solo si no cabe)
        let present = |k: Kind| app.panels.iter().any(|p| p.kind == k && p.tb_key == b.key);
        let modes: Vec<(Kind, &str)> = CHAIN.iter().map(|k| (*k, panel_mode(&app.cfg, *k))).collect();
        let mode = |k: Kind| modes.iter().find(|m| m.0 == k).map(|m| m.1).unwrap_or("full");
        let px = |w: f32| (w * b.scale) as i32;
        let full: Vec<(Kind, i32, i32, bool)> = CHAIN
            .iter()
            .filter(|k| present(**k))
            .map(|k| (*k, px(if mode(*k) == "icon" { compact_width(*k) } else { k.width() }), px(compact_width(*k)), mode(*k) == "auto"))
            .collect();
        // Con los iconos a la izquierda el multimedia va entre los iconos y estos paneles: en modo
        // automático se reserva su ancho completo (si no, en pantallas pequeñas quedaría comprimido o
        // no cabría y sistema/IA seguirían como panel completo)
        let media_need = if b.left_aligned && media_active && present(Kind::Media) { (Kind::Media.width() + 12.0) * b.scale } else { 0.0 };
        // Zona de los iconos de apps (con su margen): con los iconos centrados, a la izquierda de
        // Inicio también hay sitio libre y un panel arrastrado hasta allí conserva su formato
        let zone_l = if b.left_aligned { i32::MIN } else { b.start_left - px(16.0) };
        let zone = (zone_l, (b.tasks_right as f32 + 16.0 * b.scale + media_need) as i32);
        let compact = auto_compact(b, &full, &app.offs, zone);
        // Espacio del multimedia: los paneles de la derecha no pueden arrastrarse encima de él.
        // Iconos centrados → ocupa el lado izquierdo de la barra; iconos a la izquierda → va tras los
        // iconos y se le reserva al menos su ancho mínimo (si no cabe, se oculta como antes)
        let media_left = if media_active && present(Kind::Media) && !hide {
            if b.left_aligned {
                b.tasks_right + ((16.0 + MEDIA_MIN_W + 12.0) * b.scale) as i32
            } else {
                media_place(b, i32::MAX).map(|(x, w)| x + w + (14.0 * b.scale) as i32).unwrap_or(i32::MIN)
            }
        } else {
            i32::MIN
        };
        // Formato y ancho definitivos de cada panel de la derecha
        let mut chain: Vec<(Kind, i32)> = Vec::new();
        for k in CHAIN {
            let Some(p) = app.panels.iter_mut().find(|p| p.kind == k && p.tb_key == b.key) else { continue };
            let was = p.compact;
            p.compact = match mode(k) {
                "icon" => true,
                "auto" => compact.contains(&k),
                _ => false,
            };
            if was != p.compact {
                dbg(&format!(
                    "barra {}: {k:?} modo icono = {} (zona de iconos x={}..{}, alineada izq={})",
                    b.monitor_id, p.compact, zone.0, zone.1, b.left_aligned
                ));
            }
            chain.push((k, ((if p.compact { compact_width(k) } else { k.width() }) * b.scale) as i32));
        }
        let mut placed = chain_layout(b, &chain, &app.offs, media_left);
        if placed.iter().any(|c| c.1 < media_left) {
            // no caben junto al multimedia: los paneles mantienen su sitio y el multimedia se oculta
            placed = chain_layout(b, &chain, &app.offs, i32::MIN);
        }
        let chain_left = placed.iter().map(|c| c.1).min().unwrap_or(b.right);
        for k in KINDS {
            let Some(p) = app.panels.iter_mut().find(|p| p.kind == k && p.tb_key == b.key) else { continue };
            p.scale = b.scale;
            p.vertical = false;
            if hide {
                let _ = ShowWindow(p.hwnd, SW_HIDE);
                continue;
            }
            if k == Kind::Media && !media_active {
                let _ = ShowWindow(p.hwnd, SW_HIDE); // sin sesión multimedia
                continue;
            }
            let (x, w) = if k == Kind::Media {
                // Iconos a la izquierda → a la derecha de los iconos, hasta los paneles de la derecha
                match media_place(b, chain_left) {
                    Some(xw) => xw,
                    None => {
                        let _ = ShowWindow(p.hwnd, SW_HIDE); // no hay espacio suficiente
                        continue;
                    }
                }
            } else {
                let Some(&(_, x, w)) = placed.iter().find(|c| c.0 == k) else { continue };
                (x, w)
            };
            place_panel(p, x, b.top, w, b.height);
        }
    }
}

// ======================================================================= red
fn net_bytes() -> (u64, u64) {
    unsafe {
        let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut t).is_err() || t.is_null() {
            return (0, 0);
        }
        let n = (*t).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), n);
        let (mut rx, mut tx) = (0u64, 0u64);
        for r in rows {
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let hardware = flags & 0x1 != 0;
            let filter = flags & 0x2 != 0;
            if hardware && !filter && r.OperStatus.0 == 1 && r.Type != 24 && r.Type != 131 {
                rx += r.InOctets;
                tx += r.OutOctets;
            }
        }
        FreeMibTable(t as *const _);
        (rx, tx)
    }
}

fn fmt_rate(bytes_per_s: f64) -> String {
    let b = bytes_per_s * 8.0;
    if b >= 1e9 {
        format!("{:.2} Gb/s", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} Mb/s", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} Kb/s", b / 1e3)
    } else {
        format!("{b:.0} b/s")
    }
}

// ======================================================================= bandeja y menú
unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

fn copy_into(dst: &mut [u16], s: &str) {
    let v: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..v.len()].copy_from_slice(&v);
    dst[v.len()] = 0;
}

unsafe fn tray_add(hwnd: HWND) {
    let mut d = tray_data(hwnd);
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    d.uCallbackMessage = WM_TRAY;
    d.hIcon = LoadIconW(HINSTANCE::default(), IDI_INFORMATION).unwrap_or_default();
    copy_into(&mut d.szTip, tr("Paneles de la barra de tareas", "Taskbar panels"));
    let _ = Shell_NotifyIconW(NIM_ADD, &d);
}

unsafe fn tray_tip(hwnd: HWND, tip: &str) {
    let mut d = tray_data(hwnd);
    d.uFlags = NIF_TIP;
    copy_into(&mut d.szTip, tip);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
}

unsafe fn tray_balloon(hwnd: HWND, title: &str, text: &str) {
    let mut d = tray_data(hwnd);
    d.uFlags = NIF_INFO;
    copy_into(&mut d.szInfoTitle, title);
    copy_into(&mut d.szInfo, text);
    d.dwInfoFlags = NIIF_INFO;
    let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
}

unsafe fn copy_to_clipboard(owner: HWND, s: &str) {
    if OpenClipboard(owner).is_err() {
        return;
    }
    let _ = EmptyClipboard();
    let v = wide(s);
    if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, v.len() * 2) {
        let p = GlobalLock(h) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(v.as_ptr(), p, v.len());
            let _ = GlobalUnlock(h);
            let _ = SetClipboardData(13, HANDLE(h.0)); // CF_UNICODETEXT
        }
    }
    let _ = CloseClipboard();
}

const ID_COPY_IP: usize = 1;
const ID_REFRESH_IP: usize = 2;
const ID_REFRESH_USAGE: usize = 3;
const ID_OPEN_CLAUDE: usize = 4;
const ID_OPEN_COPILOT: usize = 5;
const ID_RESET_POS: usize = 6;
const ID_PANEL_BASE: usize = 10; // +índice de KINDS
const ID_SCREENS_ALL: usize = 20;
const ID_SCREENS_PRIMARY: usize = 21;
const ID_SCREEN_BASE: usize = 30; // +índice de barra
const ID_MEDIA_W_BASE: usize = 50; // +índice de MEDIA_SIZES (50..=54)
const ID_STYLE_BASE: usize = 40; // +índice de SPECTRUM_STYLES (40..=44)
const ID_AI_PRIMARY_ALL: usize = 100;
const ID_AI_PRIMARY_BASE: usize = 101; // +índice de proveedor (hasta 29)
const ID_AI_TOGGLE_BASE: usize = 130;
const ID_AI_ADD_OPENAI: usize = 160;
const ID_AI_ADD_ANTHROPIC: usize = 161;
const ID_AI_ADD_CUSTOM: usize = 162;
const ID_AI_COPILOT_TOKEN: usize = 164;
const ID_AI_ADD_DEEPSEEK: usize = 165;
const ID_AI_ADD_KIMI: usize = 166;
const ID_AI_ADD_OLLAMA: usize = 167;
const ID_AI_ADD_OLLAMA_CLOUD: usize = 168;
const ID_AI_ADD_LMSTUDIO: usize = 169;
const ID_AI_ADD_GEMINI: usize = 163;
const ID_AI_REMOVE_BASE: usize = 170;
const ID_SHOW_IP: usize = 60;
const ID_NET_HEALTH: usize = 61;
const ID_POPUP_STYLE_BASE: usize = 80; // +índice de POPUP_STYLES
const ID_LANG_AUTO: usize = 90;
const ID_LANG_ES: usize = 91;
const ID_LANG_EN: usize = 92;
// formato de los paneles: sistema (auto/panel/icono) y red (panel/icono)
const ID_FMT_SYS_AUTO: usize = 110;
const ID_FMT_SYS_FULL: usize = 111;
const ID_FMT_SYS_ICON: usize = 112;
const ID_FMT_NET_FULL: usize = 113;
const ID_FMT_NET_ICON: usize = 114;
const ID_FMT_AI_AUTO: usize = 115;
const ID_FMT_AI_FULL: usize = 116;
const ID_FMT_AI_ICON: usize = 117;
const ID_SPEED_BASE: usize = 93; // +índice de speedtest::PROVIDERS (93..=96)
const ID_EXIT: usize = 99;
const ID_FMT_RADIO_AUTO: usize = 210;
const ID_FMT_RADIO_FULL: usize = 211;
const ID_FMT_RADIO_ICON: usize = 212;
const ID_FMT_NET_AUTO: usize = 213;

/// Agrega un submenú con texto traducido
unsafe fn append_sub(menu: HMENU, sub: HMENU, text: &str) {
    let t = wide(text);
    let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(t.as_ptr()));
}

unsafe fn append(menu: HMENU, id: usize, text: &str, checked: bool) {
    let t = wide(text);
    let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(menu, flags, id, PCWSTR(t.as_ptr()));
}

/// Construye y muestra el menú (fuera del préstamo de APP: TrackPopupMenu es modal)
unsafe fn show_menu(main: HWND) {
    let Some((cfg_sel, enabled, monitors)) = with_app(|app| {
        let sel = app.cfg.v["Displays"].clone();
        let enabled: Vec<bool> = KINDS.iter().map(|k| app.cfg.enabled(k.name())).collect();
        let mons: Vec<(String, String, bool)> = monitors_for_config(&app.bars, &app.cfg);
        (sel, enabled, mons)
    }) else { return };
    let selected_ids: Vec<String> = match &cfg_sel {
        serde_json::Value::String(s) if s == "all" => monitors.iter().map(|m| m.0.clone()).collect(),
        serde_json::Value::String(s) if s == "primary" => monitors.iter().filter(|m| m.2).map(|m| m.0.clone()).collect(),
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
        _ => vec![],
    };
    let (ai_list, ai_primary) = with_app(|a| (ai::providers(&a.cfg), ai::primary(&a.cfg))).unwrap_or_default();
    let Ok(menu) = CreatePopupMenu() else { return };
    let show_ip = with_app(|a| a.cfg.v["IpShowAddress"].as_bool().unwrap_or(true)).unwrap_or(true);
    append(menu, ID_SHOW_IP, tr("Mostrar IP pública", "Show public IP"), show_ip);
    append(menu, ID_NET_HEALTH, tr("Salud de la conexión", "Connection health"), net::health_on());
    append(menu, ID_COPY_IP, tr("Copiar IP", "Copy IP"), false);
    append(menu, ID_REFRESH_IP, tr("Actualizar IP", "Refresh IP"), false);
    append(menu, ID_REFRESH_USAGE, tr("Actualizar consumo", "Refresh usage"), false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    append(menu, ID_OPEN_CLAUDE, tr("Abrir uso de Claude", "Open Claude usage"), false);
    append(menu, ID_OPEN_COPILOT, tr("Abrir uso de Copilot", "Open Copilot usage"), false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    if let Ok(sub) = CreatePopupMenu() {
        for (i, k) in KINDS.iter().enumerate() {
            append(sub, ID_PANEL_BASE + i, k.label(), enabled[i]);
        }
        append_sub(menu, sub, tr("Paneles", "Panels"));
    }
    if let Ok(sub) = CreatePopupMenu() {
        append(sub, ID_SCREENS_ALL, tr("Todas", "All"), cfg_sel.as_str() == Some("all"));
        append(sub, ID_SCREENS_PRIMARY, tr("Solo la principal", "Primary only"), cfg_sel.as_str() == Some("primary"));
        let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
        for (i, (id, name, primary)) in monitors.iter().enumerate() {
            let label = format!("{name}  ({id}){}", if *primary { tr(" · principal", " · primary") } else { "" });
            append(sub, ID_SCREEN_BASE + i, &label, selected_ids.contains(id));
        }
        append_sub(menu, sub, tr("Pantallas", "Displays"));
    }
    let (sys_mode, net_mode, ai_mode) =
        with_app(|a| (panel_mode(&a.cfg, Kind::System), panel_mode(&a.cfg, Kind::Ip), panel_mode(&a.cfg, Kind::Usage))).unwrap_or(("auto", "auto", "auto"));
    if let Ok(sub) = CreatePopupMenu() {
        append(sub, ID_FMT_AI_AUTO, tr("IA: automático (icono si no cabe)", "AI: automatic (icon if it does not fit)"), ai_mode == "auto");
        append(sub, ID_FMT_AI_FULL, tr("IA: panel", "AI: panel"), ai_mode == "full");
        append(sub, ID_FMT_AI_ICON, tr("IA: icono", "AI: icon"), ai_mode == "icon");
        let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
        append(sub, ID_FMT_SYS_AUTO, tr("Sistema: automático (icono si no cabe)", "System: automatic (icon if it does not fit)"), sys_mode == "auto");
        append(sub, ID_FMT_SYS_FULL, tr("Sistema: panel", "System: panel"), sys_mode == "full");
        append(sub, ID_FMT_SYS_ICON, tr("Sistema: icono", "System: icon"), sys_mode == "icon");
        let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
        append(sub, ID_FMT_NET_AUTO, tr("Red: automático (icono si no cabe)", "Network: automatic (icon if it does not fit)"), net_mode == "auto");
        append(sub, ID_FMT_NET_FULL, tr("Red: panel", "Network: panel"), net_mode == "full");
        append(sub, ID_FMT_NET_ICON, tr("Red: icono", "Network: icon"), net_mode == "icon");
        let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
        let radio_mode = with_app(|a| panel_mode(&a.cfg, Kind::Radio)).unwrap_or("auto");
        append(sub, ID_FMT_RADIO_AUTO, tr("Radio: automático (icono si no cabe)", "Radio: automatic (icon if it does not fit)"), radio_mode == "auto");
        append(sub, ID_FMT_RADIO_FULL, tr("Radio: panel", "Radio: panel"), radio_mode == "full");
        append(sub, ID_FMT_RADIO_ICON, tr("Radio: icono (nota roja)", "Radio: icon (red note)"), radio_mode == "icon");
        append_sub(menu, sub, tr("Formato de los paneles", "Panel format"));
    }
    let cur_style = with_app(|a| spectrum_style(&a.cfg)).unwrap_or("wave");
    if let Ok(sub) = CreatePopupMenu() {
        for (i, (k, es, en)) in SPECTRUM_STYLES.iter().enumerate() {
            append(sub, ID_STYLE_BASE + i, tr(es, en), cur_style == *k);
        }
        append_sub(menu, sub, tr("Estilo del espectro", "Spectrum style"));
    }
    if let Ok(sub) = CreatePopupMenu() {
        let cur = media_w();
        for (i, (w, es, en)) in MEDIA_SIZES.iter().enumerate() {
            append(sub, ID_MEDIA_W_BASE + i, tr(es, en), (cur - w).abs() < 0.5);
        }
        append_sub(menu, sub, tr("Tamaño del panel multimedia", "Media panel size"));
    }
    // IA: proveedor principal, activar/desactivar, agregar y quitar
    if let Ok(ai_menu) = CreatePopupMenu() {
        if let Ok(sub) = CreatePopupMenu() {
            append(sub, ID_AI_PRIMARY_ALL, tr("Todos (resumen)", "All (summary)"), ai_primary == "all");
            let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
            for (i, p) in ai_list.iter().enumerate().filter(|(_, p)| p.enabled) {
                append(sub, ID_AI_PRIMARY_BASE + i, &p.name, ai_primary == p.id);
            }
            append_sub(ai_menu, sub, tr("Proveedor principal", "Primary provider"));
        }
        if let Ok(sub) = CreatePopupMenu() {
            for (i, p) in ai_list.iter().enumerate() {
                append(sub, ID_AI_TOGGLE_BASE + i, &p.name, p.enabled);
            }
            append_sub(ai_menu, sub, tr("Proveedores", "Providers"));
        }
        if let Ok(sub) = CreatePopupMenu() {
            append(sub, ID_AI_ADD_OPENAI, tr("API de OpenAI…", "OpenAI API…"), false);
            append(sub, ID_AI_ADD_ANTHROPIC, tr("API de Anthropic…", "Anthropic API…"), false);
            append(sub, ID_AI_ADD_GEMINI, "Gemini…", false);
            append(sub, ID_AI_ADD_DEEPSEEK, "DeepSeek…", false);
            append(sub, ID_AI_ADD_KIMI, "Kimi (Moonshot AI)…", false);
            append(sub, ID_AI_ADD_OLLAMA, "Ollama (local)…", false);
            append(sub, ID_AI_ADD_OLLAMA_CLOUD, "Ollama Cloud…", false);
            append(sub, ID_AI_ADD_LMSTUDIO, "LM Studio…", false);
            append(sub, ID_AI_ADD_CUSTOM, tr("Personalizado (URL JSON)…", "Custom (JSON URL)…"), false);
            append_sub(ai_menu, sub, tr("Agregar proveedor", "Add provider"));
        }
        append(ai_menu, ID_AI_COPILOT_TOKEN, tr("Token de GitHub para Copilot…", "GitHub token for Copilot…"), false);
        let removable: Vec<(usize, &ai::ProviderCfg)> = ai_list.iter().enumerate().filter(|(_, p)| !matches!(p.kind.as_str(), "claude" | "copilot")).collect();
        if !removable.is_empty() {
            if let Ok(sub) = CreatePopupMenu() {
                for (i, p) in removable {
                    append(sub, ID_AI_REMOVE_BASE + i, &p.name, false);
                }
                append_sub(ai_menu, sub, tr("Quitar proveedor", "Remove provider"));
            }
        }
        append_sub(menu, ai_menu, tr("IA", "AI"));
    }
    let pstyle = with_app(|a| popup_style(&a.cfg)).unwrap_or_else(|| "glass".into());
    if let Ok(sub) = CreatePopupMenu() {
        for (i, (k, label)) in POPUP_STYLES.iter().enumerate() {
            let l = label.split('|').nth(if i18n::es() { 0 } else { 1 }).unwrap_or(label);
            append(sub, ID_POPUP_STYLE_BASE + i, l, pstyle == *k);
        }
        append_sub(menu, sub, tr("Fondo de las ventanas", "Window background"));
    }
    // Speed test: proveedor
    if let Ok(sub) = CreatePopupMenu() {
        let cur = speedtest::selected();
        for (i, (k, name)) in speedtest::PROVIDERS.iter().enumerate() {
            append(sub, ID_SPEED_BASE + i, name, cur == *k);
        }
        append_sub(menu, sub, tr("Proveedor del speed test", "Speed test provider"));
    }
    // Idioma: automático (sistema), español o inglés
    let lang = with_app(|a| a.cfg.v["Language"].as_str().unwrap_or("auto").to_string()).unwrap_or_else(|| "auto".into());
    if let Ok(sub) = CreatePopupMenu() {
        append(sub, ID_LANG_AUTO, tr("Automático (idioma del sistema)", "Automatic (system language)"), lang != "es" && lang != "en");
        append(sub, ID_LANG_ES, "Español", lang == "es");
        append(sub, ID_LANG_EN, "English", lang == "en");
        append_sub(menu, sub, tr("Idioma", "Language"));
    }
    append(menu, ID_RESET_POS, tr("Reiniciar posición", "Reset position"), false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    append(menu, ID_EXIT, tr("Salir", "Exit"), false);

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(main);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, main, None).0 as usize;
    let _ = PostMessageW(main, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    if cmd != 0 {
        on_command(main, cmd, &monitors, &selected_ids, &ai_list);
    }
}

unsafe fn on_command(main: HWND, cmd: usize, monitors: &[(String, String, bool)], selected: &[String], ai_list: &[ai::ProviderCfg]) {
    match cmd {
        ID_COPY_IP => {
            let ip = st().ip.clone();
            copy_to_clipboard(main, &ip);
        }
        ID_REFRESH_IP => state::refresh_ip(),
        ID_REFRESH_USAGE => ai::refresh(),
        ID_AI_ADD_OPENAI => ai::add_provider_async("openai"),
        ID_AI_ADD_ANTHROPIC => ai::add_provider_async("anthropic_api"),
        ID_AI_ADD_CUSTOM => ai::add_provider_async("custom"),
        ID_AI_ADD_GEMINI => ai::add_provider_async("gemini"),
        ID_AI_ADD_DEEPSEEK => ai::add_provider_async("deepseek"),
        ID_AI_ADD_KIMI => ai::add_provider_async("kimi"),
        ID_AI_ADD_OLLAMA => ai::add_provider_async("ollama"),
        ID_AI_ADD_OLLAMA_CLOUD => ai::add_provider_async("ollama_cloud"),
        ID_AI_ADD_LMSTUDIO => ai::add_provider_async("lmstudio"),
        ID_AI_COPILOT_TOKEN => ai::set_copilot_token_async(),
        ID_OPEN_CLAUDE => util::open_url("https://claude.ai/settings/usage"),
        ID_OPEN_COPILOT => util::open_url("https://github.com/settings/copilot"),
        ID_EXIT => {
            let _ = DestroyWindow(main);
            return;
        }
        _ => {}
    }
    with_app(|app| {
        let mut save = false;
        match cmd {
            ID_RESET_POS => {
                app.offs = [8; 4];
                config::save_offsets(&app.offs);
            }
            c if (ID_PANEL_BASE..ID_PANEL_BASE + KINDS.len()).contains(&c) => {
                let k = KINDS[c - ID_PANEL_BASE];
                let on = !app.cfg.enabled(k.name());
                app.cfg.set_enabled(k.name(), on);
                save = true;
            }
            c if (ID_POPUP_STYLE_BASE..ID_POPUP_STYLE_BASE + POPUP_STYLES.len()).contains(&c) => {
                app.cfg.v["PopupStyle"] = serde_json::json!(POPUP_STYLES[c - ID_POPUP_STYLE_BASE].0);
                save = true;
            }
            ID_FMT_SYS_AUTO | ID_FMT_SYS_FULL | ID_FMT_SYS_ICON | ID_FMT_NET_AUTO | ID_FMT_NET_FULL | ID_FMT_NET_ICON | ID_FMT_AI_AUTO | ID_FMT_AI_FULL | ID_FMT_AI_ICON | ID_FMT_RADIO_AUTO | ID_FMT_RADIO_FULL | ID_FMT_RADIO_ICON => {
                let (k, v) = match cmd {
                    ID_FMT_AI_AUTO => (Kind::Usage, "auto"),
                    ID_FMT_AI_FULL => (Kind::Usage, "full"),
                    ID_FMT_AI_ICON => (Kind::Usage, "icon"),
                    ID_FMT_SYS_AUTO => (Kind::System, "auto"),
                    ID_FMT_SYS_FULL => (Kind::System, "full"),
                    ID_FMT_SYS_ICON => (Kind::System, "icon"),
                    ID_FMT_NET_AUTO => (Kind::Ip, "auto"),
                    ID_FMT_NET_FULL => (Kind::Ip, "full"),
                    ID_FMT_RADIO_AUTO => (Kind::Radio, "auto"),
                    ID_FMT_RADIO_FULL => (Kind::Radio, "full"),
                    ID_FMT_RADIO_ICON => (Kind::Radio, "icon"),
                    _ => (Kind::Ip, "icon"),
                };
                if !app.cfg.v["PanelStyle"].is_object() {
                    app.cfg.v["PanelStyle"] = serde_json::json!({});
                }
                app.cfg.v["PanelStyle"][k.name()] = serde_json::json!(v);
                save = true;
            }
            c if (ID_SPEED_BASE..ID_SPEED_BASE + speedtest::PROVIDERS.len()).contains(&c) => {
                let k = speedtest::PROVIDERS[c - ID_SPEED_BASE].0;
                speedtest::select(k);
                app.cfg.v["SpeedTestProvider"] = serde_json::json!(k);
                save = true;
            }
            ID_LANG_AUTO | ID_LANG_ES | ID_LANG_EN => {
                let v = match cmd {
                    ID_LANG_ES => "es",
                    ID_LANG_EN => "en",
                    _ => "auto",
                };
                app.cfg.v["Language"] = serde_json::json!(v);
                i18n::init(Some(v));
                save = true;
            }
            ID_SHOW_IP => {
                let on = app.cfg.v["IpShowAddress"].as_bool().unwrap_or(true);
                app.cfg.v["IpShowAddress"] = serde_json::json!(!on);
                save = true;
            }
            ID_NET_HEALTH => {
                let on = !net::health_on();
                net::set_health(on);
                app.cfg.v["NetHealth"] = serde_json::json!(on);
                save = true;
            }
            ID_AI_PRIMARY_ALL => {
                app.cfg.v["AiPrimary"] = serde_json::json!("all");
                save = true;
            }
            c if c >= ID_AI_PRIMARY_BASE && c < ID_AI_PRIMARY_BASE + ai_list.len() => {
                app.cfg.v["AiPrimary"] = serde_json::json!(ai_list[c - ID_AI_PRIMARY_BASE].id);
                save = true;
            }
            c if c >= ID_AI_TOGGLE_BASE && c < ID_AI_TOGGLE_BASE + ai_list.len() => {
                ai::toggle_provider(&mut app.cfg, &ai_list[c - ID_AI_TOGGLE_BASE].id);
                ai::refresh();
                save = true;
            }
            c if c >= ID_AI_REMOVE_BASE && c < ID_AI_REMOVE_BASE + ai_list.len() => {
                ai::remove_provider(&mut app.cfg, &ai_list[c - ID_AI_REMOVE_BASE].id);
                save = true;
            }
            c if (ID_MEDIA_W_BASE..ID_MEDIA_W_BASE + MEDIA_SIZES.len()).contains(&c) => {
                app.cfg.v["MediaWidth"] = serde_json::json!(MEDIA_SIZES[c - ID_MEDIA_W_BASE].0);
                set_media_w(&app.cfg);
                save = true;
            }
            c if (ID_STYLE_BASE..ID_STYLE_BASE + SPECTRUM_STYLES.len()).contains(&c) => {
                app.cfg.v["SpectrumStyle"] = serde_json::json!(SPECTRUM_STYLES[c - ID_STYLE_BASE].0);
                save = true;
            }
            ID_SCREENS_ALL => {
                app.cfg.v["Displays"] = serde_json::json!("all");
                save = true;
            }
            ID_SCREENS_PRIMARY => {
                app.cfg.v["Displays"] = serde_json::json!("primary");
                save = true;
            }
            c if c >= ID_SCREEN_BASE && c < ID_SCREEN_BASE + monitors.len() => {
                let id = &monitors[c - ID_SCREEN_BASE].0;
                let mut new: Vec<String> = selected.to_vec();
                if let Some(pos) = new.iter().position(|x| x == id) {
                    new.remove(pos);
                } else {
                    new.push(id.clone());
                }
                if !new.is_empty() {
                    app.cfg.v["Displays"] = if monitors.iter().all(|m| new.contains(&m.0)) {
                        serde_json::json!("all")
                    } else {
                        serde_json::json!(new)
                    };
                    save = true;
                }
            }
            _ => {}
        }
        if save {
            let mons = monitors_for_config(&app.bars, &app.cfg);
            app.cfg.save(&mons);
        }
        sync(app);
    });
}

// ======================================================================= temporizadores
/// Devuelve a Windows las páginas que no se están usando (working set). Tras el arranque quedan
/// ~20 MB residentes que solo se usan al inicializar (NVML, UI Automation, WinRT…); Windows los
/// recorta por su cuenta, pero a veces tras horas. Las páginas que se vuelvan a usar regresan solas.
fn trim_memory() {
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

thread_local! {
    /// próximo recorte de memoria programado (45 s tras arrancar; 20 s tras cerrar la ventana flotante)
    static TRIM_AT: std::cell::Cell<Option<Instant>> = std::cell::Cell::new(Some(Instant::now() + std::time::Duration::from_secs(45)));
}

/// Programa un recorte de memoria dentro de `secs` segundos
fn schedule_trim(secs: u64) {
    TRIM_AT.with(|t| t.set(Some(Instant::now() + std::time::Duration::from_secs(secs))));
}

unsafe fn on_main_timer(main: HWND) {
    // diagnóstico (solo con debug.flag): CPU por hilo cada 10 s
    thread_local! { static PROF_N: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
    if PROF_N.with(|n| { n.set(n.get() + 1); n.get() % 10 == 0 }) {
        let secs = util::sections_dump(); // se vacía siempre para que no crezca
        if dbg_on() {
            dbg(&format!("CPU por hilo (ms en 10 s): {} | secciones ui (ms/veces): {secs}", util::prof_dump()));
        }
    }
    if TRIM_AT.with(|t| t.get()).map(|at| Instant::now() >= at).unwrap_or(false) {
        TRIM_AT.with(|t| t.set(None));
        trim_memory();
    }
    // Modo claro/oscuro: se comprueba cada segundo en el color real de cada barra y se aplica en
    // caliente (también a la ventana flotante abierta)
    with_app(|app| {
        // Leer el color de cada barra captura la pantalla (caro): se hace cada segundo solo durante
        // los 20 s siguientes a un cambio de modo de Windows o al aparecer una barra nueva; el resto
        // del tiempo, cada 30 s (por si Explorer repinta tarde una barra que se quedó atascada)
        thread_local! { static THEME: std::cell::Cell<(Option<bool>, Option<Instant>, Option<Instant>)> = const { std::cell::Cell::new((None, None, None)) }; }
        let (mut last_sys, mut hot_until, mut sampled) = THEME.with(|t| t.get());
        let now = Instant::now();
        let sys = render::system_light();
        if last_sys != Some(sys) || app.bars.iter().any(|b| !app.bar_light.contains_key(&b.key)) {
            last_sys = Some(sys);
            hot_until = Some(now + std::time::Duration::from_secs(20));
        }
        if hot_until.is_some_and(|u| now < u) || sampled.map(|s| now.duration_since(s).as_secs() >= 30).unwrap_or(true) {
            util::timed("1s.color", || refresh_bar_themes(app));
            sampled = Some(now);
        }
        THEME.with(|t| t.set((last_sys, hot_until, sampled)));
        let light = app.light_of(app.popup_tb);
        if app.popup_shown && app.popup_light != Some(light) {
            render::set_light(light);
            apply_popup_style(app.popup, &app.popup_style);
            app.popup_light = Some(light);
            let _ = InvalidateRect(app.popup, None, FALSE);
        }
    });
    // Red: velocidad del último segundo
    let (rx, tx) = net_bytes();
    let balloon = with_app(|app| {
        let now = Instant::now();
        if let Some((prx, ptx, pt)) = app.net_prev {
            let dt = now.duration_since(pt).as_secs_f64();
            if dt > 0.0 {
                let mut s = st();
                let (d, u) = (rx.saturating_sub(prx) as f64 / dt, tx.saturating_sub(ptx) as f64 / dt);
                s.down_bps = d;
                s.up_bps = u;
                s.sess_down += d * dt;
                s.sess_up += u * dt;
                s.down = format!("↓ {}", fmt_rate(d));
                s.up = format!("↑ {}", fmt_rate(u));
                let push = |h: &mut Vec<f32>, v: f64| {
                    if h.is_empty() {
                        *h = vec![0.0; 60];
                    }
                    h.remove(0);
                    h.push(v as f32);
                };
                push(&mut s.hist_down, d);
                push(&mut s.hist_up, u);
            }
        }
        app.net_prev = Some((rx, tx, now));
        util::timed("1s.sync", || sync(app));
        st().ip_changed.take()
    })
    .flatten();
    util::timed("1s.paneles", || render_kind(false));
    if let Some((true, h)) = with_app(|a| (a.popup_shown, a.popup)) {
        let _ = InvalidateRect(h, None, FALSE);
    }
    let (ip, isp, city) = {
        let s = st();
        (s.ip.clone(), s.isp.clone(), s.city.clone())
    };
    let tip = [format!("{}: {ip}", tr("IP pública", "Public IP")), isp, city].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n");
    thread_local! { static TIP: RefCell<String> = const { RefCell::new(String::new()) }; }
    if TIP.with(|t| *t.borrow() != tip) {
        tray_tip(main, &tip);
        TIP.with(|t| *t.borrow_mut() = tip.clone());
    }
    if let Some((old, new, isp)) = balloon {
        tray_balloon(main, tr("Cambió la IP pública", "Public IP changed"), &format!("{old} → {new}\n{isp}"));
    }
}

thread_local! {
    /// hasta cuándo hay que reafirmar los paneles por encima de la barra (tras un cambio de primer plano)
    static RAISE_UNTIL: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Cambio de ventana en primer plano (incluye clics en la barra de tareas y sus desplegables): la
/// barra, al activarse, se coloca por encima de las ventanas "siempre encima" y taparía los paneles
unsafe extern "system" fn on_foreground(_h: windows::Win32::UI::Accessibility::HWINEVENTHOOK, _e: u32, _w: HWND, _o: i32, _c: i32, _t: u32, _ms: u32) {
    RAISE_UNTIL.with(|r| r.set(Some(Instant::now() + std::time::Duration::from_millis(600))));
    raise_panels();
}

/// Vuelve a poner los paneles visibles por encima de todo (sin moverlos ni activarlos)
unsafe fn raise_panels() {
    with_app(|app| {
        for p in &app.panels {
            if IsWindowVisible(p.hwnd).as_bool() {
                // hija de la barra: por encima de sus hermanas (contenido XAML); flotante: siempre encima
                let z = if p.parent.is_invalid() { HWND_TOPMOST } else { HWND_TOP };
                let _ = SetWindowPos(p.hwnd, z, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    });
}

unsafe fn on_fast_timer() {
    if RAISE_UNTIL.with(|r| r.get()).map(|u| Instant::now() < u).unwrap_or(false) {
        raise_panels();
    }
    // multimedia: el espectro se analiza a 30 Hz, así que 30 fps bastan; en pausa no hay nada que
    // animar salvo la transición del vinilo y el resaltado de los botones: 10 fps
    thread_local! { static TICK: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }
    let tick = TICK.with(|t| { t.set(t.get().wrapping_add(1)); t.get() });
    let every = if st().playing { 2 } else { 6 };
    if tick % every == 0 {
        util::timed("media", || render_kind(true));
    }
    // radio: el banner con el nombre de la emisora solo se anima (30 fps) si no cabe
    if tick % 2 == 0 && RADIO_BANNER.load(std::sync::atomic::Ordering::Relaxed) {
        util::timed("radio", || render_radio());
    }
    util::timed("hover", || update_popup());
}

// ======================================================================= ratón en paneles
fn panel_kind(hwnd: HWND) -> Option<(Kind, f32)> {
    with_app(|app| app.panels.iter().find(|p| p.hwnd == hwnd).map(|p| (p.kind, p.scale))).flatten()
}

unsafe fn on_mouse(hwnd: HWND, msg: u32, lp: LPARAM) -> bool {
    let Some((kind, sc)) = panel_kind(hwnd) else { return false };
    let x = (lp.0 & 0xFFFF) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let vertical = with_app(|a| a.panels.iter().find(|p| p.hwnd == hwnd).map(|p| p.vertical)).flatten().unwrap_or(false);
    let layout = if vertical {
        media_layout_v(rc.right as f32, rc.bottom as f32, sc)
    } else {
        media_layout(rc.right as f32, rc.bottom as f32, sc)
    };
    let button = |x: i32| -> i32 {
        for (i, (bx, by, br)) in layout.btn.iter().enumerate() {
            let (dx, dy) = (x as f32 - bx, y as f32 - by);
            if dx * dx + dy * dy <= (br * 1.25).powi(2) {
                return i as i32;
            }
        }
        -1
    };
    match (kind, msg) {
        (Kind::Media, WM_MOUSEMOVE) => {
            let b = button(x);
            with_app(|app| {
                if let Some(p) = app.panels.iter_mut().find(|p| p.hwnd == hwnd) {
                    p.hover = b;
                }
            });
            let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            let _ = TrackMouseEvent(&mut tme);
        }
        (Kind::Media, WM_LBUTTONUP) => match button(x) {
            0 => media::send(media::Cmd::Prev),
            1 => media::send(media::Cmd::Toggle),
            2 => media::send(media::Cmd::Next),
            _ => {}
        },
        // Red, IA y sistema: arrastrar con el botón izquierdo mueve solo ese panel por la barra
        (Kind::Ip | Kind::Usage | Kind::System | Kind::Radio, WM_LBUTTONDOWN) => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let started = with_app(|app| {
                let p = app.panels.iter().find(|p| p.hwnd == hwnd)?;
                if p.vertical {
                    return None;
                }
                let b = app.bars.iter().find(|b| b.key == p.tb_key)?;
                // se parte de la posición real (puede estar desplazado por un vecino)
                let off0 = (((b.right - r.right) as f32 / sc).round() as i32).max(0);
                let max = (((b.right - b.left - (r.right - r.left)) as f32 / sc) as i32).max(0);
                app.drag = Some(Drag { slot: chain_slot(kind)?, x0: pt.x, off0, max, moved: false });
                Some(())
            })
            .flatten()
            .is_some();
            if started {
                SetCapture(hwnd);
            }
        }
        (Kind::Ip | Kind::Usage | Kind::System | Kind::Radio, WM_MOUSEMOVE) => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            with_app(|app| {
                let Some(mut d) = app.drag else { return };
                let dx = pt.x - d.x0;
                if !d.moved && dx.abs() < 3 {
                    return; // umbral: un clic sin mover no recoloca nada
                }
                d.moved = true;
                app.drag = Some(d);
                app.offs[d.slot] = (d.off0 - (dx as f32 / sc) as i32).clamp(0, d.max);
                sync(app);
            });
        }
        (Kind::Ip | Kind::Usage | Kind::System | Kind::Radio, WM_LBUTTONUP) => {
            let _ = ReleaseCapture();
            with_app(|app| {
                if app.drag.take().is_some_and(|d| d.moved) {
                    config::save_offsets(&app.offs);
                }
            });
        }
        _ => return false,
    }
    true
}

// ======================================================================= procedimiento de ventana
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with_app(|a| a.popup == hwnd).unwrap_or(false) {
                util::timed("popup", || paint_popup_window(hwnd));
            } else {
                let _ = ValidateRect(hwnd, None);
            }
            return LRESULT(0);
        }
        WM_TIMER => {
            match wp.0 {
                TIMER_MAIN => util::timed("1s", || on_main_timer(hwnd)),
                TIMER_FAST => on_fast_timer(),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_TRAY => {
            let ev = (lp.0 & 0xFFFF) as u32;
            if ev == WM_RBUTTONUP || ev == WM_CONTEXTMENU {
                show_menu(hwnd);
            } else if ev == WM_LBUTTONDBLCLK {
                let ip = st().ip.clone();
                copy_to_clipboard(hwnd, &ip);
            }
            return LRESULT(0);
        }
        WM_RBUTTONUP => {
            if panel_kind(hwnd).is_some() {
                let main = with_app(|a| a.main).unwrap_or_default();
                show_menu(main);
                return LRESULT(0);
            }
        }
        WM_MOUSEMOVE | WM_LBUTTONUP if with_app(|a| a.popup == hwnd).unwrap_or(false) => {
            // ventana flotante: botones (p. ej. iniciar speed test)
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
            // cada zona pertenece a una ventana: las de otra (que siguen en la lista) no cuentan
            let kind = with_app(|a| a.popup_kind).flatten();
            let hit = match kind {
                Some(Kind::Radio) => popup_hit(x, y, true),
                Some(Kind::Ip) => popup_hit(x, y, false),
                _ => 0,
            };
            if msg == WM_LBUTTONUP && kind == Some(Kind::Radio) {
                RADIO_USED.with(|u| u.set(Some(Instant::now())));
                radio_click(hit);
                if hit == ACT_RADIO_SEARCH {
                    // el teclado solo llega a la ventana activa: se activa al pulsar el buscador
                    let _ = SetForegroundWindow(hwnd);
                    let _ = SetFocus(hwnd);
                    RADIO_FOCUS.with(|f| f.set(GetFocus() == hwnd));
                }
            }
            if msg == WM_LBUTTONUP && hit == ACT_SPEEDTEST {
                speedtest::start(speedtest::selected());
            }
            if msg == WM_LBUTTONUP && hit == ACT_SPEED_PROVIDER && !speedtest::running() {
                let k = speedtest::next_provider();
                speedtest::select(k);
                with_app(|app| {
                    app.cfg.v["SpeedTestProvider"] = serde_json::json!(k);
                    let mons = monitors_for_config(&app.bars, &app.cfg);
                    app.cfg.save(&mons);
                });
            }
            if POPUP_HOVER.with(|h| h.replace(hit)) != hit || msg == WM_LBUTTONUP {
                let _ = InvalidateRect(hwnd, None, FALSE);
            }
            return LRESULT(0);
        }
        WM_CHAR if with_app(|a| a.popup == hwnd && a.popup_kind == Some(Kind::Radio)).unwrap_or(false) => {
            RADIO_USED.with(|u| u.set(Some(Instant::now())));
            radio_char(wp.0 as u32);
            let _ = InvalidateRect(hwnd, None, FALSE);
            return LRESULT(0);
        }
        WM_KILLFOCUS if with_app(|a| a.popup == hwnd).unwrap_or(false) => {
            RADIO_FOCUS.with(|f| f.set(false));
            return LRESULT(0);
        }
        WM_MOUSEWHEEL if with_app(|a| a.popup == hwnd && a.popup_kind == Some(Kind::Radio)).unwrap_or(false) => {
            // lista de emisoras: dos filas por paso de rueda
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16;
            radio_scroll(if delta > 0 { -2 } else { 2 });
            POPUP_HOVER.with(|h| h.set(0)); // la fila bajo el cursor cambió: se recalcula al mover
            let _ = InvalidateRect(hwnd, None, FALSE);
            return LRESULT(0);
        }
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
            if on_mouse(hwnd, msg, lp) {
                return LRESULT(0);
            }
        }
        WM_MOUSELEAVE => {
            with_app(|app| {
                if let Some(p) = app.panels.iter_mut().find(|p| p.hwnd == hwnd) {
                    p.hover = -1;
                }
            });
            return LRESULT(0);
        }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_DESTROY => {
            let is_main = with_app(|a| a.main == hwnd).unwrap_or(false);
            if is_main {
                let d = tray_data(hwnd);
                let _ = Shell_NotifyIconW(NIM_DELETE, &d);
                sys::stop_presentmon();
                PostQuitMessage(0);
            }
            return LRESULT(0);
        }
        _ => {
            let tc = with_app(|a| a.taskbar_created).unwrap_or(0);
            if tc != 0 && msg == tc {
                tray_add(hwnd); // explorer se reinició: volver a crear el icono
                return LRESULT(0);
            }
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

// ======================================================================= registro de fallos
/// Ante una violación de acceso escribe la pila en %APPDATA%\TaskbarPanels\crash.log
unsafe extern "system" fn crash_handler(info: *mut windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
    let rec = &*(*info).ExceptionRecord;
    if rec.ExceptionCode.0 as u32 == 0xC000_0005 {
        let bt = std::backtrace::Backtrace::force_capture();
        let msg = format!(
            "{:?} · violación de acceso en {:?} (op {}, dir {:#x})\n{bt}\n\n",
            std::time::SystemTime::now(),
            rec.ExceptionAddress,
            rec.ExceptionInformation[0],
            rec.ExceptionInformation[1]
        );
        let path = util::appdata_dir().join("crash.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = f.write_all(msg.as_bytes());
        }
    }
    0 // EXCEPTION_CONTINUE_SEARCH
}

// ======================================================================= imágenes de demostración
/// Guarda un lienzo como BMP de 32 bits, compuesto sobre un color opaco
fn save_bmp(path: &str, c: &mut Surface, bg: (u8, u8, u8)) {
    let (w, h) = (c.w, c.h);
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for v in c.pixels() {
        let a = (v >> 24) & 0xFF;
        let mix = |s: u32, b: u8| (s + b as u32 * (255 - a) / 255).min(255) as u8;
        data.extend_from_slice(&[mix(v & 0xFF, bg.2), mix((v >> 8) & 0xFF, bg.1), mix((v >> 16) & 0xFF, bg.0), 255]);
    }
    let mut f = Vec::new();
    f.extend_from_slice(b"BM");
    f.extend_from_slice(&(54 + data.len() as u32).to_le_bytes());
    f.extend_from_slice(&[0, 0, 0, 0]);
    f.extend_from_slice(&54u32.to_le_bytes());
    f.extend_from_slice(&40u32.to_le_bytes());
    f.extend_from_slice(&w.to_le_bytes());
    f.extend_from_slice(&(-h).to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&32u16.to_le_bytes());
    f.extend_from_slice(&[0u8; 24]);
    f.extend_from_slice(&data);
    let _ = std::fs::write(path, f);
}

/// Datos ficticios para las imágenes (IP de documentación RFC 5737, nombres genéricos)
fn demo_state() {
    // bandera ficticia (franjas) para las capturas: no corresponde a ningún país
    let (fw, fh) = (30usize, 18usize);
    let px = (0..fw * fh).map(|i| if (i % fw) * fh < (fh - i / fw) * fw { 0xFF7A5CFF } else { 0xFF2EC4B6 }).collect();
    *flags::FLAG.lock().unwrap() = Some(std::sync::Arc::new(flags::Flag { code: "xx".into(), px, w: fw, h: fh }));
    *speedtest::SPEED.lock().unwrap() = speedtest::Speed {
        phase: speedtest::Phase::Done,
        progress: 1.0,
        ping_ms: Some(12.0),
        jitter_ms: Some(2.0),
        down_mbps: Some(487.0),
        up_mbps: Some(212.0),
        provider: "ookla".into(),
        server: tr("Example ISP · Ciudad", "Example ISP · City").into(),
        finished_at: Some(util::now_epoch() - 120),
        ..Default::default()
    };
    let mut s = st();
    let wave = |k: f32, a: f32, base: f32| (0..60).map(|i| base + (i as f32 * k).sin().abs() * a).collect::<Vec<f32>>();
    s.down_bps = 2_350_000.0;
    s.up_bps = 41_000.0;
    s.hist_down = wave(0.31, 2.6e6, 2e5);
    s.hist_up = wave(0.17, 6e4, 1e4);
    s.ip = "203.0.113.42".into();
    s.isp = "Example ISP S.A.".into();
    s.city = tr("Ciudad, País", "City, Country").into();
    s.sess_down = 1.84e9;
    s.sess_up = 2.1e8;
    {
        let now = util::now_epoch();
        let m = |label: &str, pct: f64, value: &str, detail: &str, reset: i64| ai::Metric {
            label: label.into(),
            pct: Some(pct),
            value: value.into(),
            detail: detail.into(),
            reset: Some(now + reset),
        };
        let prov = |id: &str, kind: &str, name: &str, plan: &str, metrics: Vec<ai::Metric>| ai::Provider {
            id: id.into(),
            kind: kind.into(),
            name: name.into(),
            plan: plan.into(),
            metrics,
            status: String::new(),
            fetched_at: now,
        };
        *ai::AI.lock().unwrap() = vec![
            prov("claude", "claude", "Claude", "pro", vec![m("session5h", 42.0, "42%", "", 2 * 3600 + 17 * 60), m("weekly", 18.0, "18%", "", 4 * 86_400 + 6 * 3600)]),
            prov("copilot", "copilot", "GitHub Copilot", "business", vec![m("premium", 12.5, "12.5%", "37 / 300", 16 * 86_400)]),
            prov("openai-1", "openai", "OpenAI API", "API", vec![m("month", 31.8, "$15.90", "$15.90 / $50.00", 17 * 86_400)]),
            prov("custom-1", "custom", "LiteLLM proxy", "", vec![ai::Metric { reset: None, ..m("usage", 64.0, "64%", "", 0) }]),
            prov("deepseek-1", "deepseek", "DeepSeek", "API", vec![ai::Metric { reset: None, ..m("balance", 27.5, "$7.25", "$7.25 / $10.00", 0) }]),
            prov("ollama-1", "ollama", "Ollama", "local", vec![ai::Metric { reset: None, pct: None, ..m("models", 0.0, "2/14", "llama3.2:3b, qwen3:8b  ·  9.5 GB", 0) }]),
        ];
    }
    (s.cpu, s.cpu_t, s.gpu, s.gpu_t, s.fps, s.fan_c, s.fan_g) = (38.0, 71, 64, 66, 165, 3100, 2900);
    (s.bat_present, s.bat_pct, s.bat_ac, s.bat_charging, s.bat_secs, s.bat_watts) = (true, 76, true, true, -1, 64.0);
    *sys::TOP.lock().unwrap() = [("Game", 31.0, 9.8e9, 1), ("chrome", 6.4, 3.1e9, 24), ("Code", 3.2, 2.2e9, 12), ("obs64", 8.5, 0.9e9, 1), ("Discord", 0.8, 0.6e9, 6)]
        .into_iter()
        .map(|(n, cpu, mem, procs)| sys::AppUse { name: n.into(), cpu, mem, procs })
        .collect();
    (s.plan_name, s.plan_kind, s.power_mode) = (tr("Equilibrado", "Balanced").into(), 2, 1);
    s.app = "Game.exe".into();
    s.cpu_name = "Intel Core Ultra 9 275HX".into();
    s.gpu_name = "RTX 5090 Laptop GPU".into();
    (s.cpu_mhz, s.vram_used, s.vram_total, s.gpu_power, s.gpu_mhz) = (5120.0, 9.4, 24.0, 118.0, 2310);
    (s.ram_used, s.ram_total) = (21.7, 63.4);
    s.hist_cpu = wave(0.21, 35.0, 18.0);
    s.hist_gpu = wave(0.11, 40.0, 40.0);
    s.hist_cpu_t = wave(0.07, 12.0, 62.0);
    s.hist_gpu_t = wave(0.05, 10.0, 58.0);
    let mk = |name: &str, vendor: u32, integrated: bool, npu: bool, r: (i32, i32, f32, f32, f32, u32, i32), f: f32| state::GpuStat {
        name: name.into(), vendor, integrated, npu, usage: r.0, temp: r.1, mem_used: r.2, mem_total: r.3, power: r.4, mhz: r.5, fan: r.6,
        hist: wave(f, 30.0, r.0 as f32), hist_t: if r.1 >= 0 { wave(0.05, 6.0, r.1 as f32 - 6.0) } else { Vec::new() },
    };
    s.gpus = vec![
        mk("RTX 5090 Laptop GPU", gpu::VENDOR_NVIDIA, false, false, (64, 66, 9.4, 24.0, 118.0, 2310, 2900), 0.11),
        mk("Radeon 890M", gpu::VENDOR_AMD, true, false, (12, 52, 1.2, 16.0, 0.0, 2900, -1), 0.17),
        mk("Intel AI Boost", gpu::VENDOR_INTEL, true, true, (23, -1, 0.4, 0.0, 0.0, 0, -1), 0.23),
    ];
    s.hist_fps = wave(0.13, 40.0, 130.0);
    s.disk_read = 48.0e6;
    s.disk_write = 12.5e6;
    s.hist_disk_r = wave(0.23, 5e7, 1e6);
    s.hist_disk_w = wave(0.15, 1.6e7, 5e5);
    s.drives = vec![
        state::Drive { letter: 'C', label: "Windows".into(), total: 2.0e12, free: 0.94e12 },
        state::Drive { letter: 'D', label: tr("Datos", "Data").into(), total: 2.0e12, free: 1.62e12 },
    ];
    s.title = tr("Artista de ejemplo - Canción de demostración (Remasterizada)", "Example Artist - Demo Song (Remastered)").into();
    s.artist = tr("Artista de ejemplo", "Example Artist").into();
    s.media_album = tr("Álbum de ejemplo", "Example Album").into();
    s.media_app = "Spotify".into();
    s.playing = true;
    drop(s);
    // carátula sintética (degradado + círculos) y color dominante
    let n = media::ART_SIZE;
    let mut px = vec![0u32; n * n];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            let ring = (((fx - 0.62).powi(2) + (fy - 0.4).powi(2)).sqrt() * 18.0).sin() * 0.5 + 0.5;
            let r = (40.0 + 200.0 * fx) * (0.7 + 0.3 * ring);
            let g = (30.0 + 80.0 * fy) * (0.8 + 0.2 * ring);
            let b = (120.0 + 120.0 * (1.0 - fx)) * (0.75 + 0.25 * ring);
            px[y * n + x] = (b as u32).min(255) | (g as u32).min(255) << 8 | (r as u32).min(255) << 16 | 0xFF00_0000;
        }
    }
    *media::ART.lock().unwrap() = Some(std::sync::Arc::new(media::Art { key: "demo".into(), px, accent: (240, 110, 210) }));
    *media::TIMELINE.lock().unwrap() = media::Timeline { pos: 131.0, dur: 247.0, at: None };
    *net::INFO.lock().unwrap() = Some(net::NetInfo {
        adapter: "Wi-Fi".into(),
        wifi: true,
        link_bps: 1_201_000_000,
        local_ip: "192.168.1.20".into(),
        gateway: "192.168.1.1".into(),
        dns: vec!["1.1.1.1".into(), "8.8.8.8".into()],
    });
    *net::PING.lock().unwrap() = net::PingStats {
        last: Some(3),
        hist: (0..30).map(|i| 2.5 + ((i as f32 * 0.7).sin().abs() * 4.0)).collect(),
        sent: 30,
        lost: 0,
        min: 2,
        max: 7,
        sum: 105,
        reply_at: None,
    };
    // salud de la conexión: estable, con un par de picos y un paquete perdido antiguo en Internet
    let demo_hop = |target: &str, base: f32, amp: f32, lost_at: Option<usize>| net::Hop {
        target: target.into(),
        hist: (0..net::HEALTH_SAMPLES)
            .map(|i| if Some(i) == lost_at { 0.0 } else { base + amp * (i as f32 * 0.9).sin().abs() + if i % 17 == 5 { amp * 2.5 } else { 0.0 } })
            .collect(),
        ever: true,
    };
    *net::HEALTH.lock().unwrap() = net::Health {
        router: demo_hop("192.168.1.1", 2.5, 3.0, None),
        dns: demo_hop("1.1.1.1", 11.5, 6.0, None),
        internet: demo_hop("1.1.1.1", 14.5, 9.0, Some(21)),
        wifi: true,
        no_net: false,
        started: true,
    };
    *media::LEVELS.lock().unwrap() = (0..media::BANDS).map(|i| 0.2 + 0.65 * ((i as f32 * 0.37).sin().abs()) * (1.0 - i as f32 / 90.0)).collect();
}

fn demo_panel(kind: Kind) -> Panel {
    Panel { hwnd: HWND::default(), parent: HWND::default(), kind, tb_key: 0, scale: 1.5, caps: vec![], disp: vec![], hover: -1, surf: None, vertical: false, compact: false, vinyl: 1.0, spin: 0.6, last_t: 0.0 }
}

/// `taskbar-panels.exe --render-demo <carpeta>`: genera las imágenes en modo oscuro y, con el
/// sufijo `-light`, en modo claro
fn render_demo(dir: &str) {
    radio::demo();
    demo_state();
    for light in [false, true] {
        render::set_light(light);
        render_demo_theme(dir, light);
    }
}

fn render_demo_theme(dir: &str, light: bool) {
    let sfx = if light { "-light" } else { "" };
    let s = st().clone();
    let demo_view = AiView { list: ai::AI.lock().unwrap().clone(), primary: "claude".into() };
    AI_POPUP_H.store(ai_popup_h(&demo_view).to_bits(), std::sync::atomic::Ordering::Relaxed);
    let sc = 1.5f32;
    let tb_h = 72;
    let bar_bg = if light { (238u8, 240u8, 245u8) } else { (26u8, 30u8, 48u8) };

    // ---- barra horizontal: multimedia a la izquierda, sistema · consumo · red a la derecha
    let widths = [(Kind::Media, 660.0), (Kind::Radio, 94.0), (Kind::System, 236.0), (Kind::Usage, 150.0), (Kind::Ip, 112.0)];
    let mut parts = Vec::new();
    for (kind, lw) in widths {
        let w = (lw * sc) as i32;
        let Some(mut c) = Surface::new(w, tb_h) else { continue };
        let mut p = demo_panel(kind);
        for frame in 0..30 {
            c.clear();
            let t = 3.0 + frame as f32 * 0.016;
            match kind {
                Kind::Ip => paint_ip(&mut c, w as f32, tb_h as f32, sc, &s, t, true),
                Kind::Usage => paint_ai(&mut c, w as f32, tb_h as f32, sc, &demo_view),
                Kind::System => paint_system(&mut c, w as f32, tb_h as f32, sc, &s),
                Kind::Media => paint_media(&mut c, w as f32, tb_h as f32, sc, &s, &mut p, "wave", t),
                Kind::Radio => paint_radio(&mut c, w as f32, tb_h as f32, sc, &radio::now(), t),
            }
        }
        parts.push((kind, w, c.pixels()));
    }
    let gap = (14.0 * sc) as i32;
    let right_w: i32 = parts.iter().filter(|p| p.0 != Kind::Media).map(|p| p.1 + gap).sum();
    let strip_w = 18 + parts[0].1 + 120 + right_w + 12;
    if let Some(mut strip) = Surface::new(strip_w, tb_h) {
        strip.raw(|c| c.fill(0.0, 0.0, strip_w as f32, tb_h as f32, Rgb::u8(bar_bg.0, bar_bg.1, bar_bg.2), 1.0));
        strip.raw(|c| c.fill(0.0, 0.0, strip_w as f32, 1.0, if light { Rgb(0.0, 0.0, 0.0) } else { WHITE }, 0.08));
        strip.paste(18, 0, &parts[0].2, parts[0].1, tb_h);
        let mut x = strip_w - 12;
        for (kind, w, px) in parts.iter().rev() {
            if *kind == Kind::Media {
                continue;
            }
            x -= w;
            strip.paste(x, 0, px, *w, tb_h);
            x -= gap;
        }
        save_bmp(&format!("{dir}\\barra{sfx}.bmp"), &mut strip, bar_bg);
    }

    // ---- ventanas flotantes sobre un fondo degradado (simula el escritorio tras el vidrio)
    for (name, kind) in [("popup-sistema", Kind::System), ("popup-red", Kind::Ip), ("popup-consumo", Kind::Usage), ("popup-multimedia", Kind::Media), ("popup-radio", Kind::Radio), ("popup-radio-paises", Kind::Radio)] {
        let (pw, ph) = ((popup_w(kind) * sc) as i32, (popup_h(kind) * sc) as i32);
        let (Some(mut c), Some(mut bg)) = (Surface::new(pw, ph), Surface::new(pw + 48, ph + 48)) else { continue };
        let mut disp = Vec::new();
        radio::demo_picker(name == "popup-radio-paises");
        for _ in 0..30 {
            match kind {
                Kind::System => paint_popup_system(&mut c, pw as f32, ph as f32, sc, &s, 3.0, false),
                Kind::Ip => paint_popup_ip(&mut c, pw as f32, ph as f32, sc, &s, 3.0, false),
                Kind::Usage => paint_popup_ai(&mut c, pw as f32, ph as f32, sc, &demo_view),
                Kind::Media => paint_popup_media(&mut c, pw as f32, ph as f32, sc, &s, &mut disp, 3.0, "wave"),
                Kind::Radio => paint_popup_radio(&mut c, pw as f32, ph as f32, sc, 3.0),
            }
        }
        if light {
            bg.gradient_bg(Rgb::u8(176, 198, 236), Rgb::u8(226, 204, 240));
        } else {
            bg.gradient_bg(Rgb::u8(58, 34, 92), Rgb::u8(24, 46, 96));
        }
        // simulación del desenfoque del escritorio: tinte uniforme bajo el vidrio
        let glass = if light { Rgb::u8(240, 242, 248) } else { Rgb::u8(40, 42, 60) };
        bg.raw(|b| b.round_rect(24.0, 24.0, pw as f32, ph as f32, 8.0 * sc, glass, 0.55));
        let px = c.pixels();
        bg.paste(24, 24, &px, pw, ph);
        save_bmp(&format!("{dir}\\{name}{sfx}.bmp"), &mut bg, (0, 0, 0));
    }

    // ---- barra vertical (ancho 52 px lógicos)
    let vw = (44.0 * sc) as i32;
    let mut blocks = Vec::new();
    for kind in [Kind::Media, Kind::Radio, Kind::System, Kind::Usage, Kind::Ip] {
        // por defecto, en barras verticales sistema, IA y red van como icono
        let h = if kind == Kind::Media { media_v_height(vw as f32, sc) as i32 } else { (vw as f32).min(48.0 * sc) as i32 };
        let Some(mut c) = Surface::new(vw, h) else { continue };
        let mut p = demo_panel(kind);
        p.vertical = true;
        for frame in 0..30 {
            c.clear();
            let t = 3.0 + frame as f32 * 0.016;
            match kind {
                Kind::Ip => paint_ip_compact(&mut c, vw as f32, h as f32, sc, &s),
                Kind::Usage => paint_ai_compact(&mut c, vw as f32, h as f32, sc, &demo_view),
                Kind::System => paint_system_compact(&mut c, vw as f32, h as f32, sc, &s),
                Kind::Media => paint_media_v(&mut c, vw as f32, h as f32, sc, &s, &mut p, "wave", t),
                Kind::Radio => paint_radio_compact(&mut c, vw as f32, h as f32, sc, &radio::now(), t),
            }
        }
        blocks.push((h, c.pixels()));
    }

    // ---- estilos de espectro: los cinco, uno bajo otro, con su nombre
    if !light {
        let lv = media::LEVELS.lock().unwrap().clone();
        let peaks: Vec<f32> = lv.iter().map(|v| (v + 0.12).min(1.0)).collect();
        let (vw, vh, lw) = ((240.0 * sc) as i32, (64.0 * sc) as i32, (120.0 * sc) as i32);
        let (rw, rh) = (lw + vw + (24.0 * sc) as i32, (vh + (10.0 * sc) as i32) * SPECTRUM_STYLES.len() as i32 + (10.0 * sc) as i32);
        if let Some(mut sp) = Surface::new(rw, rh) {
            sp.fill(0.0, 0.0, rw as f32, rh as f32, Rgb::u8(bar_bg.0, bar_bg.1, bar_bg.2), 1.0);
            for (i, (k, es, en)) in SPECTRUM_STYLES.iter().enumerate() {
                let y = (10.0 * sc) + i as f32 * (vh as f32 + 10.0 * sc);
                sp.text(tr(es, en), 12.0 * sc, y, lw as f32, vh as f32, 11.0 * sc, 600, "Segoe UI", Rgb::u8(205, 212, 228), DT_LEFT);
                spectrum(&mut sp, k, lw as f32 + 12.0 * sc, y, vw as f32, vh as f32, sc, &lv, &peaks, 0.2, 0.6);
            }
            save_bmp(&format!("{dir}\\spectrum-styles.bmp"), &mut sp, bar_bg);
        }
    }
    let total_h: i32 = blocks.iter().map(|b| b.0 + 12).sum::<i32>() + 12;
    let bar_w = vw + 12;
    if let Some(mut v) = Surface::new(bar_w, total_h) {
        v.raw(|c| c.fill(0.0, 0.0, bar_w as f32, total_h as f32, Rgb::u8(bar_bg.0, bar_bg.1, bar_bg.2), 1.0));
        let mut y = 12;
        for (h, px) in &blocks {
            v.paste(6, y, px, vw, *h);
            y += h + 12;
        }
        save_bmp(&format!("{dir}\\barra-vertical{sfx}.bmp"), &mut v, bar_bg);
    }
}

// ======================================================================= main
fn main() {
    let args: Vec<String> = std::env::args().collect();
    // diagnóstico: lista de GPU/NPU detectadas con dos lecturas (útil para equipos AMD/Intel)
    if args.len() >= 3 && args[1] == "--gpu-report" {
        let mut g = gpu::Gpus::new();
        let mut out = String::new();
        for i in 0..4 {
            let r = g.read();
            if i < 3 {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                continue;
            }
            for ((name, vendor, integrated, npu), r) in g.info().iter().zip(r.iter()) {
                out += &format!(
                    "{name} | vendor 0x{vendor:04X} | {} | uso {}% | temp {} °C | mem {:.2}/{:.2} GB | {:.0} W | {} MHz | fan {}
",
                    if *npu { "NPU" } else if *integrated { "integrada" } else { "dedicada" },
                    r.usage, r.temp, r.mem_used, r.mem_total, r.power, r.mhz, r.fan
                );
            }
        }
        let _ = std::fs::write(&args[2], out);
        return;
    }
    // diagnóstico: catálogo de emisoras por región y prueba de reproducción (en silencio)
    if args.len() >= 3 && args[1] == "--radio-report" {
        let _ = std::fs::write(&args[2], radio::report());
        return;
    }
    if args.len() >= 3 && args[1] == "--render-demo" {
        i18n::init(Some(args.get(3).map(|s| s.as_str()).unwrap_or("es")));
        render_demo(&args[2]);
        return;
    }
    render::set_light(render::system_light());
    // Idioma: el de Windows, salvo que config.json lo fije ("Language": "es" | "en")
    let boot_cfg = Config::load();
    set_media_w(&boot_cfg);
    i18n::init(boot_cfg.v["Language"].as_str());
    speedtest::select(speedtest::provider_key(&boot_cfg.v));
    unsafe {
        windows::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(1, Some(crash_handler));
        // Instancia única
        let _mutex = CreateMutexW(None, true, w!("Global\\TaskbarPanels"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // COM en el hilo de la interfaz (UI Automation para ubicar los iconos de la barra)
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
        let _ = InitCommonControlsEx(&icc);

        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let wc_popup = WNDCLASSEXW { lpszClassName: POPUP_CLASS, ..wc };
        RegisterClassExW(&wc_popup);

        // Ventana principal oculta: bandeja, temporizadores y menús
        let main = CreateWindowExW(WS_EX_TOOLWINDOW, CLASS, w!("taskbar-panels"), WS_POPUP, 0, 0, 0, 0, HWND::default(), HMENU::default(), hinst, None)
            .unwrap_or_default();

        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                cfg: Config::load(),
                main,
                panels: Vec::new(),
                bars: Vec::new(),
                offs: config::load_offsets(),
                drag: None,
                net_prev: None,
                taskbar_created,
                t0: Instant::now(),
                popup: HWND::default(),
                popup_surf: None,
                popup_hidden_at: None,
                popup_scale: 1.0,
                popup_shown: false,
                hover_since: None,
                popup_kind: None,
                popup_disp: Vec::new(),
                hide_since: Default::default(),
                hidden: Default::default(),
                last_playing: None,
                popup_style: String::new(),
                bar_light: std::collections::HashMap::new(),
                popup_tb: 0,
                popup_light: None,
            })
        });

        state::spawn_ip_worker();
        ai::spawn_worker();
        util::prof("ui");
        sys::spawn_sys_worker();
        sys::spawn_procs_worker();
        media::spawn_media_worker();
        media::spawn_audio_worker();
        radio::spawn_worker();
        net::spawn_net_worker();
        net::set_health(with_app(|a| a.cfg.v["NetHealth"].as_bool().unwrap_or(true)).unwrap_or(true));
        net::spawn_health_worker();
        host::spawn_icons_watcher();

        tray_add(main);
        with_app(|app| {
            sync(app);
            // Registrar nombres/códigos de monitores en config.json si faltan
            if app.cfg.v.get("MonitorIds").is_none() {
                let mons = monitors_for_config(&app.bars, &app.cfg);
                app.cfg.save(&mons);
            }
        });
        SetTimer(main, TIMER_MAIN, 1000, None);
        SetTimer(main, TIMER_FAST, 16, None);
        // Reafirmar los paneles cuando cambia la ventana en primer plano (clic en la barra, desplegables...)
        let _hook = windows::Win32::UI::Accessibility::SetWinEventHook(
            0x0003, // EVENT_SYSTEM_FOREGROUND
            0x0003,
            None,
            Some(on_foreground),
            0,
            0,
            0x0000 | 0x0002, // WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS
        );

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
