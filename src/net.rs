//! Red local para la ventana de detalle: adaptador activo, puerta de enlace, DNS e IP local
//! (GetAdaptersAddresses) y ping ICMP a la puerta de enlace (IcmpSendEcho).
//! El ping solo se ejecuta mientras la ventana de red está abierta (`set_active`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::NetworkManagement::IpHelper::*;

#[derive(Clone, Default)]
pub struct NetInfo {
    pub adapter: String,
    pub wifi: bool,
    pub link_bps: u64,
    pub local_ip: String,
    pub gateway: String,
    pub dns: Vec<String>,
}

#[derive(Clone, Default)]
pub struct PingStats {
    /// último tiempo de respuesta (ms); None = sin respuesta
    pub last: Option<u32>,
    /// historial (ms; 0 = sin respuesta), el más reciente al final
    pub hist: Vec<f32>,
    pub sent: u32,
    pub lost: u32,
    pub min: u32,
    pub max: u32,
    pub sum: u64,
    /// momento de la última respuesta (para el parpadeo del LED)
    pub reply_at: Option<Instant>,
}

pub static INFO: LazyLock<Mutex<Option<NetInfo>>> = LazyLock::new(|| Mutex::new(None));
pub static PING: LazyLock<Mutex<PingStats>> = LazyLock::new(|| Mutex::new(PingStats::default()));
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Activa/desactiva el ping (al abrir la ventana se reinician las estadísticas)
pub fn set_active(on: bool) {
    let was = ACTIVE.swap(on, Ordering::Relaxed);
    if on && !was {
        *PING.lock().unwrap() = PingStats::default();
    }
}

/// Dirección IP de un SOCKET_ADDRESS (IPv4 o IPv6) como texto
unsafe fn sockaddr_str(sa: &windows::Win32::Networking::WinSock::SOCKET_ADDRESS) -> Option<String> {
    let p = sa.lpSockaddr as *const u8;
    if p.is_null() {
        return None;
    }
    let family = (p as *const u16).read_unaligned();
    match family {
        2 => {
            let b = std::slice::from_raw_parts(p.add(4), 4);
            Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
        }
        23 => {
            let b = std::slice::from_raw_parts(p.add(8), 16);
            let segs: Vec<String> = b.chunks(2).map(|c| format!("{:x}", u16::from_be_bytes([c[0], c[1]]))).collect();
            Some(segs.join(":"))
        }
        _ => None,
    }
}

fn pwstr(p: windows::core::PWSTR) -> String {
    if p.is_null() { String::new() } else { unsafe { p.to_string().unwrap_or_default() } }
}

/// Adaptador activo con puerta de enlace IPv4 y menor métrica
pub fn read_info() -> Option<NetInfo> {
    unsafe {
        let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
        let mut size = 32 * 1024u32;
        let mut buf: Vec<u8> = vec![0; size as usize];
        let mut r = GetAdaptersAddresses(0, flags, None, Some(buf.as_mut_ptr() as *mut _), &mut size);
        if r == 111 {
            // ERROR_BUFFER_OVERFLOW
            buf = vec![0; size as usize];
            r = GetAdaptersAddresses(0, flags, None, Some(buf.as_mut_ptr() as *mut _), &mut size);
        }
        if r != 0 {
            return None;
        }
        let mut best: Option<(u32, NetInfo)> = None;
        let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !a.is_null() {
            let ad = &*a;
            a = ad.Next;
            if ad.OperStatus.0 != 1 {
                continue; // no conectado
            }
            // puerta de enlace IPv4
            let mut gw = None;
            let mut g = ad.FirstGatewayAddress;
            while !g.is_null() {
                if let Some(s) = sockaddr_str(&(*g).Address) {
                    if s.contains('.') && s != "0.0.0.0" {
                        gw = Some(s);
                        break;
                    }
                }
                g = (*g).Next;
            }
            let Some(gateway) = gw else { continue };
            let mut dns = Vec::new();
            let mut d = ad.FirstDnsServerAddress;
            while !d.is_null() {
                if let Some(s) = sockaddr_str(&(*d).Address) {
                    if !s.starts_with("fec0:") {
                        dns.push(s);
                    }
                }
                d = (*d).Next;
            }
            let mut local_ip = String::new();
            let mut u = ad.FirstUnicastAddress;
            while !u.is_null() {
                if let Some(s) = sockaddr_str(&(*u).Address) {
                    if s.contains('.') {
                        local_ip = s;
                        break;
                    }
                }
                u = (*u).Next;
            }
            let info = NetInfo {
                adapter: pwstr(ad.FriendlyName),
                wifi: ad.IfType == 71, // IF_TYPE_IEEE80211
                link_bps: ad.ReceiveLinkSpeed.max(ad.TransmitLinkSpeed),
                local_ip,
                gateway,
                dns,
            };
            if best.as_ref().map(|(m, _)| ad.Ipv4Metric < *m).unwrap_or(true) {
                best = Some((ad.Ipv4Metric, info));
            }
        }
        best.map(|(_, i)| i)
    }
}

/// Un ping ICMP (timeout en ms); Some(rtt) o None si no hubo respuesta
fn ping(icmp: windows::Win32::Foundation::HANDLE, ip: &str, timeout: u32) -> Option<u32> {
    let addr: std::net::Ipv4Addr = ip.parse().ok()?;
    let dest = u32::from_ne_bytes(addr.octets());
    let data = *b"taskbar-panels-ping";
    let mut reply = vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + data.len() + 16];
    unsafe {
        let n = IcmpSendEcho(
            icmp,
            dest,
            data.as_ptr() as *const _,
            data.len() as u16,
            None,
            reply.as_mut_ptr() as *mut _,
            reply.len() as u32,
            timeout,
        );
        if n == 0 {
            return None;
        }
        let r = &*(reply.as_ptr() as *const ICMP_ECHO_REPLY);
        if r.Status != 0 { None } else { Some(r.RoundTripTime) }
    }
}

pub fn spawn_net_worker() {
    std::thread::spawn(|| {
        let icmp = unsafe { IcmpCreateFile() }.ok();
        let mut info_at: Option<Instant> = None;
        loop {
            if !ACTIVE.load(Ordering::Relaxed) {
                info_at = None; // al volver a abrir se relee la red
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            if info_at.map(|t| t.elapsed() > Duration::from_secs(10)).unwrap_or(true) {
                *INFO.lock().unwrap() = read_info();
                info_at = Some(Instant::now());
            }
            let gw = INFO.lock().unwrap().as_ref().map(|i| i.gateway.clone());
            let start = Instant::now();
            if let (Some(icmp), Some(gw)) = (icmp, gw) {
                let r = ping(icmp, &gw, 1000);
                let mut p = PING.lock().unwrap();
                p.sent += 1;
                p.last = r;
                match r {
                    Some(ms) => {
                        p.min = if p.sent - p.lost == 1 { ms } else { p.min.min(ms) };
                        p.max = p.max.max(ms);
                        p.sum += ms as u64;
                        p.reply_at = Some(Instant::now());
                    }
                    None => p.lost += 1,
                }
                if p.hist.is_empty() {
                    p.hist = vec![0.0; 30];
                }
                p.hist.remove(0);
                p.hist.push(r.map(|v| v as f32 + 0.5).unwrap_or(0.0));
            }
            // un ping por segundo
            let el = start.elapsed();
            if el < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_secs(1) - el);
            }
        }
    });
}
