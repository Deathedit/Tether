//! Finding consoles on the local network.
//!
//! Wraps chiaki-ng's `ChiakiDiscoveryService`: it pings the network every
//! `ping_interval` and reports the full list of consoles whenever it changes.

use std::ffi::c_void;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chiaki_sys as sys;
use crossbeam_channel::{Receiver, Sender};

use crate::error::{Result, check};
use crate::ffi;
use crate::target::Target;

/// Whether a console is on or in rest mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsoleState {
    Ready,
    /// Rest mode: the console can be woken over the network.
    Standby,
    Unknown,
}

impl ConsoleState {
    fn from_raw(state: sys::ChiakiDiscoveryHostState) -> Self {
        match state {
            sys::CHIAKI_DISCOVERY_HOST_STATE_READY => Self::Ready,
            sys::CHIAKI_DISCOVERY_HOST_STATE_STANDBY => Self::Standby,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for ConsoleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ready => "ready",
            Self::Standby => "rest mode",
            Self::Unknown => "unknown",
        })
    }
}

/// Console generation, from the discovery protocol version it answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsoleKind {
    Ps5,
    Ps4,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningApp {
    pub title_id: Option<String>,
    pub name: Option<String>,
}

/// A console that answered a discovery request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Console {
    /// Stable unique ID of the console (hex string).
    pub host_id: String,
    /// User-visible name set on the console.
    pub name: String,
    pub addr: IpAddr,
    pub state: ConsoleState,
    pub kind: ConsoleKind,
    /// Protocol variant, from the console kind and firmware version.
    pub target: Target,
    /// Raw `host-type` value as reported by the console.
    pub host_type: Option<String>,
    pub system_version: Option<String>,
    /// Port for registration and session requests.
    pub request_port: u16,
    pub running_app: Option<RunningApp>,
}

impl Console {
    /// # Safety
    /// `host` must be a valid host from chiaki's discovery service.
    unsafe fn from_raw(host: &mut sys::ChiakiDiscoveryHost) -> Option<Self> {
        // SAFETY (all opt_string calls): chiaki's strings are null or valid C strings.
        let addr = unsafe { ffi::opt_string(host.host_addr) }?.parse().ok()?;
        let host_id = unsafe { ffi::opt_string(host.host_id) }.unwrap_or_default();
        let name = unsafe { ffi::opt_string(host.host_name) }.unwrap_or_else(|| host_id.clone());
        let title_id = unsafe { ffi::opt_string(host.running_app_titleid) };
        let app_name = unsafe { ffi::opt_string(host.running_app_name) };
        // SAFETY: valid host pointer; the function only reads it.
        let is_ps5 = unsafe { sys::chiaki_discovery_host_is_ps5(host) };
        let target = if host.system_version.is_null() {
            // chiaki's version parser would call atoi(NULL).
            if is_ps5 {
                Target::PS5_1
            } else {
                Target::PS4_UNKNOWN
            }
        } else {
            // SAFETY: valid host with a non-null system_version.
            Target::from_sys(unsafe { sys::chiaki_discovery_host_system_version_target(host) })
        };

        Some(Console {
            host_id,
            name,
            addr,
            state: ConsoleState::from_raw(host.state),
            kind: if is_ps5 {
                ConsoleKind::Ps5
            } else {
                ConsoleKind::Ps4
            },
            target,
            host_type: unsafe { ffi::opt_string(host.host_type) },
            system_version: unsafe { ffi::opt_string(host.system_version) },
            request_port: host.host_request_port,
            running_app: (title_id.is_some() || app_name.is_some()).then_some(RunningApp {
                title_id,
                name: app_name,
            }),
        })
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Ask only this address. `None` broadcasts on every IPv4 interface.
    pub host: Option<IpAddr>,
    pub ping_interval: Duration,
    /// A console is dropped from the list after missing this many pings.
    pub drop_after_missed_pings: u64,
    pub max_consoles: usize,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            host: None,
            ping_interval: Duration::from_millis(500),
            drop_after_missed_pings: 3,
            max_consoles: 16,
        }
    }
}

/// State shared with chiaki's discovery thread.
struct Shared {
    tx: Sender<Vec<Console>>,
    latest: Mutex<Vec<Console>>,
}

/// A running discovery service. Stops when dropped.
pub struct Discovery {
    service: Box<sys::ChiakiDiscoveryService>,
    shared: Box<Shared>,
    rx: Receiver<Vec<Console>>,
}

// SAFETY: the service is driven by its own thread and guarded by its own
// mutex; this handle only calls `fini` on drop.
unsafe impl Send for Discovery {}

impl Discovery {
    pub fn start(options: &DiscoveryOptions) -> Result<Self> {
        let log = ffi::log();
        let (tx, rx) = crossbeam_channel::unbounded();
        let shared = Box::new(Shared {
            tx,
            latest: Mutex::new(Vec::new()),
        });

        // Where pings go. 255.255.255.255 makes chiaki also send to every
        // address in `broadcast_addrs`.
        let send_ip = options.host.unwrap_or(IpAddr::V4(Ipv4Addr::BROADCAST));
        let (mut send_addr, send_addr_size) = ffi::to_sockaddr_storage(SocketAddr::new(send_ip, 0));

        let mut broadcast_addrs: Vec<sys::sockaddr_storage> = if options.host.is_none() {
            interface_broadcast_addrs()
                .into_iter()
                .map(|ip| ffi::to_sockaddr_storage(SocketAddr::V4(SocketAddrV4::new(ip, 0))).0)
                .collect()
        } else {
            Vec::new()
        };
        tracing::debug!(host = ?options.host, broadcasts = broadcast_addrs.len(), "starting discovery");

        let mut raw_options = sys::ChiakiDiscoveryServiceOptions {
            hosts_max: options.max_consoles,
            host_drop_pings: options.drop_after_missed_pings,
            ping_ms: options.ping_interval.as_millis() as u64,
            // A short delay before the first ping gives us time to fix up the
            // socket (see `disable_udp_connreset`).
            ping_initial_ms: 50,
            send_addr: &mut send_addr,
            send_addr_size,
            broadcast_addrs: if broadcast_addrs.is_empty() {
                std::ptr::null_mut()
            } else {
                broadcast_addrs.as_mut_ptr()
            },
            broadcast_num: broadcast_addrs.len(),
            send_host: std::ptr::null_mut(),
            cb: Some(hosts_cb),
            cb_user: &*shared as *const Shared as *mut c_void,
        };

        let mut service = Box::new(sys::ChiakiDiscoveryService::default());
        // SAFETY: `service` is boxed so its address is stable for chiaki's
        // thread. chiaki copies `send_addr` and `broadcast_addrs`, so the
        // locals may go away after this call. `shared` outlives the service
        // (see Drop).
        check(unsafe { sys::chiaki_discovery_service_init(&mut *service, &mut raw_options, log) })?;

        #[cfg(windows)]
        disable_udp_connreset(service.discovery.socket as usize);

        Ok(Self {
            service,
            shared,
            rx,
        })
    }

    /// Receives the full console list each time it changes.
    pub fn updates(&self) -> &Receiver<Vec<Console>> {
        &self.rx
    }

    /// The most recent console list.
    pub fn consoles(&self) -> Vec<Console> {
        self.shared.latest.lock().unwrap().clone()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        // SAFETY: the service was initialised in `start`. `fini` joins chiaki's
        // threads, so no callback can touch `shared` after this.
        unsafe { sys::chiaki_discovery_service_fini(&mut *self.service) };
    }
}

unsafe extern "C" fn hosts_cb(
    hosts: *mut sys::ChiakiDiscoveryHost,
    count: usize,
    user: *mut c_void,
) {
    // SAFETY: `user` is the `Shared` passed in `start`, alive until `fini`.
    let shared = unsafe { &*(user as *const Shared) };
    let consoles: Vec<Console> = if hosts.is_null() || count == 0 {
        Vec::new()
    } else {
        // SAFETY: chiaki passes `count` valid hosts, locked for this call.
        unsafe { std::slice::from_raw_parts_mut(hosts, count) }
            .iter_mut()
            .filter_map(|h| unsafe { Console::from_raw(h) })
            .collect()
    };
    *shared.latest.lock().unwrap() = consoles.clone();
    let _ = shared.tx.send(consoles);
}

/// Scans for consoles for up to `timeout` and returns what answered.
///
/// With `options.host` set, returns as soon as that console answers.
pub fn discover(timeout: Duration, options: &DiscoveryOptions) -> Result<Vec<Console>> {
    let discovery = Discovery::start(options)?;
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match discovery.updates().recv_timeout(left) {
            Ok(list) if options.host.is_some() && !list.is_empty() => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let mut consoles = discovery.consoles();
    consoles.sort_by(|a, b| a.name.cmp(&b.name).then(a.addr.cmp(&b.addr)));
    Ok(consoles)
}

/// Broadcast addresses of every non-loopback IPv4 interface.
fn interface_broadcast_addrs() -> Vec<Ipv4Addr> {
    let ifaces = match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces,
        Err(e) => {
            tracing::warn!("could not list network interfaces: {e}");
            return Vec::new();
        }
    };
    let mut addrs: Vec<Ipv4Addr> = ifaces
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) => v4.broadcast,
            _ => None,
        })
        .filter(|b| *b != Ipv4Addr::BROADCAST)
        .collect();
    addrs.sort();
    addrs.dedup();
    addrs
}

/// On Windows, an ICMP "port unreachable" reply makes the next `recvfrom` on a
/// UDP socket fail with WSAECONNRESET, and chiaki's discovery thread exits on
/// any receive error. That happens when we ping a single host that doesn't
/// listen on one of the two discovery ports. Turn that behaviour off.
#[cfg(windows)]
fn disable_udp_connreset(socket: usize) {
    const SIO_UDP_CONNRESET: u32 = 0x9800_000C;
    unsafe extern "system" {
        fn WSAIoctl(
            s: usize,
            code: u32,
            in_buf: *const c_void,
            in_len: u32,
            out_buf: *mut c_void,
            out_len: u32,
            bytes_returned: *mut u32,
            overlapped: *mut c_void,
            completion: *const c_void,
        ) -> i32;
    }
    let enable: u32 = 0;
    let mut returned = 0u32;
    // SAFETY: valid socket from chiaki; buffers are valid for the given sizes.
    let r = unsafe {
        WSAIoctl(
            socket,
            SIO_UDP_CONNRESET,
            &enable as *const u32 as *const c_void,
            4,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if r != 0 {
        tracing::debug!(
            "SIO_UDP_CONNRESET failed: {}",
            std::io::Error::last_os_error()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_display() {
        assert_eq!(ConsoleState::Ready.to_string(), "ready");
        assert_eq!(ConsoleState::Standby.to_string(), "rest mode");
    }
}
