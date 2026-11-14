//! GPU de cualquier fabricante (NVIDIA, AMD Radeon, Intel, Qualcomm):
//! · adaptadores físicos con DXGI (nombre, fabricante, VRAM total, LUID)
//! · uso y memoria por adaptador con los contadores de Windows "GPU Engine" / "GPU Adapter Memory"
//!   (los mismos que usa el Administrador de tareas)
//! · temperatura, ventilador y frecuencia con D3DKMT (WDDM 2.5+, lo publican AMD e Intel)
//! · NVML, si existe, completa las NVIDIA con consumo en vatios y lecturas más precisas

use std::collections::HashMap;
use windows::core::{s, w, PCWSTR};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Performance::*;

pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

/// Lectura instantánea de una GPU
#[derive(Clone, Default)]
pub struct Reading {
    pub usage: i32,
    pub temp: i32,
    pub mem_used: f32,  // GB
    pub mem_total: f32, // GB
    pub power: f32,     // W (solo NVML)
    pub mhz: u32,
    pub fan: i32,
}

struct Adapter {
    key: String, // "luid_0x........_0x........" en minúsculas
    name: String,
    vendor: u32,
    integrated: bool,
    dedicated: u64,
    shared: u64,
    kmt: u32,
    nv: Option<*mut core::ffi::c_void>,
    npu: bool,
}

// ---------------------------------------------------------------- D3DKMT (gdi32)
#[repr(C)]
struct OpenFromLuid {
    luid: LUID,
    h: u32,
}
#[repr(C)]
struct QueryInfo {
    h: u32,
    ty: i32,
    data: *mut core::ffi::c_void,
    size: u32,
}
#[repr(C)]
#[derive(Default)]
struct AdapterPerf {
    phys: u32,
    mem_freq: u64,
    max_mem_freq: u64,
    max_mem_freq_oc: u64,
    mem_bw: u64,
    pcie_bw: u64,
    fan_rpm: u32,
    power: u32,
    temperature: u32, // décimas de °C
    power_override: u8,
}
#[repr(C)]
#[derive(Default)]
struct NodePerf {
    node: u32,
    phys: u32,
    freq: u64,
    max_freq: u64,
    max_freq_oc: u64,
    voltage: u32,
    voltage_max: u32,
    voltage_max_oc: u32,
    latency: u64,
}
const KMTQAITYPE_NODEPERFDATA: i32 = 61;
const KMTQAITYPE_ADAPTERPERFDATA: i32 = 62;
type FnOpen = unsafe extern "system" fn(*mut OpenFromLuid) -> i32;
type FnQuery = unsafe extern "system" fn(*mut QueryInfo) -> i32;

// ---------------------------------------------------------------- NVML
#[repr(C)]
#[derive(Default)]
struct NvUtil {
    gpu: u32,
    memory: u32,
}
#[repr(C)]
#[derive(Default)]
struct NvMem {
    total: u64,
    free: u64,
    used: u64,
}
type NvDev = *mut core::ffi::c_void;
struct Nvml {
    util: unsafe extern "C" fn(NvDev, *mut NvUtil) -> i32,
    temp: unsafe extern "C" fn(NvDev, i32, *mut u32) -> i32,
    mem: Option<unsafe extern "C" fn(NvDev, *mut NvMem) -> i32>,
    power: Option<unsafe extern "C" fn(NvDev, *mut u32) -> i32>,
    clock: Option<unsafe extern "C" fn(NvDev, i32, *mut u32) -> i32>,
    devices: Vec<(String, NvDev)>,
}

unsafe fn nvml_load() -> Option<Nvml> {
    let lib = LoadLibraryW(w!("nvml.dll")).ok()?;
    let f = |n: windows::core::PCSTR| GetProcAddress(lib, n);
    let init: unsafe extern "C" fn() -> i32 = std::mem::transmute(f(s!("nvmlInit_v2"))?);
    let count: unsafe extern "C" fn(*mut u32) -> i32 = std::mem::transmute(f(s!("nvmlDeviceGetCount_v2"))?);
    let handle: unsafe extern "C" fn(u32, *mut NvDev) -> i32 = std::mem::transmute(f(s!("nvmlDeviceGetHandleByIndex_v2"))?);
    let name: unsafe extern "C" fn(NvDev, *mut u8, u32) -> i32 = std::mem::transmute(f(s!("nvmlDeviceGetName"))?);
    let nv = Nvml {
        util: std::mem::transmute(f(s!("nvmlDeviceGetUtilizationRates"))?),
        temp: std::mem::transmute(f(s!("nvmlDeviceGetTemperature"))?),
        mem: f(s!("nvmlDeviceGetMemoryInfo")).map(|p| std::mem::transmute(p)),
        power: f(s!("nvmlDeviceGetPowerUsage")).map(|p| std::mem::transmute(p)),
        clock: f(s!("nvmlDeviceGetClockInfo")).map(|p| std::mem::transmute(p)),
        devices: Vec::new(),
    };
    if init() != 0 {
        return None;
    }
    let mut n = 0u32;
    if count(&mut n) != 0 {
        return None;
    }
    let mut nv = nv;
    for i in 0..n.min(16) {
        let mut d = std::ptr::null_mut();
        if handle(i, &mut d) != 0 {
            continue;
        }
        let mut buf = [0u8; 96];
        let nm = if name(d, buf.as_mut_ptr(), buf.len() as u32) == 0 {
            let e = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..e]).trim().to_string()
        } else {
            String::new()
        };
        nv.devices.push((nm, d));
    }
    Some(nv)
}

// ---------------------------------------------------------------- Monitor
pub struct Gpus {
    adapters: Vec<Adapter>,
    open: Option<FnOpen>,
    query: Option<FnQuery>,
    nvml: Option<Nvml>,
    pdh: isize,
    engine: isize,
    ded: isize,
    shr: isize,
    reads: u32,
}

unsafe impl Send for Gpus {}

/// Nombre corto: sin marcas registradas ni prefijos de fabricante redundantes
pub fn short_name(n: &str) -> String {
    let n = n.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    let n = n.trim();
    for p in ["NVIDIA GeForce ", "NVIDIA ", "AMD Radeon(TM) ", "AMD "] {
        if let Some(r) = n.strip_prefix(p) {
            return if p.starts_with("AMD") && !r.starts_with("Radeon") { format!("Radeon {r}") } else { r.to_string() };
        }
    }
    n.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Gpus {
    pub fn new() -> Self {
        let mut g = Gpus { adapters: Vec::new(), open: None, query: None, nvml: None, pdh: 0, engine: 0, ded: 0, shr: 0, reads: 0 };
        unsafe {
            if let Ok(lib) = LoadLibraryW(w!("gdi32.dll")) {
                g.open = GetProcAddress(lib, s!("D3DKMTOpenAdapterFromLuid")).map(|p| std::mem::transmute(p));
                g.query = GetProcAddress(lib, s!("D3DKMTQueryAdapterInfo")).map(|p| std::mem::transmute(p));
            }
            g.enumerate();
            if g.adapters.iter().any(|a| a.vendor == VENDOR_NVIDIA) {
                g.nvml = nvml_load();
                if let Some(nv) = g.nvml.as_ref() {
                    let mut used = vec![false; nv.devices.len()];
                    for a in g.adapters.iter_mut().filter(|a| a.vendor == VENDOR_NVIDIA) {
                        // empareja por nombre; si no coincide, la primera libre
                        let pick = nv.devices.iter().enumerate().position(|(i, (n, _))| !used[i] && n.eq_ignore_ascii_case(&a.name))
                            .or_else(|| used.iter().position(|u| !u));
                        if let Some(i) = pick {
                            used[i] = true;
                            a.nv = Some(nv.devices[i].1);
                        }
                    }
                }
            }
            let mut q = 0isize;
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) == 0 {
                g.pdh = q;
                let _ = PdhAddEnglishCounterW(q, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut g.engine);
                let _ = PdhAddEnglishCounterW(q, w!("\\GPU Adapter Memory(*)\\Dedicated Usage"), 0, &mut g.ded);
                let _ = PdhAddEnglishCounterW(q, w!("\\GPU Adapter Memory(*)\\Shared Usage"), 0, &mut g.shr);
                PdhCollectQueryData(q);
            }
        }
        g
    }

    unsafe fn enumerate(&mut self) {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return };
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            // solo GPU reales (descarta Microsoft Basic Render, DisplayLink y otros adaptadores virtuales)
            if ![VENDOR_NVIDIA, VENDOR_AMD, 0x1022, VENDOR_INTEL, 0x5143, 0x4D4F4351].contains(&d.VendorId) {
                continue;
            }
            let key = format!("luid_0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart);
            if self.adapters.iter().any(|x| x.key == key) {
                continue;
            }
            let e = d.Description.iter().position(|c| *c == 0).unwrap_or(d.Description.len());
            let name = String::from_utf16_lossy(&d.Description[..e]).trim().to_string();
            let dedicated = d.DedicatedVideoMemory as u64;
            let integrated = dedicated < 512 << 20 || (d.VendorId == VENDOR_INTEL && dedicated < 1 << 30);
            let mut kmt = 0u32;
            if let Some(open) = self.open {
                let mut o = OpenFromLuid { luid: d.AdapterLuid, h: 0 };
                if open(&mut o) == 0 {
                    kmt = o.h;
                }
            }
            let vendor = if d.VendorId == 0x1022 { VENDOR_AMD } else { d.VendorId };
            self.adapters.push(Adapter { key, name, vendor, integrated, dedicated, shared: d.SharedSystemMemory as u64, kmt, nv: None, npu: false });
        }
        // primero las dedicadas con más VRAM: la principal queda en el índice 0
        self.adapters.sort_by(|a, b| a.integrated.cmp(&b.integrated).then(b.dedicated.cmp(&a.dedicated)));
    }

    /// (nombre corto, fabricante, integrada, NPU) de cada adaptador, en el mismo orden que `read()`
    pub fn info(&self) -> Vec<(String, u32, bool, bool)> {
        self.adapters.iter().map(|a| (short_name(&a.name), a.vendor, a.integrated, a.npu)).collect()
    }

    /// NPU (Intel AI Boost, AMD XDNA, Qualcomm Hexagon…): DXGI no las lista porque no tienen gráficos.
    /// Aparecen en los contadores "GPU Engine" con su propio LUID; DXCore da el nombre y confirma
    /// que es un adaptador de hardware solo de cómputo.
    unsafe fn scan_npus(&mut self, luids: &[String]) {
        use windows::Win32::Graphics::DXCore::*;
        let Ok(f) = DXCoreCreateAdapterFactory::<IDXCoreAdapterFactory>() else { return };
        for key in luids {
            if self.adapters.iter().any(|a| a.key == *key) {
                continue;
            }
            let mut it = key.trim_start_matches("luid_0x").split("_0x");
            let (Some(Ok(hi)), Some(Ok(lo))) = (it.next().map(|h| u32::from_str_radix(h, 16)), it.next().map(|l| u32::from_str_radix(l, 16))) else { continue };
            let luid = LUID { LowPart: lo, HighPart: hi as i32 };
            let Ok(a) = f.GetAdapterByLuid::<IDXCoreAdapter>(&luid) else { continue };
            let graphics = a.IsAttributeSupported(&DXCORE_ADAPTER_ATTRIBUTE_D3D11_GRAPHICS) || a.IsAttributeSupported(&DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS);
            let mut hw = 0u8;
            let _ = a.GetProperty(IsHardware, 1, &mut hw as *mut u8 as *mut _);
            let mut id = DXCoreHardwareID::default();
            let _ = a.GetProperty(HardwareID, std::mem::size_of::<DXCoreHardwareID>(), &mut id as *mut _ as *mut _);
            if graphics || hw == 0 || ![VENDOR_NVIDIA, VENDOR_AMD, 0x1022, VENDOR_INTEL, 0x5143, 0x4D4F4351].contains(&id.vendorID) {
                continue;
            }
            let n = a.GetPropertySize(DriverDescription).unwrap_or(0);
            let mut buf = vec![0u8; n.max(1)];
            let _ = a.GetProperty(DriverDescription, n, buf.as_mut_ptr() as *mut _);
            let e = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..e]).trim().to_string();
            let mut shared = 0u64;
            let _ = a.GetProperty(SharedSystemMemory, 8, &mut shared as *mut u64 as *mut _);
            let mut kmt = 0u32;
            if let Some(open) = self.open {
                let mut o = OpenFromLuid { luid, h: 0 };
                if open(&mut o) == 0 {
                    kmt = o.h;
                }
            }
            let vendor = if id.vendorID == 0x1022 { VENDOR_AMD } else { id.vendorID };
            let name = if name.is_empty() { "NPU".into() } else { name };
            self.adapters.push(Adapter { key: key.clone(), name, vendor, integrated: true, dedicated: 0, shared, kmt, nv: None, npu: true });
        }
    }

    unsafe fn pdh_array(&self, h: isize) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        if h == 0 {
            return out;
        }
        let (mut size, mut count) = (0u32, 0u32);
        let _ = PdhGetFormattedCounterArrayW(h, PDH_FMT(PDH_FMT_DOUBLE.0 | 0x8000), &mut size, &mut count, None);
        if size == 0 {
            return out;
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8) + 1];
        let p = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(h, PDH_FMT(PDH_FMT_DOUBLE.0 | 0x8000), &mut size, &mut count, Some(p)) != 0 {
            return out;
        }
        for it in std::slice::from_raw_parts(p, count as usize) {
            if it.FmtValue.CStatus != 0 {
                continue;
            }
            let name = it.szName.to_string().unwrap_or_default().to_ascii_lowercase();
            out.push((name, it.FmtValue.Anonymous.doubleValue));
        }
        out
    }

    unsafe fn kmt<T: Default>(&self, h: u32, ty: i32, init: impl FnOnce(&mut T)) -> Option<T> {
        let q = self.query?;
        if h == 0 {
            return None;
        }
        let mut v = T::default();
        init(&mut v);
        let mut qi = QueryInfo { h, ty, data: &mut v as *mut T as *mut _, size: std::mem::size_of::<T>() as u32 };
        (q(&mut qi) == 0).then_some(v)
    }

    pub fn read(&mut self) -> Vec<Reading> {
        self.reads += 1;
        unsafe {
            if self.pdh != 0 {
                PdhCollectQueryData(self.pdh);
            }
            // uso: suma por (adaptador, tipo de motor) y máximo entre tipos, como el Administrador de tareas
            let mut eng: HashMap<(String, String), f64> = HashMap::new();
            for (n, v) in self.pdh_array(self.engine) {
                let (Some(l), Some(t)) = (n.find("luid_"), n.find("engtype_")) else { continue };
                let key = n[l..].split("_phys").next().unwrap_or("").to_string();
                *eng.entry((key, n[t + 8..].to_string())).or_default() += v;
            }
            // las NPU se buscan con los primeros datos válidos (y una vez más por si el driver tarda)
            if self.reads == 2 || self.reads == 3 || self.reads == 30 {
                let mut l: Vec<String> = eng.keys().map(|(k, _)| k.clone()).collect();
                l.sort();
                l.dedup();
                self.scan_npus(&l);
                // DXGI puede listar el mismo chip dos veces (p. ej. con pantallas DisplayLink): se descartan
                // los adaptadores sin motores en los contadores si hay otro con el mismo nombre
                if !l.is_empty() {
                    let names: Vec<(String, bool)> = self.adapters.iter().map(|a| (a.name.clone(), l.contains(&a.key))).collect();
                    let mut i = 0;
                    self.adapters.retain(|a| {
                        let keep = names[i].1 || a.nv.is_some() || !names.iter().any(|(n, live)| *live && *n == a.name);
                        i += 1;
                        keep
                    });
                }
            }
            let mem = |h: isize| -> HashMap<String, f64> {
                let mut m = HashMap::new();
                for (n, v) in self.pdh_array(h) {
                    if let Some(l) = n.find("luid_") {
                        *m.entry(n[l..].split("_phys").next().unwrap_or("").to_string()).or_default() += v;
                    }
                }
                m
            };
            let (ded, shr) = (mem(self.ded), mem(self.shr));
            let gb = |b: f64| (b / 1073741824.0) as f32;
            self.adapters
                .iter()
                .map(|a| {
                    let mut r = Reading { usage: -1, temp: -1, fan: -1, ..Default::default() };
                    let u = eng.iter().filter(|((k, _), _)| *k == a.key).map(|(_, v)| *v).fold(-1.0f64, f64::max);
                    if u >= 0.0 {
                        r.usage = u.round().clamp(0.0, 100.0) as i32;
                    }
                    if a.integrated {
                        r.mem_used = gb(shr.get(&a.key).copied().unwrap_or(0.0));
                        r.mem_total = gb(a.shared as f64);
                    } else {
                        r.mem_used = gb(ded.get(&a.key).copied().unwrap_or(0.0));
                        r.mem_total = gb(a.dedicated as f64);
                    }
                    if let Some(p) = self.kmt::<AdapterPerf>(a.kmt, KMTQAITYPE_ADAPTERPERFDATA, |_| {}) {
                        if p.temperature > 0 && p.temperature < 1500 {
                            r.temp = (p.temperature as f32 / 10.0).round() as i32;
                        }
                        if p.fan_rpm > 0 {
                            r.fan = p.fan_rpm as i32;
                        }
                    }
                    if let Some(p) = self.kmt::<NodePerf>(a.kmt, KMTQAITYPE_NODEPERFDATA, |_| {}) {
                        r.mhz = (p.freq / 1_000_000) as u32;
                    }
                    if let (Some(nv), Some(d)) = (self.nvml.as_ref(), a.nv) {
                        let mut ut = NvUtil::default();
                        if (nv.util)(d, &mut ut) == 0 {
                            r.usage = ut.gpu as i32;
                        }
                        let mut t = 0u32;
                        if (nv.temp)(d, 0, &mut t) == 0 {
                            r.temp = t as i32;
                        }
                        let mut m = NvMem::default();
                        if nv.mem.is_some_and(|f| f(d, &mut m) == 0) {
                            (r.mem_used, r.mem_total) = (gb(m.used as f64), gb(m.total as f64));
                        }
                        let mut mw = 0u32;
                        if nv.power.is_some_and(|f| f(d, &mut mw) == 0) {
                            r.power = mw as f32 / 1000.0;
                        }
                        let mut c = 0u32;
                        if nv.clock.is_some_and(|f| f(d, 0, &mut c) == 0) && c > 0 {
                            r.mhz = c;
                        }
                    }
                    r
                })
                .collect()
        }
    }
}
