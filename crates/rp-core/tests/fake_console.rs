//! End-to-end discovery against a fake console on localhost.
//!
//! The fake answers chiaki's real discovery request on the PS5 discovery port
//! (9302), so this exercises the whole path: chiaki-ng's C code, the FFI layer
//! and our parsing.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use rp_core::{ConsoleKind, ConsoleState, DiscoveryOptions, discover};

const PS5_DISCOVERY_PORT: u16 = 9302;

fn spawn_fake_console(
    status: &'static str,
) -> (Arc<AtomicBool>, thread::JoinHandle<Option<String>>) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, PS5_DISCOVERY_PORT))
        .expect("port 9302 must be free for this test");
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();

    let handle = thread::spawn(move || {
        let mut request = None;
        let mut buf = [0u8; 512];
        while !stop_flag.load(Ordering::Relaxed) {
            let Ok((n, from)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let text = String::from_utf8_lossy(&buf[..n]).into_owned();
            if !text.starts_with("SRCH") {
                continue;
            }
            request.get_or_insert(text);
            let reply = format!(
                "HTTP/1.1 {status}\n\
                 host-id:ABCDEF012345\n\
                 host-type:PS5\n\
                 host-name:Test Console\n\
                 host-request-port:997\n\
                 device-discovery-protocol-version:00030010\n\
                 system-version:09400001\n\n"
            );
            socket.send_to(reply.as_bytes(), from).unwrap();
        }
        request
    });
    (stop, handle)
}

#[test]
fn discovers_fake_console_ready_then_standby() {
    // Both cases in one test because they share the fixed port.
    for (status, expected) in [
        ("200 Ok", ConsoleState::Ready),
        ("620 Server Standby", ConsoleState::Standby),
    ] {
        let (stop, handle) = spawn_fake_console(status);

        let options = DiscoveryOptions {
            host: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ..Default::default()
        };
        let consoles = discover(Duration::from_secs(3), &options).expect("discovery failed");

        stop.store(true, Ordering::Relaxed);
        let request = handle
            .join()
            .unwrap()
            .expect("fake console never got a SRCH request");
        assert!(
            request.contains("device-discovery-protocol-version:00030010"),
            "request: {request}"
        );

        assert_eq!(consoles.len(), 1, "consoles: {consoles:?}");
        let c = &consoles[0];
        assert_eq!(c.name, "Test Console");
        assert_eq!(c.host_id, "ABCDEF012345");
        assert_eq!(c.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(c.state, expected);
        assert_eq!(c.kind, ConsoleKind::Ps5);
        assert_eq!(c.request_port, 997);
    }
}
