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

// ======================================================================= salud de la conexión
/// Muestras que se conservan por tramo (una cada 2 s → 2 minutos)
pub const HEALTH_SAMPLES: usize = 60;
const HEALTH_EVERY: Duration = Duration::from_secs(2);
/// Destinos de Internet: basta con que responda uno
const INTERNET: [&str; 2] = ["1.1.1.1", "8.8.8.8"];

/// Un tramo de la conexión (router, DNS o Internet)
#[derive(Clone, Default)]
pub struct Hop {
    /// destino del ping ("" = no aplica)
    pub target: String,
    /// historial: -1 sin muestra, 0 sin respuesta, >0 ms (+0,5); el más reciente al final
    pub hist: Vec<f32>,
    /// ha respondido alguna vez: si no, el destino no admite ping y el tramo no se evalúa
    pub ever: bool,
}

impl Hop {
    fn push(&mut self, r: Option<Option<u32>>) {
        if self.hist.len() != HEALTH_SAMPLES {
            self.hist = vec![-1.0; HEALTH_SAMPLES];
        }
        self.hist.remove(0);
        self.hist.push(match r {
            None => -1.0,
            Some(None) => 0.0,
            Some(Some(ms)) => {
                self.ever = true;
                ms as f32 + 0.5
            }
        });
    }
    /// (fracción perdida, mediana en ms) de las últimas `n` muestras; None si hay menos de 3
    pub fn recent(&self, n: usize) -> Option<(f32, f32)> {
        let v: Vec<f32> = self.hist.iter().rev().take(n).copied().filter(|x| *x >= 0.0).collect();
        if v.len() < 3 {
            return None;
        }
        let lost = v.iter().filter(|x| **x == 0.0).count() as f32 / v.len() as f32;
        let mut ok: Vec<f32> = v.into_iter().filter(|x| *x > 0.0).collect();
        ok.sort_by(|a, b| a.total_cmp(b));
        Some((lost, ok.get(ok.len() / 2).copied().unwrap_or(0.0)))
    }
}

#[derive(Clone, Default)]
pub struct Health {
    pub router: Hop,
    pub dns: Hop,
    pub internet: Hop,
    pub wifi: bool,
    /// no hay ningún adaptador conectado con puerta de enlace
    pub no_net: bool,
    /// ya se hizo al menos una ronda
    pub started: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Unknown,
    Ok,
    Warn,
    Bad,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cause {
    None,
    NoNetwork,
    /// el router no responde
    RouterDown,
    /// el router responde lento o con pérdidas (Wi-Fi o red local)
    Local,
    NoInternet,
    InternetLoss,
    InternetSlow,
    Dns,
}

impl Health {
    /// Diagnóstico con los últimos 20 s: se mira de cerca a lejos y se culpa al primer tramo que
    /// falla (si el router va mal, lo de detrás también, y no es culpa del proveedor)
    pub fn verdict(&self) -> (Level, Cause) {
        const N: usize = 10;
        if self.no_net {
            return (Level::Bad, Cause::NoNetwork);
        }
        if !self.started {
            return (Level::Unknown, Cause::None);
        }
        let hop = |h: &Hop| if h.ever { h.recent(N) } else { None };
        let (r, d, i) = (hop(&self.router), hop(&self.dns), hop(&self.internet));
        if r.is_none() && i.is_none() && self.internet.recent(N).is_none() {
            return (Level::Unknown, Cause::None);
        }
        if let Some((loss, ms)) = r {
            if loss >= 0.99 {
                return (Level::Bad, Cause::RouterDown);
            }
            if loss >= 0.3 || ms >= 100.0 {
                return (Level::Bad, Cause::Local);
            }
            if loss >= 0.1 || ms >= 30.0 {
                return (Level::Warn, Cause::Local);
            }
        }
        match i {
            Some((loss, ms)) => {
                if loss >= 0.99 {
                    return (Level::Bad, Cause::NoInternet);
                }
                if loss >= 0.3 {
                    return (Level::Bad, Cause::InternetLoss);
                }
                if ms >= 400.0 {
                    return (Level::Bad, Cause::InternetSlow);
                }
                if loss >= 0.1 {
                    return (Level::Warn, Cause::InternetLoss);
                }
                if ms >= 150.0 {
                    return (Level::Warn, Cause::InternetSlow);
                }
            }
            // Internet no ha respondido nunca a un ping (red que filtra ICMP): no se puede evaluar
            None if !self.internet.ever => return (if r.is_some() { Level::Ok } else { Level::Unknown }, Cause::None),
            None => {}
        }
        if let Some((loss, ms)) = d {
            if loss >= 0.3 || ms >= 150.0 {
                return (Level::Warn, Cause::Dns);
            }
        }
        (Level::Ok, Cause::None)
    }
}

pub static HEALTH: LazyLock<Mutex<Health>> = LazyLock::new(|| Mutex::new(Health::default()));
static HEALTH_ON: AtomicBool = AtomicBool::new(true);

/// Activa/desactiva el sondeo de salud (clave `NetHealth`); al desactivarlo se borra lo medido
pub fn set_health(on: bool) {
    if HEALTH_ON.swap(on, Ordering::Relaxed) && !on {
        *HEALTH.lock().unwrap() = Health::default();
    }
}
pub fn health_on() -> bool {
    HEALTH_ON.load(Ordering::Relaxed)
}

/// Sondeo continuo y ligero (tres pings cada 2 s): router, DNS e Internet
pub fn spawn_health_worker() {
    std::thread::spawn(|| {
        crate::util::prof("health");
        let icmp = unsafe { IcmpCreateFile() }.ok();
        let mut info: Option<NetInfo> = None;
        let mut info_at: Option<Instant> = None;
        loop {
            let Some(icmp) = icmp else { return };
            if !health_on() {
                info_at = None;
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            let start = Instant::now();
            if info_at.map(|t| t.elapsed() > Duration::from_secs(10)).unwrap_or(true) {
                info = read_info();
                info_at = Some(Instant::now());
            }
            let gw = info.as_ref().map(|i| i.gateway.clone());
            // DNS: el primero IPv4 que no sea el propio router (ese tramo ya se mide)
            let dns = info.as_ref().and_then(|i| i.dns.iter().find(|d| d.contains('.') && Some(*d) != gw.as_ref()).cloned());
            let r_gw = gw.as_ref().map(|g| ping(icmp, g, 800));
            let r_dns = dns.as_ref().map(|d| ping(icmp, d, 800));
            let r_net = gw.as_ref().map(|_| INTERNET.iter().find_map(|t| ping(icmp, t, 800)));
            if r_gw == Some(None) {
                info_at = None; // el router no responde: se relee la red en la siguiente ronda
            }
            if health_on() {
                let mut h = HEALTH.lock().unwrap();
                h.no_net = info.is_none();
                h.wifi = info.as_ref().map(|i| i.wifi).unwrap_or(false);
                h.router.target = gw.unwrap_or_default();
                h.dns.target = dns.unwrap_or_default();
                h.internet.target = INTERNET[0].into();
                h.router.push(r_gw);
                h.dns.push(r_dns);
                h.internet.push(r_net);
                h.started = true;
            }
            let el = start.elapsed();
            if el < HEALTH_EVERY {
                std::thread::sleep(HEALTH_EVERY - el);
            }
        }
    });
}

pub fn spawn_net_worker() {
    std::thread::spawn(|| {
        crate::util::prof("ping");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Tramo con `n` muestras recientes: latencia fija y una fracción perdida al final
    fn hop(ms: u32, lost: usize, n: usize) -> Hop {
        let mut h = Hop { target: "x".into(), ..Default::default() };
        for i in 0..n {
            h.push(Some(if i >= n - lost { None } else { Some(ms) }));
        }
        h
    }
    fn health(router: Hop, dns: Hop, internet: Hop) -> Health {
        Health { router, dns, internet, wifi: true, no_net: false, started: true }
    }

    #[test]
    fn estable() {
        assert_eq!(health(hop(2, 0, 10), hop(12, 0, 10), hop(15, 0, 10)).verdict(), (Level::Ok, Cause::None));
    }

    #[test]
    fn sin_muestras_aun() {
        assert_eq!(Health::default().verdict(), (Level::Unknown, Cause::None));
        assert_eq!(health(hop(2, 0, 2), Hop::default(), hop(15, 0, 2)).verdict(), (Level::Unknown, Cause::None));
    }

    #[test]
    fn sin_red() {
        let mut h = health(hop(2, 0, 10), Hop::default(), hop(15, 0, 10));
        h.no_net = true;
        assert_eq!(h.verdict(), (Level::Bad, Cause::NoNetwork));
    }

    #[test]
    fn el_router_tiene_prioridad_sobre_internet() {
        // si el router pierde paquetes, Internet también: la culpa es local
        assert_eq!(health(hop(3, 4, 10), Hop::default(), hop(20, 5, 10)).verdict(), (Level::Bad, Cause::Local));
        assert_eq!(health(hop(40, 0, 10), Hop::default(), hop(60, 0, 10)).verdict(), (Level::Warn, Cause::Local));
        assert_eq!(health(hop(3, 10, 12), Hop::default(), hop(20, 10, 12)).verdict(), (Level::Bad, Cause::RouterDown));
    }

    #[test]
    fn proveedor() {
        assert_eq!(health(hop(2, 0, 10), Hop::default(), hop(20, 10, 12)).verdict(), (Level::Bad, Cause::NoInternet));
        assert_eq!(health(hop(2, 0, 10), Hop::default(), hop(20, 4, 10)).verdict(), (Level::Bad, Cause::InternetLoss));
        assert_eq!(health(hop(2, 0, 10), Hop::default(), hop(20, 1, 10)).verdict(), (Level::Warn, Cause::InternetLoss));
        assert_eq!(health(hop(2, 0, 10), Hop::default(), hop(200, 0, 10)).verdict(), (Level::Warn, Cause::InternetSlow));
        assert_eq!(health(hop(2, 0, 10), Hop::default(), hop(500, 0, 10)).verdict(), (Level::Bad, Cause::InternetSlow));
    }

    #[test]
    fn dns_lento() {
        assert_eq!(health(hop(2, 0, 10), hop(300, 0, 10), hop(20, 0, 10)).verdict(), (Level::Warn, Cause::Dns));
    }

    #[test]
    fn destinos_que_no_admiten_ping_no_cuentan() {
        // router que nunca responde a ping pero Internet va bien
        assert_eq!(health(hop(0, 10, 10), Hop::default(), hop(20, 0, 10)).verdict(), (Level::Ok, Cause::None));
        // red que filtra ICMP hacia fuera: no se puede culpar al proveedor
        assert_eq!(health(hop(2, 0, 10), Hop::default(), hop(0, 10, 10)).verdict(), (Level::Ok, Cause::None));
        // nada responde nunca: desconocido, no rojo
        assert_eq!(health(hop(0, 10, 10), Hop::default(), hop(0, 10, 10)).verdict(), (Level::Unknown, Cause::None));
    }
}
