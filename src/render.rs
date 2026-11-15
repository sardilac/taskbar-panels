//! Lienzo con transparencia por píxel (DIB 32 bpp premultiplicado) para UpdateLayeredWindow.
//! Rellenos y cápsulas con antialiasing y halo; texto vía GDI (escala de grises → cobertura).

use crate::util::wide;
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy)]
pub struct Rgb(pub f32, pub f32, pub f32);

impl Rgb {
    pub const fn u8(r: u8, g: u8, b: u8) -> Self {
        Rgb(r as f32, g as f32, b as f32)
    }
    pub fn mix(self, o: Rgb, t: f32) -> Rgb {
        Rgb(self.0 + (o.0 - self.0) * t, self.1 + (o.1 - self.1) * t, self.2 + (o.2 - self.2) * t)
    }
}
pub const WHITE: Rgb = Rgb(255.0, 255.0, 255.0);

/// Modo claro de Windows activo: los colores (pensados para fondo oscuro) se adaptan al dibujar
static LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_light(on: bool) {
    LIGHT.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub fn is_light() -> bool {
    LIGHT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Modo de la barra de tareas y del sistema (`SystemUsesLightTheme`); oscuro si no existe
pub fn system_light() -> bool {
    use windows::Win32::System::Registry::*;
    let mut v: u32 = 0;
    let mut sz = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            windows::core::w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut sz),
        )
        .is_ok()
            && v != 0
    }
}

/// Convierte un color de la paleta oscura en su equivalente para fondo claro: invierte la
/// luminosidad perceptual (OKLab) conservando el tono, de modo que el blanco pasa a casi negro,
/// los grises secundarios a grises medios, los acentos brillantes a su versión oscura y las pistas
/// oscuras a gris claro. Al ser monótona, mantiene (invertido) el contraste entre elementos.
pub fn to_light(c: Rgb) -> Rgb {
    let lin = |v: f32| {
        let v = (v / 255.0).clamp(0.0, 1.0);
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c.0), lin(c.1), lin(c.2));
    let l = (0.412_221_5 * r + 0.536_332_5 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    let ll = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
    let aa = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let bb = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    let nl = 0.98 - 0.73 * ll;
    let to_rgb = |k: f32| -> Option<Rgb> {
        let (a, b) = (aa * k, bb * k);
        let l_ = (nl + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
        let m_ = (nl - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
        let s_ = (nl - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
        let r = 4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_;
        let g = -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_;
        let b = -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_;
        let ok = [r, g, b].iter().all(|v| (-0.001..=1.001).contains(v));
        let enc = |v: f32| {
            let v = v.clamp(0.0, 1.0);
            255.0 * if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
        };
        ok.then(|| Rgb(enc(r), enc(g), enc(b)))
    };
    if let Some(c) = to_rgb(1.0) {
        return c;
    }
    // fuera de gama: se reduce el croma (búsqueda binaria) conservando tono y luminosidad
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..8 {
        let mid = (lo + hi) / 2.0;
        if to_rgb(mid).is_some() { lo = mid } else { hi = mid }
    }
    to_rgb(lo).unwrap_or(Rgb(nl * 255.0, nl * 255.0, nl * 255.0))
}

/// Cobertura del texto (gris 0-255 de GDI → opacidad con gamma 0,85), precalculada
fn coverage_lut() -> &'static [f32; 256] {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|v| (v as f32 / 255.0).powf(0.85)))
}

struct Dib {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
}

impl Dib {
    unsafe fn new(w: i32, h: i32) -> Option<Dib> {
        let dc = CreateCompatibleDC(HDC::default());
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // de arriba hacia abajo
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).ok()?;
        let old = SelectObject(dc, bmp);
        Some(Dib { dc, bmp, old, bits: bits as *mut u32 })
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.dc);
        }
    }
}

thread_local! {
    /// DIB auxiliar compartido por todos los lienzos del hilo para rasterizar texto (blanco sobre
    /// negro → cobertura). Solo crece hasta el mayor rectángulo de texto dibujado, en lugar de una
    /// copia a tamaño completo por lienzo.
    static SCRATCH: std::cell::RefCell<Option<(Dib, i32, i32)>> = const { std::cell::RefCell::new(None) };
    /// Fuentes GDI en caché por (tamaño, grosor, familia)
    static FONTS: std::cell::RefCell<std::collections::HashMap<(i32, i32, String), HFONT>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// DIB auxiliar de al menos w×h con ese rectángulo en negro: (dc, píxeles, ancho de fila)
unsafe fn scratch(w: i32, h: i32) -> Option<(HDC, *const u32, i32)> {
    SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        let (cw, ch) = s.as_ref().map(|x| (x.1, x.2)).unwrap_or((0, 0));
        if w > cw || h > ch {
            let (nw, nh) = (w.max(cw).max(1), h.max(ch).max(1));
            *s = Some((Dib::new(nw, nh)?, nw, nh));
        }
        let (d, sw, _) = s.as_ref()?;
        let black = CreateSolidBrush(COLORREF(0));
        FillRect(d.dc, &RECT { left: 0, top: 0, right: w, bottom: h }, black);
        let _ = DeleteObject(black);
        Some((d.dc, d.bits as *const u32, *sw))
    })
}

/// Fuente GDI en caché (no se borra: el conjunto de tamaños es pequeño; se vacía si pasa de 96)
unsafe fn font(px: f32, weight: i32, face: &str) -> HFONT {
    let key = (-(px.max(6.0).round() as i32), weight, face.to_string());
    FONTS.with(|f| {
        let mut f = f.borrow_mut();
        if let Some(h) = f.get(&key) {
            return *h;
        }
        if f.len() > 96 {
            for (_, h) in f.drain() {
                let _ = DeleteObject(h);
            }
        }
        let w = wide(face);
        let h = CreateFontW(
            key.0, 0, 0, 0, weight, 0, 0, 0,
            DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32, CLIP_DEFAULT_PRECIS.0 as u32,
            ANTIALIASED_QUALITY.0 as u32, 0, PCWSTR(w.as_ptr()),
        );
        f.insert(key, h);
        h
    })
}

pub struct Surface {
    pub w: i32,
    pub h: i32,
    main: Dib,
    frosted: Option<(String, Vec<u32>)>, // fondo precalculado (estilo, píxeles)
    raw: u32,                            // > 0: dibujar sin adaptar los colores al modo claro
    memo: [(u32, Rgb); 2],               // últimas conversiones a modo claro (color empaquetado)
}

impl Surface {
    pub fn new(w: i32, h: i32) -> Option<Surface> {
        unsafe { Some(Surface { w, h, main: Dib::new(w, h)?, frosted: None, raw: 0, memo: [(u32::MAX, WHITE); 2] }) }
    }

    fn px(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.main.bits, (self.w * self.h) as usize) }
    }

    /// Fondo casi transparente (alfa 1): invisible, pero recibe clics y el tooltip
    pub fn clear(&mut self) {
        self.px().fill(0x0100_0000);
    }

    /// Dibuja con los colores tal cual (logotipos, fondos de demostración), también en modo claro
    pub fn raw<R>(&mut self, f: impl FnOnce(&mut Surface) -> R) -> R {
        self.raw += 1;
        let r = f(self);
        self.raw -= 1;
        r
    }

    /// Color adaptado al modo del sistema, con memoria de las dos últimas conversiones (los
    /// primitivos llaman a `blend` por píxel casi siempre con el mismo color)
    #[inline]
    fn themed(&mut self, c: Rgb) -> Rgb {
        if self.raw > 0 || !is_light() {
            return c;
        }
        let q = |v: f32| v.round().clamp(0.0, 255.0) as u32;
        let key = q(c.0) << 16 | q(c.1) << 8 | q(c.2);
        if self.memo[0].0 == key {
            return self.memo[0].1;
        }
        if self.memo[1].0 == key {
            self.memo.swap(0, 1);
            return self.memo[0].1;
        }
        let out = to_light(c);
        self.memo[1] = self.memo[0];
        self.memo[0] = (key, out);
        out
    }

    /// Mezcla "over" con el color adaptado al modo claro/oscuro del sistema
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if !(a > 0.002) {
            return;
        }
        let c = self.themed(c);
        self.blend_raw(x, y, c, a);
    }

    /// Mezcla "over" de un color con opacidad a ∈ [0,1] (premultiplicado)
    #[inline]
    fn blend_raw(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        // `!(a > ..)` descarta también NaN (antes acababa como opacidad 1 y pintaba un bloque sólido)
        if !(a > 0.002) || x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let a = a.min(1.0);
        let i = (y * self.w + x) as usize;
        let d = self.px()[i];
        let inv = 1.0 - a;
        let ch = |shift: u32, s: f32| -> u32 { ((s * a) + ((d >> shift) & 0xFF) as f32 * inv).round().clamp(0.0, 255.0) as u32 };
        let b = ch(0, c.2);
        let g = ch(8, c.1);
        let r = ch(16, c.0);
        let al = ((255.0 * a) + ((d >> 24) & 0xFF) as f32 * inv).round().clamp(0.0, 255.0) as u32;
        self.px()[i] = b | g << 8 | r << 16 | al << 24;
    }

    /// Rectángulo con bordes suavizados (cobertura fraccional en los bordes)
    pub fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb, a: f32) {
        let (x0, y0, x1, y1) = (x.floor() as i32, y.floor() as i32, (x + w).ceil() as i32, (y + h).ceil() as i32);
        for py in y0..y1 {
            let cy = ((py as f32 + 1.0).min(y + h) - (py as f32).max(y)).clamp(0.0, 1.0);
            for px in x0..x1 {
                let cx = ((px as f32 + 1.0).min(x + w) - (px as f32).max(x)).clamp(0.0, 1.0);
                self.blend(px, py, c, a * cx * cy);
            }
        }
    }

    /// Cápsula vertical centrada en cx, de y_top a y_bot, radio r; con halo gaussiano opcional.
    /// `core` aclara el centro de la cápsula hacia blanco (0 = sin núcleo).
    #[allow(clippy::too_many_arguments)]
    pub fn capsule(&mut self, cx: f32, y_top: f32, y_bot: f32, r: f32, c: Rgb, a: f32, glow_r: f32, glow_a: f32, core: f32) {
        let (sy0, sy1) = (y_top + r, (y_bot - r).max(y_top + r));
        let ext = r + glow_r * 2.0;
        let (x0, x1) = ((cx - ext).floor() as i32, (cx + ext).ceil() as i32);
        let (y0, y1) = ((y_top - glow_r * 2.0).floor() as i32, (y_bot + glow_r * 2.0).ceil() as i32);
        let mid = (y_top + y_bot) / 2.0;
        let half = ((y_bot - y_top) / 2.0).max(0.5);
        for py in y0..y1 {
            let fy = py as f32 + 0.5;
            let qy = fy.clamp(sy0, sy1);
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let d = ((fx - cx).powi(2) + (fy - qy).powi(2)).sqrt() - r;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let col = if core > 0.0 { c.mix(WHITE, core * (1.0 - ((fy - mid).abs() / half).min(1.0))) } else { c };
                    self.blend(px, py, col, a * cov);
                } else if glow_a > 0.0 && glow_r > 0.0 {
                    let g = glow_a * (-(d / glow_r).powi(2)).exp();
                    self.blend(px, py, c, g);
                }
            }
        }
    }

    /// Cápsula horizontal (barra de progreso redondeada) de x0 a x1, centrada en yc
    pub fn capsule_h(&mut self, x0: f32, x1: f32, yc: f32, r: f32, c: Rgb, a: f32) {
        let (sx0, sx1) = (x0 + r, (x1 - r).max(x0 + r));
        for py in (yc - r - 1.0).floor() as i32..(yc + r + 1.0).ceil() as i32 {
            let fy = py as f32 + 0.5;
            for px in (x0 - 1.0).floor() as i32..(x1 + 1.0).ceil() as i32 {
                let fx = px as f32 + 0.5;
                let qx = fx.clamp(sx0, sx1);
                let d = ((fx - qx).powi(2) + (fy - yc).powi(2)).sqrt() - r;
                self.blend(px, py, c, a * (0.5 - d).clamp(0.0, 1.0));
            }
        }
    }

    /// Texto suavizado: se dibuja en blanco sobre negro y la luminancia se usa como cobertura
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, s: &str, x: f32, y: f32, w: f32, h: f32, px: f32, weight: i32, face: &str, c: Rgb, flags: DRAW_TEXT_FORMAT) {
        // Un texto vacío produce un slice con puntero colgante (0x2): DrawTextW lo lee y el proceso cae
        if s.is_empty() {
            return;
        }
        let r = RECT {
            left: (x.round() as i32).max(0),
            top: (y.round() as i32).max(0),
            right: ((x + w).round() as i32).min(self.w),
            bottom: ((y + h).round() as i32).min(self.h),
        };
        if r.right <= r.left || r.bottom <= r.top {
            return;
        }
        unsafe {
            // se rasteriza en el DIB auxiliar con origen en la esquina del rectángulo
            let (tw, th) = (r.right - r.left, r.bottom - r.top);
            let Some((dc, bits, stride)) = scratch(tw, th) else { return };
            let old = SelectObject(dc, font(px, weight, face));
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            SetBkMode(dc, TRANSPARENT);
            let mut buf: Vec<u16> = s.encode_utf16().collect();
            let mut rr = RECT { left: 0, top: 0, right: tw, bottom: th };
            DrawTextW(dc, &mut buf, &mut rr, flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            SelectObject(dc, old);
            let lut = coverage_lut();
            for py in r.top..r.bottom {
                for px in r.left..r.right {
                    let v = (*bits.add(((py - r.top) * stride + (px - r.left)) as usize) >> 8) & 0xFF;
                    // casi todo el rectángulo está vacío: se salta sin calcular nada
                    if v != 0 {
                        self.blend(px, py, c, lut[v as usize]);
                    }
                }
            }
        }
    }

    /// Ancho en píxeles de un texto con la fuente indicada
    pub fn text_width(&self, s: &str, px: f32, weight: i32, face: &str) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        unsafe {
            let Some((dc, _, _)) = scratch(1, 1) else { return 0.0 };
            let old = SelectObject(dc, font(px, weight, face));
            let buf: Vec<u16> = s.encode_utf16().collect();
            let mut sz = SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &buf, &mut sz);
            SelectObject(dc, old);
            sz.cx as f32
        }
    }

    /// Mini gráfica de área: degradado bajo la curva y línea luminosa (data: más reciente al final)
    #[allow(clippy::too_many_arguments)]
    pub fn sparkline(&mut self, x: f32, y: f32, w: f32, h: f32, data: &[f32], maxv: f32, c: Rgb, strength: f32) {
        let n = data.len();
        if n < 2 || w < 2.0 || h < 2.0 {
            return;
        }
        let at = |u: f32| -> f32 {
            let t = u * (n - 1) as f32;
            let i = (t.floor() as usize).min(n - 2);
            let f = t - i as f32;
            ((data[i] + (data[i + 1] - data[i]) * f) / maxv).clamp(0.0, 1.0)
        };
        let bottom = y + h;
        for px in 0..w.round() as i32 {
            let u = px as f32 / (w - 1.0);
            let v = at(u);
            let top = bottom - v * (h - 1.0);
            let line_a = 0.12 + 0.88 * (v * 6.0).min(1.0); // sin tráfico la línea casi desaparece
            let fade = (u * 3.0).min(1.0); // se desvanece hacia la izquierda (lo más antiguo)
            let xx = x.round() as i32 + px;
            for py in (top.floor() as i32 - 1)..bottom.ceil() as i32 {
                let fy = py as f32 + 0.5;
                if fy >= top {
                    let k = ((fy - top) / h).clamp(0.0, 1.0);
                    self.blend(xx, py, c, strength * fade * (0.30 * (1.0 - k).powf(1.6) + 0.02));
                }
                let e = (1.0 - (fy - top).abs() / 0.9).clamp(0.0, 1.0);
                if e > 0.0 {
                    self.blend(xx, py, c.mix(WHITE, 0.25), strength * fade * e * 0.85 * line_a);
                }
            }
        }
    }

    /// Imagen cuadrada (BGRA premultiplicado, sw×sh) escalada con interpolación bilineal,
    /// con esquinas redondeadas suavizadas
    #[allow(clippy::too_many_arguments)]
    pub fn image_rounded(&mut self, x: f32, y: f32, size: f32, radius: f32, src: &[u32], sw: usize, sh: usize, a: f32) {
        self.image_rect(x, y, size, size, radius, src, sw, sh, a);
    }

    /// Imagen escalada (bilineal) a un rectángulo w×h con esquinas redondeadas
    pub fn image_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, src: &[u32], sw: usize, sh: usize, a: f32) {
        if src.len() < sw * sh || w < 2.0 || h < 2.0 {
            return;
        }
        let (hw, hh) = (w / 2.0, h / 2.0);
        let sample = |sx: i32, sy: i32| -> [f32; 4] {
            let p = src[(sy.clamp(0, sh as i32 - 1) as usize) * sw + sx.clamp(0, sw as i32 - 1) as usize];
            [(p & 0xFF) as f32, ((p >> 8) & 0xFF) as f32, ((p >> 16) & 0xFF) as f32, ((p >> 24) & 0xFF) as f32]
        };
        for py in y.floor() as i32..(y + h).ceil() as i32 {
            for px in x.floor() as i32..(x + w).ceil() as i32 {
                let (fx, fy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let qx = (fx - hw).abs() - (hw - radius);
                let qy = (fy - hh).abs() - (hh - radius);
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let (u, v) = (fx / w * sw as f32 - 0.5, fy / h * sh as f32 - 0.5);
                let (u0, v0) = (u.floor(), v.floor());
                let (du, dv) = (u - u0, v - v0);
                let (a00, a10, a01, a11) = (sample(u0 as i32, v0 as i32), sample(u0 as i32 + 1, v0 as i32), sample(u0 as i32, v0 as i32 + 1), sample(u0 as i32 + 1, v0 as i32 + 1));
                let mut ch = [0f32; 4];
                for k in 0..4 {
                    ch[k] = (a00[k] * (1.0 - du) + a10[k] * du) * (1.0 - dv) + (a01[k] * (1.0 - du) + a11[k] * du) * dv;
                }
                let sa = ch[3] / 255.0;
                if sa <= 0.0 {
                    continue;
                }
                self.blend_raw(px, py, Rgb(ch[2] / sa, ch[1] / sa, ch[0] / sa), a * cov * sa);
            }
        }
    }

    /// Disco de vinilo: surcos, reflejo que gira con `angle`, etiqueta de color y agujero central
    pub fn vinyl(&mut self, cx: f32, cy: f32, r: f32, angle: f32, label: Rgb, a: f32) {
        let base = Rgb::u8(16, 16, 20);
        for py in (cy - r - 1.0).floor() as i32..(cy + r + 1.0).ceil() as i32 {
            for px in (cx - r - 1.0).floor() as i32..(cx + r + 1.0).ceil() as i32 {
                let (dx, dy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
                let d = (dx * dx + dy * dy).sqrt();
                let cov = (r + 0.5 - d).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let k = d / r;
                let col = if k < 0.07 {
                    Rgb::u8(10, 10, 12) // agujero central
                } else if k < 0.34 {
                    // etiqueta con una marca que deja ver el giro
                    let ang = dy.atan2(dx) - angle;
                    let mark = if ang.cos() > 0.96 && k > 0.12 { 0.35 } else { 0.0 };
                    label.mix(Rgb::u8(20, 20, 24), 0.25).mix(WHITE, mark)
                } else {
                    // surcos + brillo especular en dos lóbulos que gira con el disco
                    let groove = 1.0 + 0.10 * (d * 2.2).sin();
                    let ang = dy.atan2(dx);
                    let spec = (2.0 * (ang - angle)).cos().max(0.0).powi(10) * 0.28 * k;
                    let g = Rgb(base.0 * groove, base.1 * groove, base.2 * groove);
                    g.mix(WHITE, spec)
                };
                self.blend_raw(px, py, col, a * cov);
            }
        }
    }

    /// Texto que empieza en `x` y solo se muestra entre clip0 y clip1, con bordes difuminados
    /// (para títulos que se desplazan)
    #[allow(clippy::too_many_arguments)]
    pub fn text_clip(&mut self, s: &str, x: f32, y: f32, h: f32, px: f32, weight: i32, face: &str, c: Rgb, clip0: f32, clip1: f32, fade: f32) {
        if s.is_empty() || clip1 <= clip0 {
            return;
        }
        let tw = self.text_width(s, px, weight, face) + 4.0;
        let (c0, c1) = ((clip0.round() as i32).max(0), (clip1.round() as i32).min(self.w));
        let (y0, y1) = ((y.round() as i32).max(0), ((y + h).round() as i32).min(self.h));
        if c1 <= c0 || y1 <= y0 {
            return;
        }
        unsafe {
            // se rasteriza en el DIB auxiliar con origen en (c0, y0): solo la franja visible
            let Some((dc, bits, stride)) = scratch(c1 - c0, y1 - y0) else { return };
            let old = SelectObject(dc, font(px, weight, face));
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            SetBkMode(dc, TRANSPARENT);
            let mut buf: Vec<u16> = s.encode_utf16().collect();
            let xl = x.round() as i32 - c0;
            let mut rr = RECT { left: xl, top: 0, right: xl + tw.round() as i32, bottom: y1 - y0 };
            DrawTextW(dc, &mut buf, &mut rr, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            SelectObject(dc, old);
            let lut = coverage_lut();
            for py in y0..y1 {
                for px in c0..c1 {
                    let v = (*bits.add(((py - y0) * stride + (px - c0)) as usize) >> 8) & 0xFF;
                    if v != 0 {
                        let mut cov = lut[v as usize];
                        let fx = px as f32 + 0.5;
                        if fade > 0.0 {
                            cov *= ((fx - clip0) / fade).clamp(0.0, 1.0) * ((clip1 - fx) / fade).clamp(0.0, 1.0);
                        }
                        self.blend(px, py, c, cov);
                    }
                }
            }
        }
    }

    /// Rectángulo redondeado con bordes suavizados
    #[allow(clippy::too_many_arguments)]
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgb, a: f32) {
        let (hw, hh) = (w / 2.0, h / 2.0);
        let r = r.min(hw).min(hh);
        for py in y.floor() as i32..(y + h).ceil() as i32 {
            for px in x.floor() as i32..(x + w).ceil() as i32 {
                let qx = (px as f32 + 0.5 - x - hw).abs() - (hw - r);
                let qy = (py as f32 + 0.5 - y - hh).abs() - (hh - r);
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
                self.blend(px, py, c, a * (0.5 - d).clamp(0.0, 1.0));
            }
        }
    }

    /// Rectángulo redondeado con degradado diagonal (de c0 arriba a la izquierda a c1 abajo a la derecha)
    pub fn round_rect_grad(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c0: Rgb, c1: Rgb, a: f32) {
        let (hw, hh) = (w / 2.0, h / 2.0);
        let r = r.min(hw).min(hh);
        for py in y.floor() as i32..(y + h).ceil() as i32 {
            for px in x.floor() as i32..(x + w).ceil() as i32 {
                let (fx, fy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                let qx = (fx - hw).abs() - (hw - r);
                let qy = (fy - hh).abs() - (hh - r);
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov > 0.0 {
                    let t = ((fx / w + fy / h) * 0.5).clamp(0.0, 1.0);
                    self.blend(px, py, c0.mix(c1, t), a * cov);
                }
            }
        }
    }

    /// Fondo de vidrio esmerilado sobre el desenfoque de DWM: tinte muy ligero, grano fino y
    /// borde con reflejo (más brillante arriba). Se calcula una vez por tamaño y se reutiliza.
    ///
    /// Estilos: "glass" (vidrio esmerilado), "clear" (cristal transparente), "acrylic" (acrílico de
    /// Windows), "mica" y "dark" (oscuro sólido). El desenfoque lo aporta DWM según el estilo.
    pub fn frosted(&mut self, sc: f32, style: &str) {
        let light = is_light();
        let key = format!("{style}{}", if light { ":light" } else { "" });
        if self.frosted.as_ref().is_some_and(|(st, _)| *st == key) {
            // se saca el fondo precalculado, se copia y se devuelve: sin duplicarlo (varios MB)
            if let Some((st, bg)) = self.frosted.take() {
                self.px().copy_from_slice(&bg);
                self.frosted = Some((st, bg));
            }
            return;
        }
        // (tinte, opacidad del tinte, luz superior, grano)
        // modo claro: tinte claro y más opaco para que el texto oscuro se lea sobre cualquier fondo
        let (tint, ta, hl, grain) = match (style, light) {
            ("clear", false) => (Rgb::u8(20, 24, 36), 0.10, 0.035, 0.6),
            ("acrylic", false) => (Rgb::u8(12, 14, 22), 0.16, 0.02, 0.0),
            ("mica", false) => (Rgb::u8(10, 12, 18), 0.08, 0.015, 0.0),
            ("dark", false) => (Rgb::u8(24, 26, 34), 0.97, 0.02, 0.0),
            (_, false) => (Rgb::u8(14, 16, 26), 0.30, 0.025, 1.0),
            ("clear", true) => (Rgb::u8(246, 248, 252), 0.30, 0.10, 0.6),
            ("acrylic", true) => (Rgb::u8(243, 243, 245), 0.40, 0.06, 0.0),
            ("mica", true) => (Rgb::u8(243, 243, 245), 0.22, 0.05, 0.0),
            ("dark", true) => (Rgb::u8(249, 249, 251), 0.97, 0.0, 0.0),
            (_, true) => (Rgb::u8(250, 251, 254), 0.50, 0.10, 1.0),
        };
        self.raw += 1;
        self.px().fill(0);
        let (w, h) = (self.w as f32, self.h as f32);
        self.fill(0.0, 0.0, w, h, tint, ta); // tinte
        self.fill(0.0, 0.0, w, h * 0.5, WHITE, hl); // luz suave en la mitad superior
        // grano (ruido estático determinista)
        for y in (0..self.h).filter(|_| grain > 0.0) {
            for x in 0..self.w {
                let mut n = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
                n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
                let v = ((n >> 16) & 0xFF) as f32 / 255.0 - 0.5;
                if v > 0.0 {
                    self.blend(x, y, WHITE, v * 0.045 * grain);
                } else {
                    self.blend(x, y, Rgb(0.0, 0.0, 0.0), -v * 0.05 * grain);
                }
            }
        }
        // borde de 1 px con reflejo: intenso arriba, tenue abajo
        let r = 8.0 * sc;
        let (hw, hh) = (w / 2.0, h / 2.0);
        for y in 0..self.h {
            let k = 1.0 - (y as f32 / h);
            for x in 0..self.w {
                let qx = (x as f32 + 0.5 - hw).abs() - (hw - r);
                let qy = (y as f32 + 0.5 - hh).abs() - (hh - r);
                let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
                let e = (1.0 - (d + 1.0).abs()).clamp(0.0, 1.0);
                if e > 0.0 {
                    if light {
                        self.blend(x, y, Rgb(0.0, 0.0, 0.0), e * (0.10 + 0.04 * (1.0 - k)));
                    } else {
                        self.blend(x, y, WHITE, e * (0.08 + 0.22 * k * k));
                    }
                }
            }
        }
        self.raw -= 1;
        let snapshot = self.px().to_vec();
        self.frosted = Some((key, snapshot));
    }

    /// Copia de los píxeles (BGRA premultiplicado), para pruebas
    pub fn pixels(&mut self) -> Vec<u32> {
        self.px().to_vec()
    }

    /// Zona (x, y, w, h) recortada al lienzo
    fn clip_rect(&self, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
        let (x0, y0) = (x.clamp(0, self.w), y.clamp(0, self.h));
        (x0, y0, (x + w).clamp(x0, self.w) - x0, (y + h).clamp(y0, self.h) - y0)
    }

    /// Copia de los píxeles de una zona, para restaurarla después con `put_rect`: permite repintar
    /// solo una parte animada sin redibujar todo lo que hay debajo
    pub fn copy_rect(&mut self, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32, Vec<u32>) {
        let (x0, y0, cw, ch) = self.clip_rect(x, y, w, h);
        let sw = self.w;
        let px = self.px();
        let mut out = Vec::with_capacity((cw * ch) as usize);
        for row in y0..y0 + ch {
            let i = (row * sw + x0) as usize;
            out.extend_from_slice(&px[i..i + cw as usize]);
        }
        (x0, y0, cw, ch, out)
    }

    /// Restaura una zona guardada con `copy_rect` (copia directa, sin mezcla)
    pub fn put_rect(&mut self, x: i32, y: i32, w: i32, h: i32, src: &[u32]) {
        if x < 0 || y < 0 || x + w > self.w || y + h > self.h || src.len() != (w * h) as usize {
            return;
        }
        let sw = self.w;
        let px = self.px();
        for row in 0..h {
            let i = ((y + row) * sw + x) as usize;
            px[i..i + w as usize].copy_from_slice(&src[(row * w) as usize..((row + 1) * w) as usize]);
        }
    }

    /// Pega una imagen BGRA premultiplicada (sw×sh) en (x, y) con mezcla "over" (para composiciones)
    pub fn paste(&mut self, x: i32, y: i32, src: &[u32], sw: i32, sh: i32) {
        for sy in 0..sh {
            for sx in 0..sw {
                let v = src[(sy * sw + sx) as usize];
                let a = ((v >> 24) & 0xFF) as f32 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                let c = Rgb(((v >> 16) & 0xFF) as f32 / a, ((v >> 8) & 0xFF) as f32 / a, (v & 0xFF) as f32 / a);
                self.blend_raw(x + sx, y + sy, c, a);
            }
        }
    }

    /// Fondo degradado opaco (para imágenes de demostración)
    pub fn gradient_bg(&mut self, a: Rgb, b: Rgb) {
        let (w, h) = (self.w as f32, self.h as f32);
        for y in 0..self.h {
            for x in 0..self.w {
                let t = ((x as f32 / w) * 0.7 + (y as f32 / h) * 0.3).clamp(0.0, 1.0);
                let c = a.mix(b, t);
                let i = (y * self.w + x) as usize;
                self.px()[i] = (c.2 as u32) | (c.1 as u32) << 8 | (c.0 as u32) << 16 | 0xFF00_0000;
            }
        }
    }

    /// Copia el lienzo a un DC de ventana (pintado normal, con alfa para el marco extendido de DWM)
    pub fn blit(&self, hdc: HDC) {
        unsafe {
            let _ = BitBlt(hdc, 0, 0, self.w, self.h, self.main.dc, 0, 0, SRCCOPY);
        }
    }

    /// Línea de tendencia sin relleno (p. ej. temperatura superpuesta a la gráfica de uso)
    #[allow(clippy::too_many_arguments)]
    pub fn trend_line(&mut self, x: f32, y: f32, w: f32, h: f32, data: &[f32], maxv: f32, c: Rgb, a: f32) {
        let n = data.len();
        if n < 2 || w < 2.0 || h < 2.0 {
            return;
        }
        let at = |u: f32| -> f32 {
            let t = u * (n - 1) as f32;
            let i = (t.floor() as usize).min(n - 2);
            let f = t - i as f32;
            ((data[i] + (data[i + 1] - data[i]) * f) / maxv).clamp(0.0, 1.0)
        };
        let bottom = y + h;
        let mut prev = bottom - at(0.0) * (h - 1.0);
        for px in 0..w.round() as i32 {
            let u = px as f32 / (w - 1.0);
            let v = at(u);
            if v <= 0.0 {
                prev = bottom;
                continue;
            }
            let top = bottom - v * (h - 1.0);
            // cubrir el tramo vertical entre muestras para que la línea no se corte en pendientes
            let (y0, y1) = (top.min(prev) - 1.0, top.max(prev) + 1.0);
            let xx = x.round() as i32 + px;
            for py in y0.floor() as i32..y1.ceil() as i32 {
                let fy = py as f32 + 0.5;
                let d = if fy < top.min(prev) { top.min(prev) - fy } else if fy > top.max(prev) { fy - top.max(prev) } else { 0.0 };
                let e = (1.0 - d / 0.9).clamp(0.0, 1.0);
                self.blend(xx, py, c, a * e * (u * 3.0).min(1.0));
            }
            prev = top;
        }
    }

    /// Anillo de progreso (donut) moderno: pista tenue, arco con degradado c1→c2 desde arriba en
    /// sentido horario, extremos redondeados y un halo suave del color del arco
    #[allow(clippy::too_many_arguments)]
    pub fn donut(&mut self, cx: f32, cy: f32, r: f32, thick: f32, frac: f32, c1: Rgb, c2: Rgb, glow: f32) {
        use std::f32::consts::TAU;
        let frac = frac.clamp(0.0, 1.0);
        let rm = r - thick / 2.0; // radio medio de la banda
        let half = thick / 2.0;
        // extremos del arco (para las tapas redondeadas)
        let end_ang = frac * TAU;
        let (sx, sy) = (cx, cy - rm);
        let (ex, ey) = (cx + rm * end_ang.sin(), cy - rm * end_ang.cos());
        let ext = r + glow + 1.0;
        for py in (cy - ext).floor() as i32..(cy + ext).ceil() as i32 {
            for px in (cx - ext).floor() as i32..(cx + ext).ceil() as i32 {
                let (dx, dy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
                let d = (dx * dx + dy * dy).sqrt();
                let band = (d - rm).abs() - half; // < 0 dentro de la banda
                // ángulo desde arriba en sentido horario, en [0, 1)
                let a = (dx.atan2(-dy) / TAU).rem_euclid(1.0);
                let in_arc = frac > 0.0 && a <= frac;
                // distancia a las tapas redondeadas
                let cap = (((px as f32 + 0.5 - sx).powi(2) + (py as f32 + 0.5 - sy).powi(2)).sqrt() - half)
                    .min(((px as f32 + 0.5 - ex).powi(2) + (py as f32 + 0.5 - ey).powi(2)).sqrt() - half);
                let arc_d = if in_arc { band } else if frac > 0.0 { cap } else { f32::MAX };
                let col = c1.mix(c2, a.min(frac) / frac.max(0.001));
                // pista
                let tcov = (0.5 - band).clamp(0.0, 1.0);
                if tcov > 0.0 {
                    self.blend(px, py, WHITE, 0.10 * tcov);
                }
                // arco
                let acov = (0.5 - arc_d).clamp(0.0, 1.0);
                if acov > 0.0 {
                    self.blend(px, py, col, acov);
                } else if glow > 0.0 && arc_d < glow * 2.5 {
                    self.blend(px, py, col, 0.30 * (-(arc_d / glow).powi(2)).exp());
                }
            }
        }
    }

    /// Segmento con extremos redondeados (grosor `width`)
    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Rgb, a: f32) {
        let r = width / 2.0;
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        for py in (y0.min(y1) - r - 1.0).floor() as i32..(y0.max(y1) + r + 1.0).ceil() as i32 {
            for px in (x0.min(x1) - r - 1.0).floor() as i32..(x0.max(x1) + r + 1.0).ceil() as i32 {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let t = (((fx - x0) * dx + (fy - y0) * dy) / len2).clamp(0.0, 1.0);
                let d = ((fx - x0 - t * dx).powi(2) + (fy - y0 - t * dy).powi(2)).sqrt() - r;
                self.blend(px, py, c, a * (0.5 - d).clamp(0.0, 1.0));
            }
        }
    }

    /// Anillo elíptico rotado (radios rx, ry; ángulo en radianes; trazo `stroke`)
    #[allow(clippy::too_many_arguments)]
    pub fn ellipse_ring(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, angle: f32, stroke: f32, c: Rgb, a: f32) {
        let (s, co) = angle.sin_cos();
        let ext = rx.max(ry) + stroke + 1.0;
        for py in (cy - ext).floor() as i32..(cy + ext).ceil() as i32 {
            for px in (cx - ext).floor() as i32..(cx + ext).ceil() as i32 {
                let (dx, dy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
                let (u, v) = (dx * co + dy * s, -dx * s + dy * co);
                // distancia aproximada a la elipse: (|p/r| - 1) · radio menor
                let k = ((u / rx).powi(2) + (v / ry).powi(2)).sqrt();
                let d = (k - 1.0).abs() * rx.min(ry) - stroke / 2.0;
                self.blend(px, py, c, a * (0.5 - d).clamp(0.0, 1.0));
            }
        }
    }

    /// Punto con halo (indicadores de estado)
    pub fn dot(&mut self, cx: f32, cy: f32, r: f32, c: Rgb, a: f32, glow_r: f32, glow_a: f32) {
        self.capsule(cx, cy - r, cy + r, r, c, a, glow_r, glow_a, 0.35);
    }

    /// Publica el contenido en la ventana con alfa por píxel
    pub fn present(&self, hwnd: HWND) {
        unsafe {
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(hwnd, HDC::default(), None, Some(&size), self.main.dc, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        }
    }
}
