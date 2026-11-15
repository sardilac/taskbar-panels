//! Métricas de sistema: CPU (PDH), GPU y NPU (gpu.rs), temperaturas/ventiladores Lenovo (WMI),
//! FPS de la ventana en primer plano (PresentMon), batería y plan de energía.

use crate::state::{self, st};
use crate::util::{exe_dir, hidden_cmd};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use windows::core::{w, BSTR, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::*;
use windows::Win32::System::Performance::*;
use windows::core::VARIANT;
use windows::Win32::System::Wmi::*;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

// ---------------------------------------------------------------- CPU (PDH)
struct Cpu {
    query: isize,
    counter: isize,
    perf: isize,
    disk_r: isize,
    disk_w: isize,
    base_mhz: f32,
    name: String,
}

fn reg_cpu() -> (String, f32) {
    use windows::Win32::System::Registry::*;
    let key = w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0");
    unsafe {
        let mut buf = [0u16; 128];
        let mut sz = (buf.len() * 2) as u32;
        let name = if RegGetValueW(HKEY_LOCAL_MACHINE, key, w!("ProcessorNameString"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut sz)).is_ok() {
            String::from_utf16_lossy(&buf[..(sz as usize / 2).saturating_sub(1)]).trim().to_string()
        } else {
            String::new()
        };
        let mut mhz = 0u32;
        let mut sz = 4u32;
        let _ = RegGetValueW(HKEY_LOCAL_MACHINE, key, w!("~MHz"), RRF_RT_REG_DWORD, None, Some(&mut mhz as *mut u32 as *mut _), Some(&mut sz));
        (name.replace("(R)", "").replace("(TM)", ""), mhz as f32)
    }
}

impl Cpu {
    fn new() -> Option<Self> {
        unsafe {
            let mut q = 0isize;
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) != 0 {
                return None;
            }
            let mut c = 0isize;
            if PdhAddEnglishCounterW(q, w!("\\Processor Information(_Total)\\% Processor Utility"), 0, &mut c) != 0 {
                return None;
            }
            let mut p = 0isize;
            let _ = PdhAddEnglishCounterW(q, w!("\\Processor Information(_Total)\\% Processor Performance"), 0, &mut p);
            let (mut dr, mut dw) = (0isize, 0isize);
            let _ = PdhAddEnglishCounterW(q, w!("\\PhysicalDisk(_Total)\\Disk Read Bytes/sec"), 0, &mut dr);
            let _ = PdhAddEnglishCounterW(q, w!("\\PhysicalDisk(_Total)\\Disk Write Bytes/sec"), 0, &mut dw);
            PdhCollectQueryData(q);
            let (name, base_mhz) = reg_cpu();
            Some(Cpu { query: q, counter: c, perf: p, disk_r: dr, disk_w: dw, base_mhz, name })
        }
    }
    fn read(&self) -> f32 {
        unsafe {
            PdhCollectQueryData(self.query);
            let mut v = PDH_FMT_COUNTERVALUE::default();
            if PdhGetFormattedCounterValue(self.counter, PDH_FMT_DOUBLE, None, &mut v) != 0 {
                return -1.0;
            }
            (v.Anonymous.doubleValue as f32).clamp(0.0, 100.0)
        }
    }
    /// Lectura y escritura de disco en bytes/s (después de read(), que recoge la consulta)
    fn disk(&self) -> (f64, f64) {
        let get = |h: isize| unsafe {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            if h == 0 || PdhGetFormattedCounterValue(h, PDH_FMT_DOUBLE, None, &mut v) != 0 {
                0.0
            } else {
                v.Anonymous.doubleValue.max(0.0)
            }
        };
        (get(self.disk_r), get(self.disk_w))
    }

    /// Frecuencia actual estimada (MHz) = base × % de rendimiento (después de read())
    fn mhz(&self) -> f32 {
        unsafe {
            let mut v = PDH_FMT_COUNTERVALUE::default();
            if self.perf == 0 || PdhGetFormattedCounterValue(self.perf, PDH_FMT_DOUBLE, None, &mut v) != 0 {
                return 0.0;
            }
            self.base_mhz * v.Anonymous.doubleValue as f32 / 100.0
        }
    }
}

// ---------------------------------------------------------------- Lenovo (WMI, requiere admin)
struct Lenovo {
    svc: IWbemServices,
    path: BSTR,
    inparams: IWbemClassObject,
}

impl Lenovo {
    fn new() -> Option<Self> {
        unsafe {
            let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
            let svc = loc
                .ConnectServer(&BSTR::from("root\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None)
                .ok()?;
            CoSetProxyBlanket(&svc, 10, 0, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE).ok()?;
            let en = svc
                .ExecQuery(
                    &BSTR::from("WQL"),
                    &BSTR::from("SELECT * FROM LENOVO_OTHER_METHOD"),
                    WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                    None,
                )
                .ok()?;
            let mut objs = [None];
            let mut n = 0u32;
            let _ = en.Next(WBEM_INFINITE, &mut objs, &mut n);
            let inst = objs[0].take()?;
            let mut v = VARIANT::default();
            inst.Get(w!("__PATH"), 0, &mut v, None, None).ok()?;
            let path = BSTR::try_from(&v).ok()?;
            let mut class = None;
            svc.GetObject(&BSTR::from("LENOVO_OTHER_METHOD"), WBEM_GENERIC_FLAG_TYPE(0), None, Some(&mut class), None).ok()?;
            let mut insig = None;
            class?.GetMethod(w!("GetFeatureValue"), 0, &mut insig, std::ptr::null_mut()).ok()?;
            let inparams = insig?.SpawnInstance(0).ok()?;
            Some(Lenovo { svc, path, inparams })
        }
    }
    fn get(&self, id: i32) -> i32 {
        unsafe {
            let v = VARIANT::from(id);
            if self.inparams.Put(w!("IDs"), 0, &v, 0).is_err() {
                return -1;
            }
            let mut out = None;
            if self
                .svc
                .ExecMethod(&self.path, &BSTR::from("GetFeatureValue"), WBEM_GENERIC_FLAG_TYPE(0), None, &self.inparams, Some(&mut out), None)
                .is_err()
            {
                return -1;
            }
            let Some(out) = out else { return -1 };
            let mut r = VARIANT::default();
            if out.Get(w!("Value"), 0, &mut r, None, None).is_err() {
                return -1;
            }
            i32::try_from(&r).unwrap_or(-1)
        }
    }
}

// ---------------------------------------------------------------- FPS (PresentMon)
type PresentMap = Arc<Mutex<HashMap<u32, (VecDeque<Instant>, String)>>>;
static PRESENTMON: LazyLock<Mutex<Option<Child>>> = LazyLock::new(|| Mutex::new(None));

fn start_presentmon(map: PresentMap) {
    let exe = exe_dir().join("tools").join("PresentMon.exe");
    let child = hidden_cmd(&exe.to_string_lossy())
        .args([
            "--output_stdout", "--no_console_stats", "--stop_existing_session", "--session_name", "PanelFps",
            "--v1_metrics", "--exclude", "dwm.exe",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return };
    let Some(out) = child.stdout.take() else { return };
    *PRESENTMON.lock().unwrap() = Some(child);
    std::thread::spawn(move || {
        crate::util::prof("presentmon");
        let (mut pid_col, mut app_col) = (usize::MAX, 0usize);
        for line in BufReader::new(out).lines() {
            let Ok(line) = line else { break };
            let cols: Vec<&str> = line.split(',').collect();
            if pid_col == usize::MAX {
                pid_col = cols.iter().position(|c| *c == "ProcessID").unwrap_or(usize::MAX);
                app_col = cols.iter().position(|c| *c == "Application").unwrap_or(0);
                continue;
            }
            let Some(pid) = cols.get(pid_col).and_then(|p| p.parse::<u32>().ok()) else { continue };
            let mut m = map.lock().unwrap();
            let e = m.entry(pid).or_insert_with(|| (VecDeque::new(), cols.get(app_col).unwrap_or(&"").to_string()));
            e.0.push_back(Instant::now());
            if e.0.len() > 2000 {
                e.0.pop_front();
            }
        }
    });
}

fn presentmon_running() -> bool {
    match PRESENTMON.lock().unwrap().as_mut() {
        Some(c) => matches!(c.try_wait(), Ok(None)),
        None => false,
    }
}

pub fn stop_presentmon() {
    if let Some(mut c) = PRESENTMON.lock().unwrap().take() {
        let _ = c.kill();
    }
}

/// FPS del proceso indicado: presents en el último segundo (y limpieza de procesos inactivos)
fn fps_of(map: &PresentMap, pid: u32) -> (u32, String) {
    let mut m = map.lock().unwrap();
    let now = Instant::now();
    m.retain(|_, (q, _)| {
        while q.front().map(|t| now.duration_since(*t) > Duration::from_secs(1)).unwrap_or(false) {
            q.pop_front();
        }
        !q.is_empty()
    });
    m.get(&pid).map(|(q, n)| (q.len() as u32, n.clone())).unwrap_or((0, String::new()))
}

// ---------------------------------------------------------------- Unidades fijas
fn scan_drives() -> Vec<crate::state::Drive> {
    use windows::Win32::Storage::FileSystem::*;
    let mut out = Vec::new();
    for l in b'A'..=b'Z' {
        let root: Vec<u16> = format!("{}:\\", l as char).encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            if GetDriveTypeW(PCWSTR(root.as_ptr())) != 3 {
                continue; // solo DRIVE_FIXED
            }
            let (mut free, mut total) = (0u64, 0u64);
            if GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), None, Some(&mut total), Some(&mut free)).is_err() || total == 0 {
                continue;
            }
            let mut name = [0u16; 64];
            let _ = GetVolumeInformationW(PCWSTR(root.as_ptr()), Some(&mut name), None, None, None, None);
            let n = name.iter().position(|c| *c == 0).unwrap_or(name.len());
            out.push(crate::state::Drive {
                letter: l as char,
                label: String::from_utf16_lossy(&name[..n]),
                total: total as f64,
                free: free as f64,
            });
        }
    }
    out
}

// ---------------------------------------------------------------- Energía (batería y plan)
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Battery {
    pub present: bool,
    pub pct: i32,
    pub ac: bool,
    pub charging: bool,
    pub secs: i32,
    pub watts: f32,
}

#[repr(C)]
#[derive(Default)]
struct SystemPowerStatus {
    ac: u8,
    flag: u8,
    pct: u8,
    saver: u8,
    life: u32,
    full: u32,
}

/// SYSTEM_BATTERY_STATE (CallNtPowerInformation, nivel SystemBatteryState = 5)
#[repr(C)]
#[derive(Default)]
struct BatteryState {
    ac_online: u8,
    present: u8,
    charging: u8,
    discharging: u8,
    spare: [u8; 3],
    tag: u8,
    max_capacity: u32,
    remaining: u32,
    /// mW; negativo al descargar
    rate: i32,
    estimated_time: u32,
    alert1: u32,
    alert2: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemPowerStatus(s: *mut SystemPowerStatus) -> i32;
    fn LocalFree(p: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}
#[link(name = "powrprof")]
extern "system" {
    fn CallNtPowerInformation(level: i32, inp: *const core::ffi::c_void, in_len: u32, out: *mut core::ffi::c_void, out_len: u32) -> i32;
    fn PowerGetActiveScheme(root: *mut core::ffi::c_void, guid: *mut *mut windows::core::GUID) -> u32;
    fn PowerReadFriendlyName(
        root: *mut core::ffi::c_void,
        scheme: *const windows::core::GUID,
        sub: *const windows::core::GUID,
        setting: *const windows::core::GUID,
        buf: *mut u8,
        size: *mut u32,
    ) -> u32;
}

/// Batería: porcentaje y autonomía de GetSystemPowerStatus; potencia de SYSTEM_BATTERY_STATE
fn read_battery() -> (Battery, bool) {
    unsafe {
        let mut sp = SystemPowerStatus::default();
        if GetSystemPowerStatus(&mut sp) == 0 {
            return (Battery::default(), false);
        }
        let saver = sp.saver == 1;
        // flag 128 = sin batería; 255 = estado desconocido
        if sp.flag & 128 != 0 || sp.flag == 255 || sp.pct > 100 {
            return (Battery { ac: true, secs: -1, ..Default::default() }, saver);
        }
        let mut b = Battery {
            present: true,
            pct: sp.pct as i32,
            ac: sp.ac == 1,
            charging: sp.flag & 8 != 0,
            secs: if sp.ac == 1 || sp.life == u32::MAX { -1 } else { sp.life as i32 },
            watts: 0.0,
        };
        let mut bs = BatteryState::default();
        if CallNtPowerInformation(5, std::ptr::null(), 0, &mut bs as *mut _ as *mut _, std::mem::size_of::<BatteryState>() as u32) == 0 && bs.present != 0 {
            b.charging = bs.charging != 0;
            // algunos equipos informan 0x80000000 (desconocida)
            if bs.rate != i32::MIN && bs.rate.unsigned_abs() < 500_000 {
                b.watts = bs.rate as f32 / 1000.0;
            }
        }
        (b, saver)
    }
}

/// Tipo de plan a partir de su GUID (planes de Windows) o, si es de un fabricante, de su nombre
pub fn plan_kind(guid: &str, name: &str) -> u8 {
    match guid.to_ascii_lowercase().as_str() {
        "a1841308-3541-4fab-bc81-f71556f20b4a" => return 0, // economizador
        "381b4222-f694-41f0-9685-ff5bb260df2e" => return 1, // equilibrado
        "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c" | "e9a42b02-d5df-448d-aa00-03f14749eb61" => return 2, // alto / máximo rendimiento
        _ => {}
    }
    let n = name.to_lowercase();
    if ["perform", "rendimiento", "ultimate", "turbo", "gaming", "máximo", "maximo"].iter().any(|k| n.contains(k)) {
        2
    } else if ["saver", "ahorr", "econom", "eco", "silen", "quiet"].iter().any(|k| n.contains(k)) {
        0
    } else {
        1
    }
}

/// Plan de energía activo: (GUID, nombre)
fn read_plan() -> Option<(String, String)> {
    unsafe {
        let mut g: *mut windows::core::GUID = std::ptr::null_mut();
        if PowerGetActiveScheme(std::ptr::null_mut(), &mut g) != 0 || g.is_null() {
            return None;
        }
        let guid = format!("{:?}", *g).to_ascii_lowercase();
        let mut buf = vec![0u8; 1024];
        let mut size = buf.len() as u32;
        let name = if PowerReadFriendlyName(std::ptr::null_mut(), g, std::ptr::null(), std::ptr::null(), buf.as_mut_ptr(), &mut size) == 0 {
            let u: Vec<u16> = buf[..(size as usize).min(buf.len())].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
            String::from_utf16_lossy(&u)
        } else {
            String::new()
        };
        LocalFree(g as *mut _);
        Some((guid, name))
    }
}

/// Modo de energía de Windows 11 (Configuración → Energía): -1 mejor eficiencia, 0 equilibrado,
/// 1 mejor rendimiento. La función se busca en tiempo de ejecución (no existe en Windows antiguos).
fn read_power_mode() -> i8 {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    type F = unsafe extern "system" fn(*mut windows::core::GUID) -> u32;
    // la función se resuelve una sola vez (0 = no existe en este Windows)
    static ADDR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    unsafe {
        let addr = *ADDR.get_or_init(|| {
            LoadLibraryW(w!("powrprof.dll"))
                .ok()
                .and_then(|lib| GetProcAddress(lib, windows::core::s!("PowerGetEffectiveOverlayScheme")))
                .map(|f| f as usize)
                .unwrap_or(0)
        });
        if addr == 0 {
            return 0;
        }
        let f: F = std::mem::transmute(addr);
        let mut g = windows::core::GUID::zeroed();
        if f(&mut g) != 0 {
            return 0;
        }
        match format!("{g:?}").to_ascii_lowercase().as_str() {
            "961cc777-2547-4f9d-8174-7d86181b8a7a" => -1,
            "ded574b5-45a0-4f42-8737-46345c09c238" => 1,
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------- Aplicaciones que más consumen
/// Consumo de una aplicación (todos sus procesos sumados)
#[derive(Clone, Default, Debug, PartialEq)]
pub struct AppUse {
    pub name: String,
    /// % del total de la CPU (todos los núcleos = 100)
    pub cpu: f32,
    /// memoria privada en bytes
    pub mem: f64,
    pub procs: u32,
}

pub static TOP: LazyLock<Mutex<Vec<AppUse>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static PROCS_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// El muestreo de procesos solo corre mientras la ventana de sistema está abierta
pub fn set_procs_active(on: bool) {
    PROCS_ON.store(on, std::sync::atomic::Ordering::Relaxed);
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut core::ffi::c_void, len: u32, ret: *mut u32) -> i32;
}

/// Un proceso: (pid, nombre de imagen, tiempo de CPU en unidades de 100 ns, memoria privada)
type Proc = (u32, String, u64, u64);

/// Todos los procesos en una sola llamada (SystemProcessInformation), sin abrir ninguno: funciona
/// también con los protegidos
fn list_procs() -> Vec<Proc> {
    let mut len = 1usize << 20;
    let mut buf: Vec<u8> = Vec::new();
    let mut ok = false;
    for _ in 0..5 {
        buf = vec![0u8; len];
        let mut need = 0u32;
        let st = unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr() as *mut _, len as u32, &mut need) };
        if st == 0 {
            ok = true;
            break;
        }
        if st != 0xC000_0004u32 as i32 {
            return Vec::new(); // distinto de STATUS_INFO_LENGTH_MISMATCH
        }
        len = (need as usize).max(len) + (1 << 16);
    }
    if !ok {
        return Vec::new();
    }
    // SYSTEM_PROCESS_INFORMATION (x64): 0 NextEntryOffset, 8 WorkingSetPrivateSize, 40 UserTime,
    // 48 KernelTime, 56 ImageName (Length u16; Buffer en 64), 80 UniqueProcessId
    let rd = |off: usize| -> u64 { unsafe { (buf.as_ptr().add(off) as *const u64).read_unaligned() } };
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 136 <= buf.len() {
        let next = rd(off) as u32 as usize;
        let name_len = (rd(off + 56) as u16 / 2) as usize;
        let name_ptr = rd(off + 64) as *const u16;
        let name = if name_ptr.is_null() || name_len == 0 {
            String::new()
        } else {
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name_ptr, name_len) })
        };
        out.push((rd(off + 80) as u32, name, rd(off + 40) + rd(off + 48), rd(off + 8)));
        if next == 0 {
            break;
        }
        off += next;
    }
    out
}

/// Agrupa los procesos por aplicación y devuelve las `n` que más consumen. La puntuación suma la
/// fracción de CPU y la fracción de memoria, para que destaque tanto lo que calienta como lo que llena
pub fn top_apps(prev: &HashMap<u32, u64>, cur: &[Proc], dt: f64, ncpu: usize, total_ram: f64, n: usize) -> Vec<AppUse> {
    let mut apps: HashMap<String, AppUse> = HashMap::new();
    for (pid, name, cpu_t, mem) in cur {
        if *pid == 0 || name.is_empty() {
            continue; // proceso inactivo del sistema
        }
        let shown = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
        let e = apps.entry(shown.to_lowercase()).or_insert_with(|| AppUse { name: shown.to_string(), ..Default::default() });
        if let Some(p) = prev.get(pid).filter(|p| **p <= *cpu_t) {
            if dt > 0.0 {
                e.cpu += ((*cpu_t - *p) as f64 / 1e7 / dt / ncpu.max(1) as f64 * 100.0) as f32;
            }
        }
        e.mem += *mem as f64;
        e.procs += 1;
    }
    let score = |a: &AppUse| a.cpu as f64 / 100.0 + if total_ram > 0.0 { a.mem / total_ram } else { 0.0 };
    let mut v: Vec<AppUse> = apps.into_values().collect();
    v.sort_by(|a, b| score(b).total_cmp(&score(a)).then_with(|| a.name.cmp(&b.name)));
    v.truncate(n);
    v
}

pub fn spawn_procs_worker() {
    std::thread::spawn(|| {
        crate::util::prof("procs");
        let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let mut prev: HashMap<u32, u64> = HashMap::new();
        let mut at = Instant::now();
        loop {
            if !PROCS_ON.load(std::sync::atomic::Ordering::Relaxed) {
                prev.clear(); // al reabrir se toma una referencia nueva
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            let cur = list_procs();
            let now = Instant::now();
            if !prev.is_empty() {
                let total = st().ram_total as f64 * 1_073_741_824.0;
                *TOP.lock().unwrap() = top_apps(&prev, &cur, now.duration_since(at).as_secs_f64(), ncpu, total, 5);
            }
            prev = cur.iter().map(|p| (p.0, p.2)).collect();
            at = now;
            // primera muestra: la referencia; la siguiente llega enseguida para no dejar la tarjeta vacía
            std::thread::sleep(Duration::from_millis(if TOP.lock().unwrap().is_empty() { 700 } else { 2000 }));
        }
    });
}

// ---------------------------------------------------------------- Trabajador
pub fn spawn_sys_worker() {
    std::thread::spawn(|| unsafe {
        crate::util::prof("sys");
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let cpu = Cpu::new();
        let mut gpus = crate::gpu::Gpus::new();
        let lenovo = Lenovo::new();
        let map: PresentMap = Arc::new(Mutex::new(HashMap::new()));
        start_presentmon(map.clone());
        let me = std::process::id();
        let mut tick: u64 = 0;
        loop {
            let c = cpu.as_ref().map(|c| c.read()).unwrap_or(-1.0);
            let gr = gpus.read();
            let gpu_info = gpus.info();
            let main = gpu_info.iter().position(|i| !i.3);
            let pr = main.and_then(|i| gr.get(i)).cloned().unwrap_or(crate::gpu::Reading { usage: -1, temp: -1, fan: -1, ..Default::default() });
            let (g, mut gt) = (pr.usage, pr.temp);
            let lenovo_vals = if tick % 2 == 0 {
                lenovo.as_ref().map(|l| (l.get(0x0504_0000), l.get(0x0403_0001), l.get(0x0403_0002), l.get(0x0505_0000)))
            } else {
                None
            };
            if tick % 10 == 9 && !presentmon_running() {
                start_presentmon(map.clone());
            }
            let (ram_used, ram_total) = {
                use windows::Win32::System::SystemInformation::*;
                let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
                if GlobalMemoryStatusEx(&mut ms).is_ok() {
                    let gb = |b: u64| b as f32 / 1073741824.0;
                    (gb(ms.ullTotalPhys - ms.ullAvailPhys), gb(ms.ullTotalPhys))
                } else {
                    (0.0, 0.0)
                }
            };
            let cpu_mhz = cpu.as_ref().map(|c| c.mhz()).unwrap_or(0.0);
            let (disk_r, disk_w) = cpu.as_ref().map(|c| c.disk()).unwrap_or((0.0, 0.0));
            let drives = if tick % 10 == 0 { Some(scan_drives()) } else { None };
            let battery = if tick % 2 == 0 { Some(read_battery()) } else { None };
            let plan = if tick % 5 == 0 { Some((read_plan(), read_power_mode())) } else { None };
            let gpu_detail = (pr.mem_used, pr.mem_total, pr.power, pr.mhz);
            let mut fg_pid = 0u32;
            GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut fg_pid));
            let (fps, app) = if fg_pid == me || fg_pid == 0 { (0, String::new()) } else { fps_of(&map, fg_pid) };
            {
                let mut s = st();
                s.cpu = c;
                s.gpu = g;
                if let Some((ct, fc, fgv, gt2)) = lenovo_vals {
                    s.cpu_t = ct;
                    s.fan_c = fc;
                    s.fan_g = fgv;
                    if gt < 0 {
                        gt = gt2;
                    }
                } else if lenovo.is_none() {
                    s.fan_g = pr.fan;
                }
                if gt >= 0 || s.gpu_t < 0 {
                    s.gpu_t = gt;
                }
                s.fps = fps;
                s.app = app;
                s.cpu_mhz = cpu_mhz;
                (s.vram_used, s.vram_total, s.gpu_power, s.gpu_mhz) = gpu_detail;
                s.ram_used = ram_used;
                s.ram_total = ram_total;
                if s.cpu_name.is_empty() {
                    s.cpu_name = cpu.as_ref().map(|c| c.name.clone()).unwrap_or_default();
                    s.gpu_name = main.map(|i| gpu_info[i].0.clone()).unwrap_or_default();
                }
                let push = |h: &mut Vec<f32>, v: f32| {
                    if h.is_empty() {
                        *h = vec![0.0; 60];
                    }
                    h.remove(0);
                    h.push(v.max(0.0));
                };
                let (cpu_v, gpu_v, ct, gt, fp) = (s.cpu, s.gpu as f32, s.cpu_t as f32, s.gpu_t as f32, s.fps as f32);
                push(&mut s.hist_cpu, cpu_v);
                push(&mut s.hist_gpu, gpu_v);
                push(&mut s.hist_cpu_t, ct);
                push(&mut s.hist_gpu_t, gt);
                push(&mut s.hist_fps, fp);
                let (fan_g, gpu_t) = (s.fan_g, s.gpu_t);
                let changed = s.gpus.len() != gpu_info.len()
                    || s.gpus.iter().zip(gpu_info.iter()).any(|(g, (n, v, i, npu))| g.name != *n || g.vendor != *v || g.integrated != *i || g.npu != *npu);
                if changed {
                    s.gpus = gpu_info.iter().map(|(name, vendor, integrated, npu)| state::GpuStat {
                        name: name.clone(), vendor: *vendor, integrated: *integrated, npu: *npu, ..Default::default()
                    }).collect();
                }
                for (i, (gs, r)) in s.gpus.iter_mut().zip(gr.iter()).enumerate() {
                    let principal = Some(i) == main;
                    gs.usage = r.usage;
                    gs.temp = if principal { gpu_t } else { r.temp };
                    gs.fan = if principal && r.fan < 0 { fan_g } else { r.fan };
                    (gs.mem_used, gs.mem_total, gs.power, gs.mhz) = (r.mem_used, r.mem_total, r.power, r.mhz);
                    push(&mut gs.hist, r.usage as f32);
                    push(&mut gs.hist_t, gs.temp as f32);
                }
                s.disk_read = disk_r;
                s.disk_write = disk_w;
                push(&mut s.hist_disk_r, disk_r as f32);
                push(&mut s.hist_disk_w, disk_w as f32);
                if let Some(d) = drives {
                    s.drives = d;
                }
                let mut saver = None;
                if let Some((b, sv)) = battery {
                    (s.bat_present, s.bat_pct, s.bat_ac, s.bat_charging, s.bat_secs, s.bat_watts) = (b.present, b.pct, b.ac, b.charging, b.secs, b.watts);
                    saver = Some(sv);
                }
                if let Some((p, mode)) = plan {
                    let (guid, name) = p.unwrap_or_default();
                    s.plan_kind = match mode {
                        -1 => 0,
                        1 => 2,
                        _ => plan_kind(&guid, &name),
                    };
                    s.plan_name = name;
                    s.power_mode = mode;
                }
                // ahorro de energía de Windows activo: manda sobre el plan
                if saver == Some(true) {
                    s.plan_kind = 0;
                }
            }
            tick += 1;
            std::thread::sleep(Duration::from_millis(1000));
        }
    });
    let _ = HWND::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tipo_de_plan() {
        assert_eq!(plan_kind("381B4222-F694-41F0-9685-FF5BB260DF2E", "lo que sea"), 1);
        assert_eq!(plan_kind("a1841308-3541-4fab-bc81-f71556f20b4a", ""), 0);
        assert_eq!(plan_kind("e9a42b02-d5df-448d-aa00-03f14749eb61", ""), 2);
        // planes de fabricante: por nombre
        assert_eq!(plan_kind("d11347a9-ccac-46a1-a5ef-a1c63cec0593", "High performance"), 2);
        assert_eq!(plan_kind("x", "Alto rendimiento"), 2);
        assert_eq!(plan_kind("x", "Economizador"), 0);
        assert_eq!(plan_kind("x", "Mi plan"), 1);
    }

    #[test]
    fn procesos_del_sistema() {
        let me = std::process::id();
        let procs = list_procs();
        assert!(procs.len() > 5);
        let mine = procs.iter().find(|p| p.0 == me).expect("este proceso aparece en la lista");
        assert!(mine.1.to_lowercase().contains("taskbar"), "nombre: {}", mine.1);
        assert!(mine.3 > 100_000, "memoria privada: {}", mine.3);
    }

    #[test]
    fn apps_agrupadas_y_ordenadas() {
        let prev: HashMap<u32, u64> = [(10, 0), (11, 0), (20, 0), (30, 0)].into_iter().collect();
        // 2 s, 4 núcleos: 8 s de CPU = 100 %
        let cur: Vec<Proc> = vec![
            (0, String::new(), 999_999_999, 0),
            (10, "chrome.exe".into(), 10_000_000, 500_000_000),
            (11, "Chrome.exe".into(), 10_000_000, 700_000_000),
            (20, "game.exe".into(), 40_000_000, 2_000_000_000),
            (30, "idle-app.exe".into(), 0, 10_000_000),
            (40, "nuevo.exe".into(), 50_000_000, 1_000),
        ];
        let top = top_apps(&prev, &cur, 2.0, 4, 16e9, 2);
        assert_eq!(top.len(), 2);
        assert_eq!((top[0].name.as_str(), top[0].procs), ("game", 1));
        assert!((top[0].cpu - 50.0).abs() < 0.01);
        assert_eq!((top[1].name.as_str(), top[1].procs), ("chrome", 2));
        assert!((top[1].cpu - 25.0).abs() < 0.01);
        assert_eq!(top[1].mem, 1.2e9);
        // un proceso sin referencia previa no cuenta CPU (no se sabe cuánto gastó en el intervalo)
        let all = top_apps(&prev, &cur, 2.0, 4, 16e9, 10);
        assert_eq!(all.iter().find(|a| a.name == "nuevo").unwrap().cpu, 0.0);
    }

    #[test]
    fn lectura_de_energia_no_falla() {
        let (b, _) = read_battery();
        assert!((0..=100).contains(&b.pct));
        assert!(b.present || b.ac);
        let (guid, _) = read_plan().expect("plan activo");
        assert_eq!(guid.len(), 36);
        assert!((-1..=1).contains(&read_power_mode()));
    }
}
