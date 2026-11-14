//! Métricas de sistema: CPU (PDH), GPU y NPU (gpu.rs), temperaturas/ventiladores Lenovo (WMI)
//! y FPS de la ventana en primer plano (PresentMon).

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

// ---------------------------------------------------------------- Trabajador
pub fn spawn_sys_worker() {
    std::thread::spawn(|| unsafe {
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
            }
            tick += 1;
            std::thread::sleep(Duration::from_millis(1000));
        }
    });
    let _ = HWND::default();
}
