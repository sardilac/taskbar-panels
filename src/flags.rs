//! Bandera del país de la IP pública: se descarga una sola vez por país desde flagcdn.com
//! (PNG de 80 px de ancho) y se guarda en %APPDATA%\TaskbarPanels\flags\<cc>.png.

use std::sync::{Arc, LazyLock, Mutex};

/// Bandera decodificada (BGRA premultiplicado)
pub struct Flag {
    pub code: String,
    pub px: Vec<u32>,
    pub w: usize,
    pub h: usize,
}

pub static FLAG: LazyLock<Mutex<Option<Arc<Flag>>>> = LazyLock::new(|| Mutex::new(None));

/// Decodifica un PNG con WinRT (BitmapDecoder)
fn decode(bytes: &[u8]) -> Option<(Vec<u32>, usize, usize)> {
    use windows::Graphics::Imaging::*;
    use windows::Storage::Streams::*;
    let ms = InMemoryRandomAccessStream::new().ok()?;
    let wr = DataWriter::CreateDataWriter(&ms).ok()?;
    wr.WriteBytes(bytes).ok()?;
    wr.StoreAsync().ok()?.get().ok()?;
    wr.FlushAsync().ok()?.get().ok()?;
    wr.DetachStream().ok()?;
    ms.Seek(0).ok()?;
    let dec = BitmapDecoder::CreateAsync(&ms).ok()?.get().ok()?;
    let (w, h) = (dec.PixelWidth().ok()? as usize, dec.PixelHeight().ok()? as usize);
    let pd = dec
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &BitmapTransform::new().ok()?,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )
        .ok()?
        .get()
        .ok()?;
    let bytes = pd.DetachPixelData().ok()?;
    if w == 0 || h == 0 || bytes.len() < w * h * 4 {
        return None;
    }
    Some((bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(), w, h))
}

/// Carga (de la caché o de internet) la bandera del país `cc` (ISO 3166-1 alfa-2). Llamar desde un
/// hilo de trabajo con COM inicializado; no hace nada si ya está cargada.
pub fn ensure(a: &ureq::Agent, cc: &str) {
    let cc = cc.trim().to_ascii_lowercase();
    if cc.len() != 2 || !cc.bytes().all(|b| b.is_ascii_lowercase()) {
        *FLAG.lock().unwrap() = None;
        return;
    }
    if FLAG.lock().unwrap().as_ref().map(|f| f.code == cc).unwrap_or(false) {
        return;
    }
    let dir = crate::util::appdata_dir().join("flags");
    let path = dir.join(format!("{cc}.png"));
    let bytes = match std::fs::read(&path) {
        Ok(b) if !b.is_empty() => b,
        _ => {
            let Ok(r) = a.get(&format!("https://flagcdn.com/w80/{cc}.png")).call() else { return };
            let mut b = Vec::new();
            if std::io::Read::read_to_end(&mut r.into_reader(), &mut b).is_err() || b.is_empty() {
                return;
            }
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, &b);
            b
        }
    };
    if let Some((px, w, h)) = decode(&bytes) {
        *FLAG.lock().unwrap() = Some(Arc::new(Flag { code: cc, px, w, h }));
    }
}
