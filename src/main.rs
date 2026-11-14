//! taskbar-panels: paneles sobre la barra de tareas de Windows 11.
//!   · Red/IP        (↑ subida, ↓ bajada, IP pública)
//!   · Consumo       (Claude 5h / 7d, Copilot premium)
//!   · Sistema       (CPU, GPU, temperaturas, FPS)
//!   · Multimedia    (canción actual, botones, espectro neón)
//! Una ventana por panel y por barra seleccionada (config.json). Proceso único.

#![windows_subsystem = "windows"]

mod config;
mod host;
mod media;
mod render;
mod state;
mod sys;
mod util;

use config::Config;
use host::Taskbar;
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
}
const KINDS: [Kind; 4] = [Kind::Ip, Kind::Usage, Kind::System, Kind::Media];

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Ip => "NetworkPanel",
            Kind::Usage => "UsagePanel",
            Kind::System => "SystemPanel",
            Kind::Media => "MediaPanel",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Kind::Ip => "Red e IP pública",
            Kind::Usage => "Consumo Claude / Copilot",
            Kind::System => "Sistema (CPU, GPU, FPS)",
            Kind::Media => "Multimedia",
        }
    }
    /// ancho en px lógicos (96 dpi)
    fn width(self) -> f32 {
        match self {
            Kind::Ip => 112.0,
            Kind::Usage => 150.0,
            Kind::System => 236.0, // métricas + anillo del disco del sistema
            Kind::Media => 660.0,
        }
    }
}

struct Panel {
    hwnd: HWND,
    kind: Kind,
    tb_key: isize,
    scale: f32,
    caps: Vec<f32>,
    disp: Vec<f32>,
    hover: i32,
    surf: Option<Surface>,
    /// panel en una barra vertical (diseño compacto)
    vertical: bool,
    /// 0 = vinilo escondido, 1 = asomado (animado)
    vinyl: f32,
    spin: f32,
    last_t: f32,
}

struct App {
    cfg: Config,
    main: HWND,
    panels: Vec<Panel>,
    bars: Vec<Taskbar>,
    gap: i32,
    drag: Option<(i32, i32)>,
    net_prev: Option<(u64, u64, Instant)>,
    taskbar_created: u32,
    t0: Instant,
    popup: HWND,
    popup_surf: Option<Surface>,
    popup_scale: f32,
    popup_shown: bool,
    hover_since: Option<Instant>,
    popup_kind: Option<Kind>,
    popup_disp: Vec<f32>,
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
        None => GRAY,
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
    } else if s.ip.starts_with("consult") {
        (Rgb::u8(255, 200, 60), 0.4)
    } else {
        (Rgb::u8(255, 70, 90), 0.5)
    };
    c.dot(1.0 * sc + icon_w / 2.0, y + row / 2.0, 2.6 * sc, dot_col, 1.0, 2.5 * sc, glow);
    c.text(&s.ip, x_txt, y, w - x_txt, row, px, 400, "Segoe UI", GAINSBORO, DT_LEFT);
}

#[derive(Clone, Copy, PartialEq)]
enum UsageShow {
    Both,
    Claude,
    Copilot,
}

fn usage_show(cfg: &Config) -> UsageShow {
    match cfg.v["UsageShow"].as_str() {
        Some("claude") => UsageShow::Claude,
        Some("copilot") => UsageShow::Copilot,
        _ => UsageShow::Both,
    }
}

const CLAUDE5: Rgb = Rgb::u8(255, 140, 90);
const CLAUDE7: Rgb = Rgb::u8(255, 180, 120);
const COPILOT: Rgb = Rgb::u8(150, 110, 255);

/// Filas etiqueta · barra · valor; letra y barra crecen cuando hay menos filas
fn usage_rows(c: &mut Surface, w: f32, h: f32, sc: f32, rows: &[(&str, &state::UsageRow, Rgb)]) {
    let n = rows.len() as f32;
    let row = h / n;
    let px = (10.0 * sc * (3.0 / n).sqrt()).min((row * 0.62).floor());
    let hb = (row * 0.16).clamp(3.0, 7.0 * sc);
    let lw = rows.iter().map(|(l, _, _)| c.text_width(l, px, 400, "Segoe UI")).fold(0.0, f32::max);
    let vw = rows.iter().map(|(_, r, _)| c.text_width(&r.text, px, 600, "Segoe UI")).fold(c.text_width("100%", px, 600, "Segoe UI"), f32::max);
    let xb = 2.0 * sc + lw + 6.0 * sc;
    let wb = (w - xb - vw - 8.0 * sc).max(10.0 * sc);
    for (i, (label, r, base)) in rows.iter().enumerate() {
        let y = i as f32 * row;
        let col = severity(r.pct, *base);
        c.text(label, 2.0 * sc, y, lw + 2.0, row, px, 400, "Segoe UI", GAINSBORO, DT_LEFT);
        pill(c, xb, y + row / 2.0, wb, hb, r.pct.map(|p| p as f32 / 100.0), col);
        c.text(&r.text, w - vw - 2.0 * sc, y, vw, row, px, 600, "Segoe UI", col, DT_RIGHT);
    }
}

fn paint_usage(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, show: UsageShow) {
    match show {
        UsageShow::Both => usage_rows(c, w, h, sc, &[("Claude 5h", &s.usage[0], CLAUDE5), ("Claude 7d", &s.usage[1], CLAUDE7), ("Copilot", &s.usage[2], COPILOT)]),
        UsageShow::Claude => usage_rows(c, w, h, sc, &[("Claude 5h", &s.usage[0], CLAUDE5), ("Claude 7d", &s.usage[1], CLAUDE7)]),
        UsageShow::Copilot => {
            // Una sola métrica: nombre y porcentaje grandes arriba; barra y créditos abajo
            let r = &s.usage[2];
            let col = severity(r.pct, COPILOT);
            let top = (h * 0.58).floor();
            let px = (14.0 * sc).min((top * 0.62).floor());
            c.text("Copilot", 2.0 * sc, 0.0, w * 0.6, top, px, 400, "Segoe UI", GAINSBORO, DT_LEFT);
            c.text(&r.text, w * 0.4, 0.0, w * 0.6 - 2.0 * sc, top, px * 1.1, 700, "Segoe UI", col, DT_RIGHT);
            let bot = h - top;
            let small = (8.5 * sc).min((bot * 0.62).floor());
            let dw = c.text_width(&s.copilot_detail, small, 400, "Segoe UI");
            let hb = (bot * 0.22).clamp(3.0, 6.0 * sc);
            pill(c, 2.0 * sc, top + bot / 2.0, w - dw - 10.0 * sc, hb, r.pct.map(|p| p as f32 / 100.0), col);
            c.text(&s.copilot_detail, w - dw - 2.0 * sc, top, dw + 1.0, bot, small, 400, "Segoe UI", Rgb::u8(165, 175, 195), DT_RIGHT);
        }
    }
}
const CPU_COL: Rgb = Rgb::u8(0, 230, 255);
const GPU_COL: Rgb = Rgb::u8(120, 255, 90);
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
    let w = (150.0 * sc).min(full_w);
    paint_disk_mini(c, w + 4.0 * sc, full_w, h, sc, s);
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
}

// ======================================================================= ventana flotante del sistema
const POPUP_CLASS: PCWSTR = w!("TaskbarPanelsPopup");
const POPUP_W: f32 = 360.0;

const TEMP_COL: Rgb = Rgb::u8(255, 150, 70);

/// Leyenda mínima en la esquina superior derecha de una gráfica: "▬ uso  ▬ temp"
fn chart_legend(c: &mut Surface, right: f32, top: f32, sc: f32, use_col: Rgb) {
    let px = 8.0 * sc;
    let tw = c.text_width("temp", px, 400, "Segoe UI");
    let uw = c.text_width("uso", px, 400, "Segoe UI");
    let seg = 8.0 * sc;
    let x_t = right - tw - 2.0;
    let x_ts = x_t - seg - 3.0 * sc;
    let x_u = x_ts - 9.0 * sc - uw;
    let x_us = x_u - seg - 3.0 * sc;
    let yc = top + 5.0 * sc;
    c.capsule_h(x_us, x_us + seg, yc, 1.2 * sc, use_col, 1.0);
    c.text("uso", x_u, top, uw + 2.0, 10.0 * sc, px, 400, "Segoe UI", DIM, DT_LEFT);
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

/// Sección del panel: anillo del disco del sistema (% usado) + lectura/escritura
fn paint_disk_mini(c: &mut Surface, x0: f32, full_w: f32, h: f32, sc: f32, s: &state::State) {
    let Some(d) = system_drive(s) else { return };
    let used = (1.0 - d.free / d.total) as f32;
    let dia = (h * 0.80).min(32.0 * sc);
    let r = dia / 2.0;
    let (cx, cy) = (x0 + r, h / 2.0);
    let (c1, c2) = disk_cols(used);
    c.donut(cx, cy, r, (r * 0.30).max(3.0), used, c1, c2, 2.5 * sc);
    c.text(&format!("{:.0}", used * 100.0), cx - r, cy - r, dia, dia, (r * 0.62).floor(), 700, "Segoe UI", WHITE, DT_CENTER);
    let tx = x0 + dia + 5.0 * sc;
    let tw = full_w - tx - 2.0 * sc;
    let row = h / 3.0;
    let px = (8.5 * sc).min((row * 0.66).floor());
    // libre abreviado: "1.09T", "512G"
    let free = if d.free >= 1e12 { format!("{:.2}T", d.free / 1e12) } else { format!("{:.0}G", d.free / 1e9) };
    c.text(&format!("{}:", d.letter), tx, 0.0, tw, row, px, 700, "Segoe UI", DIM, DT_LEFT);
    c.text(&free, tx, 0.0, tw, row, px, 600, "Segoe UI", c1.mix(WHITE, 0.35), DT_RIGHT);
    c.text("R", tx, row, tw, row, px, 700, "Segoe UI", DIM, DT_LEFT);
    c.text(&rate_short(s.disk_read / 8.0), tx, row, tw, row, px, 600, "Segoe UI", DOWN_COL, DT_RIGHT);
    c.text("W", tx, 2.0 * row, tw, row, px, 700, "Segoe UI", DIM, DT_LEFT);
    c.text(&rate_short(s.disk_write / 8.0), tx, 2.0 * row, tw, row, px, 600, "Segoe UI", UP_COL, DT_RIGHT);
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
    card_head(c, x, y, cw, sc, "DISCOS", &format!("{n} unidad{} · {} libres", if n == 1 { "" } else { "es" }, fmt_size(total_free)), "", "", DISK_C1);
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
        c.text(&format!("{} libres", fmt_size(d.free)), tx, cy0 + 17.0 * sc, tw, 14.0 * sc, 9.5 * sc, 600, "Segoe UI", c1.mix(WHITE, 0.3), DT_LEFT | DT_END_ELLIPSIS);
        c.text(&format!("de {}", fmt_size(d.total)), tx, cy0 + 30.0 * sc, tw, 14.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    }
    // lectura / escritura
    let ry = y + 34.0 * sc + n.max(1).div_ceil(2) as f32 * 54.0 * sc;
    let maxv = s.hist_disk_r.iter().chain(s.hist_disk_w.iter()).fold(1e6f32, |m, v| m.max(*v)) * 1.15;
    for (i, (label, v, hist, col)) in [("LECTURA", s.disk_read, &s.hist_disk_r, DOWN_COL), ("ESCRITURA", s.disk_write, &s.hist_disk_w, UP_COL)].into_iter().enumerate() {
        let cx0 = x + 10.0 * sc + i as f32 * col_w;
        let bw = badge(c, cx0, ry + 8.0 * sc, label, 8.0 * sc, col, sc, 0.0);
        c.text(&format!("{}/s", fmt_bytes(v)), cx0 + bw + 6.0 * sc, ry, col_w - bw - 12.0 * sc, 16.0 * sc, 10.5 * sc, 600, "Segoe UI", col.mix(WHITE, 0.35), DT_LEFT);
        c.sparkline(cx0, ry + 20.0 * sc, col_w - 8.0 * sc, 28.0 * sc, hist, maxv, col, 1.0);
    }
    ch
}

fn popup_h(kind: Kind) -> f32 {
    match kind {
        Kind::System => 352.0 + 8.0 + disk_card_h(st().drives.len()),
        Kind::Ip => 324.0,
        Kind::Usage => 268.0,
        Kind::Media => 244.0,
    }
}

/// Fondo (tinte sobre el acrílico) + título y texto secundario a la derecha; devuelve la y de inicio
fn popup_frame(c: &mut Surface, w: f32, sc: f32, title: &str, right: &str) -> f32 {
    c.frosted(sc);
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

/// 1000000 → "1.000.000"
fn miles(v: f64) -> String {
    let s = format!("{:.0}", v.max(0.0));
    let b = s.as_bytes();
    let mut out = String::new();
    for (i, ch) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(*ch as char);
    }
    out
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

// ----------------------------------------------------------------------- Red
fn paint_popup_ip(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State) {
    let mut y = popup_frame(c, w, sc, "Red", &format!("sesión: ↓ {}  ↑ {}", fmt_bytes(s.sess_down), fmt_bytes(s.sess_up)));
    let pad = 12.0 * sc;
    let (x, cw) = (pad, w - pad * 2.0);
    // IP pública
    let ih = 64.0 * sc;
    card(c, x, y, cw, ih, sc);
    let online = s.ip.contains('.') || s.ip.contains(':');
    let dot = if online { Rgb::u8(60, 230, 120) } else { Rgb::u8(255, 70, 90) };
    let bw = badge(c, x + 10.0 * sc, y + 17.0 * sc, "IP PÚBLICA", 9.0 * sc, Rgb::u8(170, 200, 255), sc, 0.0);
    c.dot(x + 10.0 * sc + bw + 12.0 * sc, y + 17.0 * sc, 3.0 * sc, dot, 1.0, 3.0 * sc, 0.5);
    c.text(&s.ip, x + 10.0 * sc + bw + 20.0 * sc, y + 4.0 * sc, cw - bw - 30.0 * sc, 26.0 * sc, 16.0 * sc, 600, "Segoe UI", WHITE, DT_LEFT);
    let isp = [s.isp.as_str(), s.city.as_str()].iter().filter(|v| !v.is_empty()).copied().collect::<Vec<_>>().join("   ·   ");
    c.text(if isp.is_empty() { "ISP desconocido" } else { &isp }, x + 10.0 * sc, y + 34.0 * sc, cw - 20.0 * sc, 22.0 * sc, 10.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    y += ih + 8.0 * sc;
    // Descarga / subida
    let maxv = s.hist_down.iter().chain(s.hist_up.iter()).fold(8_000.0f32, |m, v| m.max(*v)) * 1.15;
    let ch = (h - y - pad - 8.0 * sc) / 2.0;
    let items = [("BAJADA", s.down_bps, &s.hist_down, DOWN_COL, s.sess_down), ("SUBIDA", s.up_bps, &s.hist_up, UP_COL, s.sess_up)];
    for (label, bps, hist, col, total) in items {
        card(c, x, y, cw, ch, sc);
        let (num, unit) = rate_parts(bps);
        let max = hist.iter().fold(0f32, |m, v| m.max(*v)) as f64;
        let (mn, mu) = rate_parts(max);
        card_head(c, x, y, cw, sc, label, &format!("máx. 60 s: {mn} {mu}"), &num, unit, col);
        c.text(&format!("total de la sesión: {}", fmt_bytes(total)), x + 10.0 * sc, y + 30.0 * sc, cw - 20.0 * sc, 14.0 * sc, 9.5 * sc, 400, "Segoe UI", DIM, DT_LEFT);
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
    c.capsule_h(x, x + w, by, 2.5 * sc, WHITE, 0.12);
    if let Some(p) = pct {
        c.capsule_h(x, x + (w * (p as f32 / 100.0).clamp(0.0, 1.0)).max(5.0 * sc), by, 2.5 * sc, severity(pct, col), 1.0);
    }
    c.text(note, x, y + 27.0 * sc, w, 14.0 * sc, 9.0 * sc, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
}

fn paint_popup_usage(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State) {
    let mut y = popup_frame(c, w, sc, "Consumo de IA", "se actualiza cada 3–5 min");
    let pad = 12.0 * sc;
    let (x, cw) = (pad, w - pad * 2.0);
    let ix = x + 10.0 * sc;
    let iw = cw - 20.0 * sc;
    let reset = |r: Option<i64>| r.map(|e| format!("reinicia en {}  ·  {}", util::fmt_left(e), util::fmt_local(e))).unwrap_or_else(|| "—".into());
    // Claude
    let ch = 128.0 * sc;
    card(c, x, y, cw, ch, sc);
    let plan = if s.claude_plan.is_empty() { "—".to_string() } else { format!("plan {}", s.claude_plan) };
    card_head(c, x, y, cw, sc, "CLAUDE", &plan, "", "", CLAUDE5);
    usage_line(c, ix, y + 34.0 * sc, iw, sc, "Sesión de 5 horas", s.usage[0].pct, &s.usage[0].text, &reset(s.claude_reset5), CLAUDE5);
    usage_line(c, ix, y + 80.0 * sc, iw, sc, "Límite semanal", s.usage[1].pct, &s.usage[1].text, &reset(s.claude_reset7), CLAUDE7);
    y += ch + 8.0 * sc;
    // Copilot
    let kh = h - y - pad;
    card(c, x, y, cw, kh, sc);
    let plan = if s.copilot_plan.is_empty() { "—".to_string() } else { format!("plan {}", s.copilot_plan) };
    card_head(c, x, y, cw, sc, "COPILOT", &plan, "", "", COPILOT);
    let label = if s.copilot_unlimited {
        "Solicitudes premium: ilimitadas".to_string()
    } else if s.copilot_total <= 0.0 {
        "Solicitudes premium: —".to_string()
    } else {
        format!("Solicitudes premium: {} de {}", miles(s.copilot_used), miles(s.copilot_total))
    };
    let note = s.copilot_reset.map(|e| format!("reinicia el {}  ·  en {}", util::fmt_date(e), util::fmt_left(e))).unwrap_or_else(|| "—".into());
    usage_line(c, ix, y + 34.0 * sc, iw, sc, &label, s.usage[2].pct, &s.usage[2].text, &note, COPILOT);
}

// ----------------------------------------------------------------------- Multimedia
fn paint_popup_media(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, disp: &mut Vec<f32>, t: f32) {
    let art = media::ART.lock().unwrap().clone();
    let accent = art.as_ref().map(|a| Rgb::u8(a.accent.0, a.accent.1, a.accent.2)).unwrap_or(Rgb::u8(0, 230, 255));
    let app = if s.media_app.is_empty() { "—".to_string() } else { s.media_app.clone() };
    let y0 = popup_frame(c, w, sc, "Reproduciendo", &app);
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
    let state_txt = if s.playing { "▶  reproduciendo" } else { "❚❚  en pausa" };
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
    spectrum_wave(c, x0, vy, x1 - x0, (h - vy - pad).max(8.0), sc, &d, &d, (t / 14.0).rem_euclid(1.0), pulse);
}

fn paint_popup_system(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State) {
    let mut y = popup_frame(c, w, sc, "Sistema", &format!("RAM {:.1}/{:.0} GB", s.ram_used, s.ram_total));
    let pad = 12.0 * sc;
    let cw = w - pad * 2.0;
    let x = pad;
    let small = 9.5 * sc;
    let fan = |v: i32| if v >= 0 { format!("{v} rpm") } else { "—".into() };
    let temp = |t: i32| if t >= 0 { format!("{t} °C") } else { "— °C".into() };

    // ---- CPU
    let ch = 84.0 * sc;
    card(c, x, y, cw, ch, sc);
    card_head(c, x, y, cw, sc, "CPU", &s.cpu_name, &format!("{:.0}", s.cpu.max(0.0)), "%", CPU_COL);
    let ghz = if s.cpu_mhz > 0.0 { format!("≈{:.2} GHz", s.cpu_mhz / 1000.0) } else { "— GHz".into() };
    let det = format!("{ghz}   ·   {}   ·   ventilador {}", temp(s.cpu_t), fan(s.fan_c));
    c.text(&det, x + 10.0 * sc, y + 30.0 * sc, cw - 20.0 * sc, 14.0 * sc, small, 400, "Segoe UI", temp_color(s.cpu_t).mix(DIM, 0.55), DT_LEFT | DT_END_ELLIPSIS);
    let (gx, gy, gw, gh) = (x + 10.0 * sc, y + 48.0 * sc, cw - 20.0 * sc, ch - 54.0 * sc);
    c.sparkline(gx, gy, gw, gh, &s.hist_cpu, 100.0, CPU_COL, 1.0);
    c.trend_line(gx, gy, gw, gh, &s.hist_cpu_t, 105.0, TEMP_COL, 0.95);
    chart_legend(c, gx + gw, gy, sc, CPU_COL);
    y += ch + 8.0 * sc;

    // ---- GPU
    card(c, x, y, cw, ch, sc);
    card_head(c, x, y, cw, sc, "GPU", &s.gpu_name, &format!("{}", s.gpu.max(0)), "%", GPU_COL);
    let det = format!(
        "{}   ·   VRAM {:.1}/{:.0} GB   ·   {:.0} W   ·   {}   ·   {}",
        temp(s.gpu_t), s.vram_used, s.vram_total, s.gpu_power,
        if s.gpu_mhz > 0 { format!("{:.2} GHz", s.gpu_mhz as f32 / 1000.0) } else { "— GHz".into() },
        fan(s.fan_g)
    );
    c.text(&det, x + 10.0 * sc, y + 30.0 * sc, cw - 20.0 * sc, 14.0 * sc, small, 400, "Segoe UI", temp_color(s.gpu_t).mix(DIM, 0.55), DT_LEFT | DT_END_ELLIPSIS);
    let (gx, gy) = (x + 10.0 * sc, y + 48.0 * sc);
    c.sparkline(gx, gy, gw, gh, &s.hist_gpu, 100.0, GPU_COL, 1.0);
    c.trend_line(gx, gy, gw, gh, &s.hist_gpu_t, 105.0, TEMP_COL, 0.95);
    chart_legend(c, gx + gw, gy, sc, GPU_COL);
    y += ch + 8.0 * sc;

    // ---- Memoria
    let rh = 46.0 * sc;
    card(c, x, y, cw, rh, sc);
    let frac = if s.ram_total > 0.0 { s.ram_used / s.ram_total } else { 0.0 };
    card_head(c, x, y, cw, sc, "RAM", &format!("{:.1} de {:.1} GB", s.ram_used, s.ram_total), &format!("{:.0}", frac * 100.0), "%", RAM_COL);
    let by = y + 36.0 * sc;
    c.capsule_h(x + 10.0 * sc, x + cw - 10.0 * sc, by, 2.5 * sc, WHITE, 0.12);
    c.capsule_h(x + 10.0 * sc, x + 10.0 * sc + ((cw - 20.0 * sc) * frac).max(5.0 * sc), by, 2.5 * sc, RAM_COL, 1.0);
    y += rh + 8.0 * sc;

    // ---- Discos
    y += paint_disk_card(c, x, y, cw, sc, s) + 8.0 * sc;

    // ---- FPS
    let fh = h - y - pad;
    card(c, x, y, cw, fh, sc);
    let app = if s.app.is_empty() { "sin juego en primer plano".to_string() } else { s.app.trim_end_matches(".exe").to_string() };
    card_head(c, x, y, cw, sc, "FPS", &app, &if s.fps > 0 { s.fps.to_string() } else { "—".into() }, "fps", FPS_COL);
    let ft = if s.fps > 0 { format!("tiempo por frame {:.1} ms", 1000.0 / s.fps as f32) } else { "PresentMon mide la ventana en primer plano".into() };
    c.text(&ft, x + 10.0 * sc, y + 30.0 * sc, cw - 20.0 * sc, 14.0 * sc, small, 400, "Segoe UI", DIM, DT_LEFT | DT_END_ELLIPSIS);
    let fmax = s.hist_fps.iter().fold(60.0f32, |m, v| m.max(*v)) * 1.15;
    c.sparkline(x + 10.0 * sc, y + 48.0 * sc, cw - 20.0 * sc, (fh - 54.0 * sc).max(4.0), &s.hist_fps, fmax, FPS_COL, 1.0);
}

/// Desenfoque "vidrio esmerilado" detrás de la ventana: ACCENT_ENABLE_ACRYLICBLURBEHIND con un tinte
/// casi transparente (SetWindowCompositionAttribute, no documentada pero estable desde Windows 10)
unsafe fn set_glass_blur(h: HWND) {
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
    let mut ap = AccentPolicy { state: 4, flags: 0, gradient: 0x1A18_1210, anim: 0 }; // tinte azulado al ~10 %
    let mut d = CompAttrData { attrib: 19, data: &mut ap as *mut _ as *mut _, size: std::mem::size_of::<AccentPolicy>() }; // WCA_ACCENT_POLICY
    let _ = f(h, &mut d);
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
    let one: i32 = 1;
    let round: i32 = 2; // DWMWCP_ROUND
    let none: i32 = 1; // DWMSBT_NONE: el desenfoque lo pone la política de acento
    let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &one as *const _ as *const _, 4);
    let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
    let _ = DwmSetWindowAttribute(h, DWMWA_SYSTEMBACKDROP_TYPE, &none as *const _ as *const _, 4);
    set_glass_blur(h);
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
    let s = st().clone();
    with_app(|app| {
        if app.popup_surf.as_ref().map(|x| x.w != rc.right || x.h != rc.bottom).unwrap_or(true) {
            app.popup_surf = Surface::new(rc.right, rc.bottom);
        }
        let sc = app.popup_scale;
        let tt = app.t0.elapsed().as_secs_f32();
        let (wf, hf) = (rc.right as f32, rc.bottom as f32);
        if let (Some(surf), Some(kind)) = (app.popup_surf.as_mut(), app.popup_kind) {
            match kind {
                Kind::System => paint_popup_system(surf, wf, hf, sc, &s),
                Kind::Ip => paint_popup_ip(surf, wf, hf, sc, &s),
                Kind::Usage => paint_popup_usage(surf, wf, hf, sc, &s),
                Kind::Media => paint_popup_media(surf, wf, hf, sc, &s, &mut app.popup_disp, tt),
            }
            surf.blit(hdc);
        }
    });
    let _ = EndPaint(hwnd, &ps);
}

/// Registro de diagnóstico (solo si existe %APPDATA%\TaskbarPanels\debug.flag)
fn dbg(msg: &str) {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| util::appdata_dir().join("debug.flag").exists());
    if *ON {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(util::appdata_dir().join("debug.log")) {
            let _ = writeln!(f, "{:?} {msg}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
        }
    }
}

/// Ventana flotante: aparece si el cursor lleva 300 ms sobre un panel; sigue abierta mientras el
/// cursor esté sobre el panel o sobre la propia ventana; cambia al instante entre paneles.
unsafe fn update_popup() {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    with_app(|app| {
        let inside = |r: &RECT| pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom;
        let mut over: Option<(RECT, f32, isize, Kind)> = None;
        for p in app.panels.iter() {
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
        dbg(&format!("cursor=({},{}) over={:?} shown={} kind={:?} on_popup={on_popup}", pt.x, pt.y, over.map(|o| o.3), app.popup_shown, app.popup_kind));
        match over {
            Some((r, sc, tb_key, kind)) => {
                let since = *app.hover_since.get_or_insert_with(Instant::now);
                let switch = app.popup_shown && app.popup_kind != Some(kind);
                if (since.elapsed().as_millis() >= 300 && !app.popup_shown) || switch {
                    let h = ensure_popup(app);
                    if h.is_invalid() {
                        return;
                    }
                    app.popup_scale = sc;
                    app.popup_kind = Some(kind);
                    let (pw, ph) = ((POPUP_W * sc) as i32, (popup_h(kind) * sc) as i32);
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
                if app.popup_shown {
                    dbg("OCULTAR");
                    let _ = ShowWindow(app.popup, SW_HIDE);
                    app.popup_shown = false;
                    app.popup_kind = None;
                }
            }
        }
        // multimedia: animación a 60 fps (espectro, progreso)
        if app.popup_shown && app.popup_kind == Some(Kind::Media) {
            let _ = InvalidateRect(app.popup, None, FALSE);
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
fn media_place(tb: &Taskbar, right_limit: i32) -> Option<(i32, i32)> {
    let sc = tb.scale;
    let (x, avail) = if tb.left_aligned {
        let x = tb.tasks_right + (12.0 * sc) as i32;
        (x, (right_limit - x) as f32 - 12.0 * sc)
    } else {
        (tb.left + (12.0 * sc) as i32, (tb.start_left - tb.left) as f32 - 24.0 * sc)
    };
    if avail < 240.0 * sc {
        return None;
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
    let viz_w = (210.0 * sc).min(w * 0.28);
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
fn paint_media(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, p: &mut Panel, bars_style: bool, t: f32) {
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
        let k = if target[i] > p.disp[i] { 0.45 } else { 0.12 };
        p.disp[i] += (target[i] - p.disp[i]) * k;
        p.caps[i] = (p.caps[i] - 0.006).max(p.disp[i]);
    }
    let pulse = (p.disp.iter().take(6).sum::<f32>() / 6.0).clamp(0.0, 1.0);
    let phase = (t / 14.0).rem_euclid(1.0);
    let (disp, caps) = (p.disp.clone(), p.caps.clone());
    if bars_style {
        spectrum_bars(c, l.viz_x, h * 0.06, l.viz_w, h * 0.88, sc, &disp, &caps, phase, pulse);
    } else {
        spectrum_wave(c, l.viz_x, h * 0.06, l.viz_w, h * 0.88, sc, &disp, &caps, phase, pulse);
    }
}

// ======================================================================= barras verticales (izquierda/derecha)
/// Alto (px lógicos) de cada panel apilado en una barra vertical
fn v_height(k: Kind, show_ip: bool, usage: UsageShow) -> f32 {
    match k {
        Kind::Ip => if show_ip { 96.0 } else { 66.0 },
        Kind::Usage => match usage { UsageShow::Both => 96.0, UsageShow::Claude => 66.0, UsageShow::Copilot => 36.0 },
        Kind::System => 144.0,
        Kind::Media => 0.0, // depende del ancho (media_v_height)
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
    if show_ip {
        // punto de estado con halo, alineado con la etiqueta "IP"
        let bh = h / 3.0;
        let lpx = (8.0 * sc).min((bh * 0.28).floor());
        let online = s.ip.contains('.') || s.ip.contains(':');
        let col = if online { Rgb::u8(60, 230, 120) } else { Rgb::u8(255, 70, 90) };
        c.dot(w - 7.0 * sc, 2.0 * bh + 2.0 * sc + (lpx + 4.0 * sc) / 2.0, 2.4 * sc, col, 1.0, 2.5 * sc, 0.5);
    }
}

fn paint_usage_v(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, show: UsageShow) {
    let row = |label, r: &state::UsageRow, col| VBlock {
        label,
        value: r.text.clone(),
        sub: String::new(),
        sub_col: DIM,
        col: severity(r.pct, col),
        hist: None,
        frac: r.pct.map(|p| p as f32 / 100.0),
    };
    let blocks = match show {
        UsageShow::Both => vec![row("Claude 5h", &s.usage[0], CLAUDE5), row("Claude 7d", &s.usage[1], CLAUDE7), row("Copilot", &s.usage[2], COPILOT)],
        UsageShow::Claude => vec![row("Claude 5h", &s.usage[0], CLAUDE5), row("Claude 7d", &s.usage[1], CLAUDE7)],
        UsageShow::Copilot => vec![row("Copilot", &s.usage[2], COPILOT)],
    };
    paint_vblocks(c, w, h, sc, &blocks);
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
                label: if letter == 'C' { "DISCO C:" } else { "DISCO" },
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
fn paint_media_v(c: &mut Surface, w: f32, h: f32, sc: f32, s: &state::State, p: &mut Panel, t: f32) {
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
        let k = if target[i] > p.disp[i] { 0.45 } else { 0.12 };
        p.disp[i] += (target[i] - p.disp[i]) * k;
        p.caps[i] = (p.caps[i] - 0.006).max(p.disp[i]);
    }
    let pulse = (p.disp.iter().take(6).sum::<f32>() / 6.0).clamp(0.0, 1.0);
    let (d, cp) = (p.disp.clone(), p.caps.clone());
    spectrum_wave(c, l.viz_x, l.viz_y, l.viz_w, l.viz_h, sc, &d, &cp, (t / 14.0).rem_euclid(1.0), pulse);
}

/// Apila los paneles en una barra vertical: red/consumo/sistema hacia arriba desde la bandeja,
/// multimedia bajo los iconos de aplicaciones
unsafe fn place_vertical(panels: &mut [Panel], b: &Taskbar, hide: bool, show_ip: bool, usage: UsageShow) {
    let sc = b.scale;
    let m = (4.0 * sc) as i32;
    let x = b.rect.left + m;
    let w = (b.rect.right - b.rect.left) - 2 * m;
    let mut bottom = b.tray_top - (8.0 * sc) as i32;
    for k in [Kind::Ip, Kind::Usage, Kind::System] {
        let Some(p) = panels.iter_mut().find(|p| p.kind == k && p.tb_key == b.key) else { continue };
        p.scale = sc;
        p.vertical = true;
        if hide || w < 20 {
            let _ = ShowWindow(p.hwnd, SW_HIDE);
            continue;
        }
        let h = (v_height(k, show_ip, usage) * sc) as i32;
        let y = bottom - h;
        bottom = y - (8.0 * sc) as i32;
        let _ = SetWindowPos(p.hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    if let Some(p) = panels.iter_mut().find(|p| p.kind == Kind::Media && p.tb_key == b.key) {
        p.scale = sc;
        p.vertical = true;
        let top = b.tasks_bottom + (10.0 * sc) as i32;
        let need = media_v_height(w as f32, sc) as i32;
        if hide || w < 20 || bottom - top < need {
            let _ = ShowWindow(p.hwnd, SW_HIDE); // sin espacio entre los iconos y los paneles
        } else {
            let _ = SetWindowPos(p.hwnd, HWND_TOPMOST, x, top, w, need, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }
}

/// Dibuja el panel en su lienzo y lo publica con alfa por píxel
unsafe fn render(p: &mut Panel, s: &state::State, bars_style: bool, show: UsageShow, show_ip: bool, t: f32) {
    if !IsWindowVisible(p.hwnd).as_bool() {
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
        (Kind::Ip, false) => paint_ip(&mut surf, wf, hf, sc, s, t, show_ip),
        (Kind::Usage, false) => paint_usage(&mut surf, wf, hf, sc, s, show),
        (Kind::System, false) => paint_system(&mut surf, wf, hf, sc, s),
        (Kind::Media, false) => paint_media(&mut surf, wf, hf, sc, s, p, bars_style, t),
        (Kind::Ip, true) => paint_ip_v(&mut surf, wf, hf, sc, s, show_ip),
        (Kind::Usage, true) => paint_usage_v(&mut surf, wf, hf, sc, s, show),
        (Kind::System, true) => paint_system_v(&mut surf, wf, hf, sc, s),
        (Kind::Media, true) => paint_media_v(&mut surf, wf, hf, sc, s, p, t),
    }
    surf.present(p.hwnd);
    p.surf = Some(surf);
}

unsafe fn render_kind(media_only: bool) {
    let s = st().clone();
    with_app(|app| {
        let bars = app.cfg.v["SpectrumStyle"].as_str() == Some("bars");
        let show = usage_show(&app.cfg);
        let show_ip = app.cfg.v["IpShowAddress"].as_bool().unwrap_or(true);
        let t = app.t0.elapsed().as_secs_f32();
        for p in app.panels.iter_mut() {
            if (p.kind == Kind::Media) == media_only {
                render(p, &s, bars, show, show_ip, t);
            }
        }
    });
}

// ======================================================================= sincronización de ventanas
unsafe fn create_panel(app: &mut App, kind: Kind, tb: &Taskbar) {
    let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
        CLASS,
        w!(""),
        WS_POPUP,
        0, 0, 10, 10,
        HWND::default(),
        HMENU::default(),
        hinst,
        None,
    );
    let Ok(hwnd) = hwnd else { return };
    app.panels.push(Panel { hwnd, kind, tb_key: tb.key, scale: tb.scale, caps: Vec::new(), disp: Vec::new(), hover: -1, surf: None, vertical: false, vinyl: 0.0, spin: 0.0, last_t: 0.0 });
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
    // Cerrar las sobrantes
    let bars = app.bars.clone();
    app.panels.retain(|p| {
        let keep = wanted.iter().any(|(k, i)| *k == p.kind && bars[*i].key == p.tb_key);
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
    let usage = usage_show(&app.cfg);
    for b in &bars {
        let hide = host::should_hide(b);
        if b.vertical {
            place_vertical(&mut app.panels, b, hide, show_ip, usage);
            continue;
        }
        let mut offset = app.gap as f32 * b.scale;
        for k in [Kind::Ip, Kind::Usage, Kind::System, Kind::Media] {
            let Some(p) = app.panels.iter_mut().find(|p| p.kind == k && p.tb_key == b.key) else { continue };
            p.scale = b.scale;
            p.vertical = false;
            if hide {
                let _ = ShowWindow(p.hwnd, SW_HIDE);
                continue;
            }
            let (x, w) = if k == Kind::Media {
                // Iconos a la izquierda → a la derecha de los iconos, hasta los paneles de la derecha
                let right_limit = b.right - offset as i32 + (14.0 * b.scale) as i32;
                match media_place(b, right_limit) {
                    Some(xw) => xw,
                    None => {
                        let _ = ShowWindow(p.hwnd, SW_HIDE); // no hay espacio suficiente
                        continue;
                    }
                }
            } else {
                let w = (k.width() * b.scale) as i32;
                let x = b.right - offset as i32 - w;
                offset += w as f32 + 14.0 * b.scale;
                (x, w)
            };
            let _ = SetWindowPos(p.hwnd, HWND_TOPMOST, x, b.top, w, b.height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
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
    copy_into(&mut d.szTip, "Paneles de la barra de tareas");
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
const ID_STYLE_WAVE: usize = 40;
const ID_STYLE_BARS: usize = 41;
const ID_USAGE_BOTH: usize = 50;
const ID_USAGE_CLAUDE: usize = 51;
const ID_USAGE_COPILOT: usize = 52;
const ID_SHOW_IP: usize = 60;
const ID_EXIT: usize = 99;

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
    let Ok(menu) = CreatePopupMenu() else { return };
    let show_ip = with_app(|a| a.cfg.v["IpShowAddress"].as_bool().unwrap_or(true)).unwrap_or(true);
    append(menu, ID_SHOW_IP, "Mostrar IP pública", show_ip);
    append(menu, ID_COPY_IP, "Copiar IP", false);
    append(menu, ID_REFRESH_IP, "Actualizar IP", false);
    append(menu, ID_REFRESH_USAGE, "Actualizar consumo", false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    append(menu, ID_OPEN_CLAUDE, "Abrir uso de Claude", false);
    append(menu, ID_OPEN_COPILOT, "Abrir uso de Copilot", false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    if let Ok(sub) = CreatePopupMenu() {
        for (i, k) in KINDS.iter().enumerate() {
            append(sub, ID_PANEL_BASE + i, k.label(), enabled[i]);
        }
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Paneles"));
    }
    if let Ok(sub) = CreatePopupMenu() {
        append(sub, ID_SCREENS_ALL, "Todas", cfg_sel.as_str() == Some("all"));
        append(sub, ID_SCREENS_PRIMARY, "Solo la principal", cfg_sel.as_str() == Some("primary"));
        let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
        for (i, (id, name, primary)) in monitors.iter().enumerate() {
            let label = format!("{name}  ({id}){}", if *primary { " · principal" } else { "" });
            append(sub, ID_SCREEN_BASE + i, &label, selected_ids.contains(id));
        }
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Pantallas"));
    }
    let bars_style = with_app(|a| a.cfg.v["SpectrumStyle"].as_str() == Some("bars")).unwrap_or(false);
    if let Ok(sub) = CreatePopupMenu() {
        append(sub, ID_STYLE_WAVE, "Onda líquida", !bars_style);
        append(sub, ID_STYLE_BARS, "Barras cápsula", bars_style);
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Estilo del espectro"));
    }
    let show = with_app(|a| usage_show(&a.cfg)).unwrap_or(UsageShow::Both);
    if let Ok(sub) = CreatePopupMenu() {
        append(sub, ID_USAGE_BOTH, "Claude y Copilot", show == UsageShow::Both);
        append(sub, ID_USAGE_CLAUDE, "Solo Claude", show == UsageShow::Claude);
        append(sub, ID_USAGE_COPILOT, "Solo Copilot", show == UsageShow::Copilot);
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Consumo"));
    }
    append(menu, ID_RESET_POS, "Reiniciar posición", false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    append(menu, ID_EXIT, "Salir", false);

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(main);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, main, None).0 as usize;
    let _ = PostMessageW(main, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    if cmd != 0 {
        on_command(main, cmd, &monitors, &selected_ids);
    }
}

unsafe fn on_command(main: HWND, cmd: usize, monitors: &[(String, String, bool)], selected: &[String]) {
    match cmd {
        ID_COPY_IP => {
            let ip = st().ip.clone();
            copy_to_clipboard(main, &ip);
        }
        ID_REFRESH_IP => state::refresh_ip(),
        ID_REFRESH_USAGE => state::refresh_usage(),
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
                app.gap = 8;
                config::save_gap(8);
            }
            c if (ID_PANEL_BASE..ID_PANEL_BASE + KINDS.len()).contains(&c) => {
                let k = KINDS[c - ID_PANEL_BASE];
                let on = !app.cfg.enabled(k.name());
                app.cfg.set_enabled(k.name(), on);
                save = true;
            }
            ID_SHOW_IP => {
                let on = app.cfg.v["IpShowAddress"].as_bool().unwrap_or(true);
                app.cfg.v["IpShowAddress"] = serde_json::json!(!on);
                save = true;
            }
            ID_USAGE_BOTH | ID_USAGE_CLAUDE | ID_USAGE_COPILOT => {
                let v = match cmd {
                    ID_USAGE_CLAUDE => "claude",
                    ID_USAGE_COPILOT => "copilot",
                    _ => "both",
                };
                app.cfg.v["UsageShow"] = serde_json::json!(v);
                save = true;
            }
            ID_STYLE_WAVE | ID_STYLE_BARS => {
                app.cfg.v["SpectrumStyle"] = serde_json::json!(if cmd == ID_STYLE_BARS { "bars" } else { "wave" });
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
unsafe fn on_main_timer(main: HWND) {
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
        sync(app);
        st().ip_changed.take()
    })
    .flatten();
    render_kind(false);
    if let Some((true, h)) = with_app(|a| (a.popup_shown, a.popup)) {
        let _ = InvalidateRect(h, None, FALSE);
    }
    let (ip, isp, city) = {
        let s = st();
        (s.ip.clone(), s.isp.clone(), s.city.clone())
    };
    let tip = [format!("IP pública: {ip}"), isp, city].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n");
    tray_tip(main, &tip);
    if let Some((old, new, isp)) = balloon {
        tray_balloon(main, "Cambió la IP pública", &format!("{old} → {new}\n{isp}"));
    }
}

unsafe fn on_fast_timer() {
    render_kind(true);
    update_popup();
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
        (Kind::Ip, WM_LBUTTONDOWN) => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            with_app(|app| app.drag = Some((pt.x, app.gap)));
            SetCapture(hwnd);
        }
        (Kind::Ip, WM_MOUSEMOVE) => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            with_app(|app| {
                if let Some((x0, g0)) = app.drag {
                    app.gap = (g0 - ((pt.x - x0) as f32 / sc) as i32).max(0);
                    sync(app);
                }
            });
        }
        (Kind::Ip, WM_LBUTTONUP) => {
            let _ = ReleaseCapture();
            with_app(|app| {
                if app.drag.take().is_some() {
                    config::save_gap(app.gap);
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
                paint_popup_window(hwnd);
            } else {
                let _ = ValidateRect(hwnd, None);
            }
            return LRESULT(0);
        }
        WM_TIMER => {
            match wp.0 {
                TIMER_MAIN => on_main_timer(hwnd),
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
    let mut s = st();
    let wave = |k: f32, a: f32, base: f32| (0..60).map(|i| base + (i as f32 * k).sin().abs() * a).collect::<Vec<f32>>();
    s.down_bps = 2_350_000.0;
    s.up_bps = 41_000.0;
    s.hist_down = wave(0.31, 2.6e6, 2e5);
    s.hist_up = wave(0.17, 6e4, 1e4);
    s.ip = "203.0.113.42".into();
    s.isp = "Example ISP S.A.".into();
    s.city = "Ciudad, País".into();
    s.sess_down = 1.84e9;
    s.sess_up = 2.1e8;
    s.usage = [
        state::UsageRow { pct: Some(42.0), text: "42%".into() },
        state::UsageRow { pct: Some(18.0), text: "18%".into() },
        state::UsageRow { pct: Some(12.5), text: "12.5%".into() },
    ];
    s.claude_plan = "pro".into();
    s.claude_reset5 = Some(util::now_epoch() + 2 * 3600 + 17 * 60);
    s.claude_reset7 = Some(util::now_epoch() + 4 * 86_400 + 6 * 3600);
    s.copilot_plan = "business".into();
    (s.copilot_used, s.copilot_total, s.copilot_unlimited) = (37.0, 300.0, false);
    s.copilot_detail = "37 / 300".into();
    s.copilot_reset = Some(util::now_epoch() + 16 * 86_400);
    (s.cpu, s.cpu_t, s.gpu, s.gpu_t, s.fps, s.fan_c, s.fan_g) = (38.0, 71, 64, 66, 165, 3100, 2900);
    s.app = "Game.exe".into();
    s.cpu_name = "Intel Core Ultra 9 275HX".into();
    s.gpu_name = "RTX 5090 Laptop GPU".into();
    (s.cpu_mhz, s.vram_used, s.vram_total, s.gpu_power, s.gpu_mhz) = (5120.0, 9.4, 24.0, 118.0, 2310);
    (s.ram_used, s.ram_total) = (21.7, 63.4);
    s.hist_cpu = wave(0.21, 35.0, 18.0);
    s.hist_gpu = wave(0.11, 40.0, 40.0);
    s.hist_cpu_t = wave(0.07, 12.0, 62.0);
    s.hist_gpu_t = wave(0.05, 10.0, 58.0);
    s.hist_fps = wave(0.13, 40.0, 130.0);
    s.disk_read = 48.0e6;
    s.disk_write = 12.5e6;
    s.hist_disk_r = wave(0.23, 5e7, 1e6);
    s.hist_disk_w = wave(0.15, 1.6e7, 5e5);
    s.drives = vec![
        state::Drive { letter: 'C', label: "Windows".into(), total: 2.0e12, free: 0.94e12 },
        state::Drive { letter: 'D', label: "Datos".into(), total: 2.0e12, free: 1.62e12 },
    ];
    s.title = "Artista de ejemplo - Canción de demostración (Remasterizada)".into();
    s.artist = "Artista de ejemplo".into();
    s.media_album = "Álbum de ejemplo".into();
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
    *media::LEVELS.lock().unwrap() = (0..media::BANDS).map(|i| 0.2 + 0.65 * ((i as f32 * 0.37).sin().abs()) * (1.0 - i as f32 / 90.0)).collect();
}

fn demo_panel(kind: Kind) -> Panel {
    Panel { hwnd: HWND::default(), kind, tb_key: 0, scale: 1.5, caps: vec![], disp: vec![], hover: -1, surf: None, vertical: false, vinyl: 1.0, spin: 0.6, last_t: 0.0 }
}

/// `taskbar-panels.exe --render-demo <carpeta>`: genera las imágenes del README con datos ficticios
fn render_demo(dir: &str) {
    demo_state();
    let s = st().clone();
    let sc = 1.5f32;
    let tb_h = 72;
    let bar_bg = (26u8, 30u8, 48u8);

    // ---- barra horizontal: multimedia a la izquierda, sistema · consumo · red a la derecha
    let widths = [(Kind::Media, 660.0), (Kind::System, 236.0), (Kind::Usage, 150.0), (Kind::Ip, 112.0)];
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
                Kind::Usage => paint_usage(&mut c, w as f32, tb_h as f32, sc, &s, UsageShow::Both),
                Kind::System => paint_system(&mut c, w as f32, tb_h as f32, sc, &s),
                Kind::Media => paint_media(&mut c, w as f32, tb_h as f32, sc, &s, &mut p, false, t),
            }
        }
        parts.push((kind, w, c.pixels()));
    }
    let gap = (14.0 * sc) as i32;
    let right_w: i32 = parts.iter().filter(|p| p.0 != Kind::Media).map(|p| p.1 + gap).sum();
    let strip_w = 18 + parts[0].1 + 120 + right_w + 12;
    if let Some(mut strip) = Surface::new(strip_w, tb_h) {
        strip.fill(0.0, 0.0, strip_w as f32, tb_h as f32, Rgb::u8(bar_bg.0, bar_bg.1, bar_bg.2), 1.0);
        strip.fill(0.0, 0.0, strip_w as f32, 1.0, WHITE, 0.08);
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
        save_bmp(&format!("{dir}\\barra.bmp"), &mut strip, bar_bg);
    }

    // ---- ventanas flotantes sobre un fondo degradado (simula el escritorio tras el vidrio)
    for (name, kind) in [("popup-sistema", Kind::System), ("popup-red", Kind::Ip), ("popup-consumo", Kind::Usage), ("popup-multimedia", Kind::Media)] {
        let (pw, ph) = ((POPUP_W * sc) as i32, (popup_h(kind) * sc) as i32);
        let (Some(mut c), Some(mut bg)) = (Surface::new(pw, ph), Surface::new(pw + 48, ph + 48)) else { continue };
        let mut disp = Vec::new();
        for _ in 0..30 {
            match kind {
                Kind::System => paint_popup_system(&mut c, pw as f32, ph as f32, sc, &s),
                Kind::Ip => paint_popup_ip(&mut c, pw as f32, ph as f32, sc, &s),
                Kind::Usage => paint_popup_usage(&mut c, pw as f32, ph as f32, sc, &s),
                Kind::Media => paint_popup_media(&mut c, pw as f32, ph as f32, sc, &s, &mut disp, 3.0),
            }
        }
        bg.gradient_bg(Rgb::u8(58, 34, 92), Rgb::u8(24, 46, 96));
        // simulación del desenfoque del escritorio: tinte uniforme bajo el vidrio
        bg.round_rect(24.0, 24.0, pw as f32, ph as f32, 8.0 * sc, Rgb::u8(40, 42, 60), 0.55);
        let px = c.pixels();
        bg.paste(24, 24, &px, pw, ph);
        save_bmp(&format!("{dir}\\{name}.bmp"), &mut bg, (0, 0, 0));
    }

    // ---- barra vertical (ancho 52 px lógicos)
    let vw = (44.0 * sc) as i32;
    let mut blocks = Vec::new();
    for kind in [Kind::Media, Kind::System, Kind::Usage, Kind::Ip] {
        let h = if kind == Kind::Media { media_v_height(vw as f32, sc) as i32 } else { (v_height(kind, true, UsageShow::Both) * sc) as i32 };
        let Some(mut c) = Surface::new(vw, h) else { continue };
        let mut p = demo_panel(kind);
        p.vertical = true;
        for frame in 0..30 {
            c.clear();
            let t = 3.0 + frame as f32 * 0.016;
            match kind {
                Kind::Ip => paint_ip_v(&mut c, vw as f32, h as f32, sc, &s, true),
                Kind::Usage => paint_usage_v(&mut c, vw as f32, h as f32, sc, &s, UsageShow::Both),
                Kind::System => paint_system_v(&mut c, vw as f32, h as f32, sc, &s),
                Kind::Media => paint_media_v(&mut c, vw as f32, h as f32, sc, &s, &mut p, t),
            }
        }
        blocks.push((h, c.pixels()));
    }
    let total_h: i32 = blocks.iter().map(|b| b.0 + 12).sum::<i32>() + 12;
    let bar_w = vw + 12;
    if let Some(mut v) = Surface::new(bar_w, total_h) {
        v.fill(0.0, 0.0, bar_w as f32, total_h as f32, Rgb::u8(bar_bg.0, bar_bg.1, bar_bg.2), 1.0);
        let mut y = 12;
        for (h, px) in &blocks {
            v.paste(6, y, px, vw, *h);
            y += h + 12;
        }
        save_bmp(&format!("{dir}\\barra-vertical.bmp"), &mut v, bar_bg);
    }
}

// ======================================================================= main
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--render-demo" {
        render_demo(&args[2]);
        return;
    }
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
                gap: config::load_gap(),
                drag: None,
                net_prev: None,
                taskbar_created,
                t0: Instant::now(),
                popup: HWND::default(),
                popup_surf: None,
                popup_scale: 1.0,
                popup_shown: false,
                hover_since: None,
                popup_kind: None,
                popup_disp: Vec::new(),
            })
        });

        state::spawn_ip_worker();
        state::spawn_usage_worker();
        sys::spawn_sys_worker();
        media::spawn_media_worker();
        media::spawn_audio_worker();

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

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
