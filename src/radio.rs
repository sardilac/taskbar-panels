//! Radio por Internet: catálogo de emisoras por país (directorio comunitario Radio Browser,
//! api.radio-browser.info) y reproducción con el reproductor multimedia de Windows (WinRT
//! `MediaPlayer`: MP3, AAC y HLS). Primero se elige el país (América o Europa) y después la
//! emisora. Estado, última emisora, volumen y catálogos se guardan en
//! %APPDATA%\TaskbarPanels\radio.json.
//!
//! Para añadir un continente basta con agregarlo a `CONTINENTS` (nombre es/en y países ISO).

use crate::i18n::tr;
use crate::util::{appdata_dir, now_epoch};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use windows::core::HSTRING;
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Media::Core::MediaSource;
use windows::Media::Playback::*;

/// País del selector: código ISO alfa-2 y nombre en español e inglés
type Country = (&'static str, &'static str, &'static str);

/// Continentes del selector de países: nombre en español e inglés y sus países
pub const CONTINENTS: [(&str, &str, &[Country]); 2] = [
    (
        "América",
        "Americas",
        &[
            ("AG", "Antigua y Barbuda", "Antigua and Barbuda"), ("AR", "Argentina", "Argentina"), ("BS", "Bahamas", "Bahamas"), ("BB", "Barbados", "Barbados"), ("BZ", "Belice", "Belize"),
            ("BO", "Bolivia", "Bolivia"), ("BR", "Brasil", "Brazil"), ("CA", "Canadá", "Canada"), ("CL", "Chile", "Chile"), ("CO", "Colombia", "Colombia"),
            ("CR", "Costa Rica", "Costa Rica"), ("CU", "Cuba", "Cuba"), ("DM", "Dominica", "Dominica"), ("DO", "República Dominicana", "Dominican Republic"), ("EC", "Ecuador", "Ecuador"),
            ("SV", "El Salvador", "El Salvador"), ("GD", "Granada", "Grenada"), ("GT", "Guatemala", "Guatemala"), ("GY", "Guyana", "Guyana"), ("HT", "Haití", "Haiti"),
            ("HN", "Honduras", "Honduras"), ("JM", "Jamaica", "Jamaica"), ("MX", "México", "Mexico"), ("NI", "Nicaragua", "Nicaragua"), ("PA", "Panamá", "Panama"),
            ("PY", "Paraguay", "Paraguay"), ("PE", "Perú", "Peru"), ("PR", "Puerto Rico", "Puerto Rico"), ("KN", "San Cristóbal y Nieves", "Saint Kitts and Nevis"),
            ("LC", "Santa Lucía", "Saint Lucia"), ("VC", "San Vicente y las Granadinas", "Saint Vincent and the Grenadines"), ("SR", "Surinam", "Suriname"),
            ("TT", "Trinidad y Tobago", "Trinidad and Tobago"), ("US", "Estados Unidos", "United States"), ("UY", "Uruguay", "Uruguay"), ("VE", "Venezuela", "Venezuela"),
        ],
    ),
    (
        "Europa",
        "Europe",
        &[
            ("AL", "Albania", "Albania"), ("DE", "Alemania", "Germany"), ("AD", "Andorra", "Andorra"), ("AT", "Austria", "Austria"), ("BY", "Bielorrusia", "Belarus"),
            ("BE", "Bélgica", "Belgium"), ("BA", "Bosnia y Herzegovina", "Bosnia and Herzegovina"), ("BG", "Bulgaria", "Bulgaria"), ("CY", "Chipre", "Cyprus"),
            ("VA", "Ciudad del Vaticano", "Vatican City"), ("HR", "Croacia", "Croatia"), ("DK", "Dinamarca", "Denmark"), ("SK", "Eslovaquia", "Slovakia"), ("SI", "Eslovenia", "Slovenia"),
            ("ES", "España", "Spain"), ("EE", "Estonia", "Estonia"), ("FI", "Finlandia", "Finland"), ("FR", "Francia", "France"), ("GR", "Grecia", "Greece"), ("HU", "Hungría", "Hungary"),
            ("IE", "Irlanda", "Ireland"), ("IS", "Islandia", "Iceland"), ("IT", "Italia", "Italy"), ("XK", "Kosovo", "Kosovo"), ("LV", "Letonia", "Latvia"),
            ("LI", "Liechtenstein", "Liechtenstein"), ("LT", "Lituania", "Lithuania"), ("LU", "Luxemburgo", "Luxembourg"), ("MK", "Macedonia del Norte", "North Macedonia"),
            ("MT", "Malta", "Malta"), ("MD", "Moldavia", "Moldova"), ("MC", "Mónaco", "Monaco"), ("ME", "Montenegro", "Montenegro"), ("NO", "Noruega", "Norway"),
            ("NL", "Países Bajos", "Netherlands"), ("PL", "Polonia", "Poland"), ("PT", "Portugal", "Portugal"), ("GB", "Reino Unido", "United Kingdom"),
            ("CZ", "República Checa", "Czech Republic"), ("RO", "Rumanía", "Romania"), ("RU", "Rusia", "Russia"), ("SM", "San Marino", "San Marino"), ("RS", "Serbia", "Serbia"),
            ("SE", "Suecia", "Sweden"), ("CH", "Suiza", "Switzerland"), ("TR", "Turquía", "Turkey"), ("UA", "Ucrania", "Ukraine"),
        ],
    ),
];
/// emisoras por país (las más escuchadas); el buscador llega al resto del directorio
const PER_COUNTRY: usize = 160;
/// versión del catálogo guardado: al cambiar cómo se arma, el anterior se descarta
const CATALOG_V: u32 = 3;
/// el catálogo guardado de un país se renueva pasado un día; se conservan los 15 más recientes
const LIST_TTL: i64 = 86_400;
const KEEP_LISTS: usize = 15;
const HOSTS: [&str; 2] = ["de1", "all"];
/// en pausa desde los controles multimedia: pasado este tiempo sin reanudar se cierra la emisora
/// (y con ella su sesión multimedia, así que el panel multimedia se oculta)
const PAUSE_CLOSE: Duration = Duration::from_secs(60);

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub url: String,
    pub country: String,
    /// ciudad, estado o provincia
    pub place: String,
    pub tags: String,
    pub codec: String,
    pub bitrate: u32,
    pub homepage: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Status {
    #[default]
    Stopped,
    Connecting,
    Playing,
    Error,
}

/// Lo que se muestra de la emisora actual (panel y ventana flotante)
#[derive(Clone, Default)]
pub struct Now {
    pub status: Status,
    pub station: Option<Station>,
    pub error: String,
    /// 0..1
    pub volume: f32,
    /// desde cuándo está al aire
    pub since: Option<Instant>,
}

impl Now {
    pub fn on_air(&self) -> bool {
        self.status == Status::Playing
    }
}

#[derive(Default)]
pub struct Radio {
    pub now: Now,
    /// índice en `CONTINENTS` de la pestaña del selector de países
    pub continent: usize,
    /// país elegido (ISO); vacío = se muestra el selector de países
    pub country: String,
    /// catálogo por país
    pub lists: HashMap<String, Vec<Station>>,
    pub loading: HashSet<String>,
    /// países cuya última descarga falló (sin conexión)
    pub failed: HashSet<String>,
    fetched_at: HashMap<String, i64>,
    /// texto del cuadro de búsqueda y lo encontrado en el directorio (fuera del catálogo guardado)
    pub query: String,
    pub found: Vec<Station>,
    pub searching: bool,
    search_seq: u64,
}

pub static RADIO: LazyLock<Mutex<Radio>> = LazyLock::new(|| Mutex::new(Radio { now: Now { volume: 0.8, ..Default::default() }, ..Default::default() }));

pub fn radio() -> MutexGuard<'static, Radio> {
    RADIO.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now() -> Now {
    radio().now.clone()
}

// ---------------------------------------------------------------- países
/// Minúsculas sin tildes, para comparar nombres ("olimpica" encuentra "Olímpica")
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

/// Países de un continente (código, nombre en el idioma de la interfaz), por orden alfabético
fn countries(continent: usize) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = CONTINENTS[continent].2.iter().map(|c| (c.0.to_string(), tr(c.1, c.2).to_string())).collect();
    v.sort_by_key(|x| fold(&x.1));
    v
}

/// Nombre del país (o el código si no es de los continentes disponibles)
pub fn country_label(code: &str) -> String {
    CONTINENTS.iter().flat_map(|c| c.2.iter()).find(|c| c.0 == code).map(|c| tr(c.1, c.2).to_string()).unwrap_or_else(|| code.to_string())
}

/// Países que muestra el selector: los del continente elegido o, con búsqueda, los que coinciden
/// en cualquiera de los continentes
pub fn visible_countries() -> Vec<(String, String)> {
    let (continent, q) = {
        let r = radio();
        (r.continent.min(CONTINENTS.len() - 1), fold(r.query.trim()))
    };
    if q.is_empty() {
        return countries(continent);
    }
    (0..CONTINENTS.len()).flat_map(countries).filter(|c| fold(&c.1).contains(&q) || c.0.to_lowercase() == q).collect()
}

pub fn set_continent(i: usize) {
    if i < CONTINENTS.len() {
        let mut r = radio();
        r.continent = i;
        save(&r);
    }
}

/// Elige el país (se descarga su catálogo) o, con "", vuelve al selector de países
pub fn set_country(code: &str) {
    {
        let mut r = radio();
        r.country = code.to_string();
        if let Some(i) = CONTINENTS.iter().position(|c| c.2.iter().any(|x| x.0 == code)) {
            r.continent = i;
        }
        save(&r);
    }
    set_query("");
    if !code.is_empty() {
        ensure_list(code);
    }
}

// ---------------------------------------------------------------- radio.json
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct SavedList {
    at: i64,
    stations: Vec<Station>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    v: u32,
    /// versiones anteriores: región ("co", "us", "eu")
    region: String,
    continent: usize,
    country: String,
    volume: f32,
    last: Option<Station>,
    lists: HashMap<String, SavedList>,
}

fn load() {
    let Some(s) = std::fs::read_to_string(appdata_dir().join("radio.json")).ok().and_then(|s| serde_json::from_str::<Saved>(&s).ok()) else { return };
    let mut r = radio();
    if s.volume > 0.0 {
        r.now.volume = s.volume.clamp(0.05, 1.0);
    }
    r.now.station = s.last;
    if s.v == CATALOG_V {
        r.continent = s.continent.min(CONTINENTS.len() - 1);
        r.country = s.country;
        for (cc, l) in s.lists {
            r.fetched_at.insert(cc.clone(), l.at);
            r.lists.insert(cc, l.stations);
        }
    } else {
        // versión por regiones: se conserva el país que equivalía a la región elegida
        match s.region.as_str() {
            "co" => r.country = "CO".into(),
            "us" => r.country = "US".into(),
            "eu" => r.continent = 1,
            _ => {}
        }
    }
}

fn save(r: &Radio) {
    // solo los catálogos más recientes: el archivo no crece con cada país visitado
    let mut recent: Vec<(&String, &i64)> = r.fetched_at.iter().collect();
    recent.sort_by_key(|x| std::cmp::Reverse(*x.1));
    let lists = recent.into_iter().take(KEEP_LISTS).filter_map(|(cc, at)| Some((cc.clone(), SavedList { at: *at, stations: r.lists.get(cc)?.clone() }))).collect();
    let s = Saved { v: CATALOG_V, region: String::new(), continent: r.continent, country: r.country.clone(), volume: r.now.volume, last: r.now.station.clone(), lists };
    if let Ok(j) = serde_json::to_string(&s) {
        let _ = std::fs::write(appdata_dir().join("radio.json"), j);
    }
}

// ---------------------------------------------------------------- catálogo
/// Nombre corto: el directorio a veces trae "Emisora - 100.9 FM - Grupo - Ciudad, País"; con solo
/// dos partes ("Olímpica Stereo - Barranquilla") se conserva entero, que la ciudad distingue
fn short_name(n: &str) -> String {
    let n = n.trim();
    let first = n.split(" - ").next().unwrap_or(n).trim();
    let s = if n.split(" - ").count() >= 3 && first.chars().count() >= 3 { first } else { n };
    s.chars().take(60).collect()
}

/// Emisora del directorio → `Station`; None si Windows no la reproduce (sin URL http o en OGG)
fn parse(v: &Value) -> Option<Station> {
    let g = |k: &str| v[k].as_str().unwrap_or_default().trim().to_string();
    let url = if g("url_resolved").is_empty() { g("url") } else { g("url_resolved") };
    let codec = g("codec");
    let name = short_name(&g("name"));
    if !url.starts_with("http") || name.is_empty() || codec.to_ascii_uppercase().contains("OGG") {
        return None;
    }
    let tags = g("tags").split(',').map(|t| t.trim()).filter(|t| !t.is_empty()).take(3).collect::<Vec<_>>().join(", ");
    Some(Station { id: g("stationuuid"), name, url, country: g("country"), place: g("state"), tags, codec, bitrate: v["bitrate"].as_u64().unwrap_or(0) as u32, homepage: g("homepage") })
}

/// Consulta al directorio (probando los servidores en orden), de más a menos escuchadas
fn query(a: &ureq::Agent, params: &[(&str, &str)]) -> Option<Vec<Value>> {
    HOSTS.iter().find_map(|host| {
        let mut req = a.get(&format!("https://{host}.api.radio-browser.info/json/stations/search")).query("hidebroken", "true").query("order", "clickcount").query("reverse", "true");
        for (k, v) in params {
            req = req.query(k, v);
        }
        req.call().ok()?.into_json::<Value>().ok()?.as_array().cloned()
    })
}

/// Las emisoras más escuchadas del país; None sin conexión (una lista vacía es un país sin emisoras)
fn fetch(code: &str) -> Option<Vec<Station>> {
    // se piden de más: hay emisoras repetidas (varios flujos) y formatos que Windows no reproduce
    let list = query(&crate::state::agent(), &[("countrycode", code), ("limit", &(PER_COUNTRY * 2).to_string())])?;
    let mut seen = HashSet::new();
    Some(list.iter().filter_map(parse).filter(|s| seen.insert(fold(&s.name)) && seen.insert(s.url.clone())).take(PER_COUNTRY).collect())
}

/// Busca por nombre en todo el directorio del país
fn search_remote(code: &str, q: &str) -> Vec<Station> {
    let list = query(&crate::state::agent(), &[("name", q), ("countrycode", code), ("limit", "300")]).unwrap_or_default();
    let mut seen = HashSet::new();
    list.iter().filter_map(parse).filter(|s| seen.insert(s.url.clone())).take(80).collect()
}

/// Descarga el catálogo del país si falta o está viejo (en segundo plano; no bloquea)
pub fn ensure_list(code: &str) {
    if code.is_empty() {
        return;
    }
    {
        let mut r = radio();
        let fresh = r.lists.contains_key(code) && now_epoch() - r.fetched_at.get(code).copied().unwrap_or(0) < LIST_TTL;
        if fresh || !r.loading.insert(code.to_string()) {
            return;
        }
    }
    let code = code.to_string();
    std::thread::spawn(move || {
        let got = fetch(&code);
        let mut r = radio();
        r.loading.remove(&code);
        match got {
            Some(l) => {
                r.failed.remove(&code);
                r.lists.insert(code.clone(), l);
                r.fetched_at.insert(code, now_epoch());
                save(&r);
            }
            None => {
                r.failed.insert(code);
            }
        }
    });
}

/// Cambia el texto de búsqueda. En el selector filtra los países; con un país elegido filtra su
/// catálogo al instante (ver `visible`) y, con dos letras o más, tras una pausa de 0,45 s consulta
/// el directorio completo del país
pub fn set_query(q: &str) {
    let (seq, code, q) = {
        let mut r = radio();
        r.query = q.to_string();
        r.found.clear();
        r.search_seq += 1;
        let q = q.trim().to_string();
        r.searching = !r.country.is_empty() && q.chars().count() >= 2;
        if !r.searching {
            return;
        }
        (r.search_seq, r.country.clone(), q)
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(450));
        if radio().search_seq != seq {
            return; // se siguió escribiendo
        }
        let found = search_remote(&code, &q);
        let mut r = radio();
        if r.search_seq == seq {
            r.found = found;
            r.searching = false;
        }
    });
}

/// Emisoras que muestra la lista: el catálogo del país o, con búsqueda, las que coinciden en el
/// catálogo seguidas de las encontradas en el directorio
pub fn visible() -> Vec<Station> {
    let r = radio();
    let empty = Vec::new();
    let list = r.lists.get(&r.country).unwrap_or(&empty);
    let q = fold(r.query.trim());
    if q.is_empty() {
        return list.clone();
    }
    let hit = |s: &&Station| fold(&s.name).contains(&q) || fold(&s.place).contains(&q) || fold(&s.tags).contains(&q);
    let mut out: Vec<Station> = list.iter().filter(hit).cloned().collect();
    for s in &r.found {
        if !out.iter().any(|o| o.url == s.url) {
            out.push(s.clone());
        }
    }
    out
}

// ---------------------------------------------------------------- órdenes
enum Cmd {
    Play(Station, f32),
    Stop,
    Volume(f32),
}
static CMD: LazyLock<Mutex<Option<Sender<Cmd>>>> = LazyLock::new(|| Mutex::new(None));

fn send(c: Cmd) {
    if let Some(tx) = CMD.lock().unwrap().as_ref() {
        let _ = tx.send(c);
    }
}

pub fn play(s: Station) {
    let vol = {
        let mut r = radio();
        r.now.status = Status::Connecting;
        r.now.error.clear();
        r.now.since = None;
        r.now.station = Some(s.clone());
        save(&r);
        r.now.volume
    };
    // cortesía con el directorio: cuenta la escucha (así ordena las emisoras por popularidad)
    if !s.id.is_empty() {
        let id = s.id.clone();
        std::thread::spawn(move || {
            let _ = crate::state::agent().get(&format!("https://{}.api.radio-browser.info/json/url/{id}", HOSTS[0])).call();
        });
    }
    send(Cmd::Play(s, vol));
}

/// Reproduce la emisora `i` de la lista visible (país elegido, con el filtro de búsqueda)
pub fn play_index(i: usize) {
    if let Some(s) = visible().get(i).cloned() {
        play(s);
    }
}

pub fn stop() {
    {
        let mut r = radio();
        r.now.status = Status::Stopped;
        r.now.since = None;
    }
    OPEN.store(false, Ordering::Relaxed);
    send(Cmd::Stop);
}

/// Hay una emisora abierta (sonando, conectando o en pausa desde los controles multimedia)
static OPEN: AtomicBool = AtomicBool::new(false);

/// La sesión multimedia de Windows con este título y artista es la de la radio, pero la emisora
/// ya se detuvo: Windows puede conservar la sesión un tiempo y el panel multimedia no debe mostrarla
pub fn stale_session(title: &str, artist: &str, own_app: bool) -> bool {
    if OPEN.load(Ordering::Relaxed) {
        return false;
    }
    let named = (artist == "Radio" || artist.starts_with("Radio · ")) && radio().now.station.as_ref().is_some_and(|s| s.name == title);
    own_app || named
}

/// Detiene si suena (o está conectando); si no, vuelve a la última emisora
pub fn toggle() {
    let n = now();
    match n.status {
        Status::Playing | Status::Connecting => stop(),
        _ => {
            if let Some(s) = n.station {
                play(s);
            }
        }
    }
}

pub fn volume_step(d: f32) {
    let v = {
        let mut r = radio();
        r.now.volume = ((r.now.volume + d) * 20.0).round().clamp(1.0, 20.0) / 20.0;
        save(&r);
        r.now.volume
    };
    send(Cmd::Volume(v));
}

// ---------------------------------------------------------------- reproducción
struct Player {
    mp: MediaPlayer,
    failed: Arc<Mutex<Option<String>>>,
    started: Instant,
    was_playing: bool,
    /// desde cuándo está en pausa (controles multimedia de Windows)
    paused_at: Option<Instant>,
}

fn open(s: &Station, vol: f32) -> windows::core::Result<Player> {
    let mp = MediaPlayer::new()?;
    let uri = Uri::CreateUri(&HSTRING::from(s.url.as_str()))?;
    let item = MediaPlaybackItem::Create(&MediaSource::CreateFromUri(&uri)?)?;
    // nombre de la emisora en los controles multimedia de Windows (y en el panel multimedia)
    if let Ok(props) = item.GetDisplayProperties() {
        let _ = props.SetType(windows::Media::MediaPlaybackType::Music);
        if let Ok(m) = props.MusicProperties() {
            let _ = m.SetTitle(&HSTRING::from(s.name.as_str()));
            let _ = m.SetArtist(&HSTRING::from(if s.country.is_empty() { "Radio".to_string() } else { format!("Radio · {}", s.country) }));
        }
        let _ = item.ApplyDisplayProperties(&props);
    }
    let failed = Arc::new(Mutex::new(None));
    let flag = failed.clone();
    mp.MediaFailed(&TypedEventHandler::<MediaPlayer, MediaPlayerFailedEventArgs>::new(move |_, args| {
        let msg = match args.as_ref().and_then(|a| a.Error().ok()) {
            Some(MediaPlayerError::NetworkError) => tr("Sin conexión con la emisora", "Cannot reach the station"),
            Some(MediaPlayerError::SourceNotSupported) | Some(MediaPlayerError::DecodingError) => tr("Formato no compatible", "Unsupported format"),
            _ => tr("No se pudo reproducir", "Playback failed"),
        };
        *flag.lock().unwrap() = Some(msg.to_string());
        Ok(())
    }))?;
    mp.SetVolume(vol as f64)?;
    mp.SetSource(&item)?;
    mp.Play()?;
    Ok(Player { mp, failed, started: Instant::now(), was_playing: false, paused_at: None })
}

/// Refleja el estado del reproductor en `RADIO`; devuelve false si hay que cerrarlo
fn poll(p: &mut Player) -> bool {
    let state = p.mp.PlaybackSession().and_then(|s| s.PlaybackState()).unwrap_or(MediaPlaybackState::None);
    let err = p.failed.lock().unwrap().take();
    let mut r = radio();
    let fail = |r: &mut Radio, msg: String| {
        r.now.status = Status::Error;
        r.now.error = msg;
        r.now.since = None;
        false
    };
    if let Some(e) = err {
        return fail(&mut r, e);
    }
    match state {
        MediaPlaybackState::Playing => {
            if r.now.status != Status::Playing || r.now.since.is_none() {
                r.now.since = Some(Instant::now());
            }
            r.now.status = Status::Playing;
            p.was_playing = true;
            p.paused_at = None;
        }
        // en pausa desde los controles multimedia de Windows: se conserva para poder reanudar,
        // pero no indefinidamente
        MediaPlaybackState::Paused if p.was_playing => {
            r.now.status = Status::Stopped;
            r.now.since = None;
            if p.paused_at.get_or_insert_with(Instant::now).elapsed() >= PAUSE_CLOSE {
                return false;
            }
        }
        _ if !p.was_playing && p.started.elapsed() > Duration::from_secs(25) => {
            return fail(&mut r, tr("La emisora no responde", "The station is not responding").to_string());
        }
        _ => r.now.status = Status::Connecting,
    }
    true
}

pub fn spawn_worker() {
    load();
    let (tx, rx) = channel();
    *CMD.lock().unwrap() = Some(tx);
    std::thread::spawn(move || {
        crate::util::prof("radio");
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let mut player: Option<Player> = None;
        let close = |p: Option<Player>| {
            if let Some(p) = p {
                let _ = p.mp.Close();
            }
        };
        loop {
            // con una emisora abierta se sondea su estado; sin ella el hilo duerme hasta la próxima orden
            let wait = if player.is_some() { Duration::from_millis(400) } else { Duration::from_secs(3600) };
            match rx.recv_timeout(wait) {
                Ok(Cmd::Play(s, vol)) => {
                    close(player.take());
                    match open(&s, vol) {
                        Ok(p) => player = Some(p),
                        Err(_) => {
                            let mut r = radio();
                            r.now.status = Status::Error;
                            r.now.error = tr("No se pudo reproducir", "Playback failed").to_string();
                        }
                    }
                }
                Ok(Cmd::Stop) => close(player.take()),
                Ok(Cmd::Volume(v)) => {
                    if let Some(p) = &player {
                        let _ = p.mp.SetVolume(v as f64);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if player.as_mut().is_some_and(|p| !poll(p)) {
                close(player.take());
            }
            OPEN.store(player.is_some(), Ordering::Relaxed);
        }
    });
}

/// Datos de ejemplo para las imágenes de demostración (`--render-demo`)
pub fn demo() {
    let mk = |name: &str, place: &str, tags: &str, codec: &str, bitrate: u32| Station {
        name: name.into(),
        url: format!("demo://{name}"),
        country: "Colombia".into(),
        place: place.into(),
        tags: tags.into(),
        codec: codec.into(),
        bitrate,
        ..Default::default()
    };
    let list = vec![
        mk("Radio Andina", "Bogotá", "noticias, hablada", "AAC+", 64),
        mk("Tropical Estéreo", "Cali", "salsa, tropical", "MP3", 128),
        mk("La Montaña FM", "Medellín", "pop, éxitos", "MP3", 128),
        mk("Caribe 99.1", "Barranquilla", "vallenato, tropical", "AAC", 96),
        mk("Rock del Sur", "Pasto", "rock, clásicos", "MP3", 192),
        mk("Clásica 24", "Bogotá", "clásica, jazz", "MP3", 320),
        mk("Café y Noticias", "Manizales", "noticias", "AAC+", 48),
        mk("Sabana Estéreo", "Sincelejo", "popular, ranchera", "MP3", 96),
        mk("Llano Radio", "Villavicencio", "llanera, folclor", "MP3", 128),
        mk("Pacífico FM", "Buenaventura", "currulao, salsa", "AAC", 64),
        mk("Sierra Nevada Radio", "Santa Marta", "tropical, pop", "MP3", 128),
        mk("Eje Cafetero FM", "Pereira", "popular, noticias", "AAC+", 64),
    ];
    let mut r = radio();
    r.now = Now { status: Status::Playing, station: Some(list[1].clone()), error: String::new(), volume: 0.8, since: Instant::now().checked_sub(Duration::from_secs(754)) };
    r.continent = 0;
    r.country = "CO".into();
    r.lists.insert("CO".into(), list);
}

/// Imágenes de demostración: muestra el selector de países (true) o la lista de emisoras
pub fn demo_picker(on: bool) {
    radio().country = if on { String::new() } else { "CO".into() };
}

/// Diagnóstico (`--radio-report <archivo>`): países disponibles y, para algunos, descarga del
/// catálogo, búsqueda y prueba en silencio (volumen 0) de sus primeras emisoras
pub fn report() -> String {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let mut out = String::new();
    for (i, c) in CONTINENTS.iter().enumerate() {
        out += &format!("{} ({}): {}\n", c.0, c.2.len(), countries(i).iter().map(|x| format!("{} {}", x.0, x.1)).collect::<Vec<_>>().join(", "));
    }
    for code in ["CO", "MX", "VC", "DE", "XK"] {
        let Some(list) = fetch(code) else {
            out += &format!("{}: sin catálogo (sin conexión)\n", country_label(code));
            continue;
        };
        out += &format!("{}: {} emisoras\n", country_label(code), list.len());
        for s in list.iter().take(2) {
            radio().now = Now::default();
            let res = match open(s, 0.0) {
                Ok(mut p) => {
                    let t0 = Instant::now();
                    while poll(&mut p) && radio().now.status != Status::Playing && t0.elapsed() < Duration::from_secs(20) {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    let _ = p.mp.Close();
                    let n = now();
                    format!("{:?} {} ({:.1} s)", n.status, n.error, t0.elapsed().as_secs_f32())
                }
                Err(e) => format!("no se pudo abrir: {e}"),
            };
            out += &format!("  {} [{} {}k] {} → {res}\n", s.name, s.codec, s.bitrate, s.url);
        }
        if code == "CO" {
            for q in ["vibra", "olimpica"] {
                let cat: Vec<&str> = list.iter().filter(|s| fold(&s.name).contains(q)).map(|s| s.name.as_str()).collect();
                let dir: Vec<String> = search_remote(code, q).iter().take(8).map(|s| s.name.clone()).collect();
                out += &format!("  búsqueda «{q}»: catálogo: {} || directorio: {}\n", cat.join(" | "), dir.join(" | "));
            }
        }
    }
    out
}
