//! Small network helpers for the UI.
//!
//! Nothing here talks to the network. The remote-control web server binds to
//! every interface, so the address the user actually needs is the machine's LAN
//! address — `localhost` only means anything to the machine itself, which is
//! useless for the case the remote exists for: opening the page on a phone.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// This machine's LAN address, or `None` when there is no usable route.
///
/// The kernel already knows which local address it would source traffic from,
/// and `connect`ing a UDP socket makes it say so. Nothing is transmitted: for
/// UDP, `connect` is a pure routing-table lookup that sets the socket's default
/// peer without putting a packet on the wire. That keeps this dependency-free —
/// no `getifaddrs` crate, no shelling out to `ip addr`.
///
/// The destination sits in the TEST-NET-1 documentation range (RFC 5737), so
/// even a stray packet could not reach a real service.
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(addr) if !addr.ip().is_loopback() => Some(*addr.ip()),
        _ => None,
    }
}

/// The URL to open the remote-control page at.
///
/// The LAN address when there is one, so the same string can be typed into a
/// phone, and `localhost` as the fallback for a machine with no network — where
/// the remote is still reachable, just only from itself.
///
/// Called from a render closure, which means twice per frame while the remote
/// group is on screen. That is fine: two syscalls on an unbound socket, with no
/// traffic either way. Caching it would be the wrong trade — a laptop that
/// changes network would then show a stale address.
pub fn remote_url(port: u16) -> String {
    match lan_ipv4() {
        Some(ip) => format!("http://{ip}:{port}"),
        None => format!("http://localhost:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_url_is_always_a_usable_http_url() {
        let url = remote_url(9999);
        assert!(url.starts_with("http://"), "{url}");
        let (host, port) = url
            .trim_start_matches("http://")
            .rsplit_once(':')
            .expect("host:port");
        assert_eq!(port, "9999");
        if host != "localhost" {
            host.parse::<Ipv4Addr>().expect("a real address");
        }
    }

    #[test]
    fn lan_address_is_never_loopback_or_unspecified() {
        // `None` is a legitimate answer on a machine with no network at all.
        if let Some(ip) = lan_ipv4() {
            assert!(!ip.is_loopback(), "{ip}");
            assert!(!ip.is_unspecified(), "{ip}");
        }
    }
}
