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

/// This machine's Tailscale address, or `None` when Tailscale is not up.
///
/// Same trick as [`lan_ipv4`]: a UDP `connect` is only a routing-table lookup.
/// The destination is Tailscale's own service address, inside the CGNAT range
/// the daemon routes through `tailscale0`, so a successful lookup names the
/// address on that interface.
///
/// The range check is what makes this honest. Without Tailscale there is still
/// a default route, so `connect` succeeds and reports the *LAN* address; only
/// an answer inside `100.64.0.0/10` proves the traffic really left through the
/// tunnel. Userspace-networking mode has no interface to name and therefore
/// reports nothing — which is the truthful answer, since there is no address
/// anyone could type into a phone.
pub fn tailscale_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(100, 100, 100, 100), 9)).ok()?;
    let addr = match socket.local_addr().ok()? {
        SocketAddr::V4(addr) => *addr.ip(),
        _ => return None,
    };
    let [first, second, _, _] = addr.octets();
    (first == 100 && (64..128).contains(&second)).then_some(addr)
}

/// The same page over Tailscale, for a phone that is not on the same Wi-Fi.
pub fn tailscale_url(port: u16) -> Option<String> {
    tailscale_ipv4().map(|ip| format!("http://{ip}:{port}"))
}

/// The address a phone should get, token and all.
///
/// The tailnet address when Tailscale is up, because it keeps working when the
/// phone leaves the house and costs nothing when it is home; the LAN address
/// otherwise. The token rides in the query string so that scanning the QR code
/// both opens the page and authorises the device.
pub fn phone_url(port: u16, token: Option<&str>) -> String {
    let base = tailscale_url(port).unwrap_or_else(|| remote_url(port));
    match token.map(str::trim).filter(|token| !token.is_empty()) {
        Some(token) => format!("{base}/?token={token}"),
        None => base,
    }
}

/// A fresh remote token: 128 bits of randomness, written as hex.
///
/// Long enough that guessing it is pointless, short enough to sit in a URL. It
/// is a bearer credential — anyone holding it gets the same control as the
/// person at the keyboard — so it is only ever handed out on this screen.
pub fn new_remote_token() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    format!("{:016x}{:016x}", rng.random::<u64>(), rng.random::<u64>())
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

    #[test]
    fn tailscale_address_is_only_reported_inside_the_cgnat_range() {
        // The dangerous failure is reporting the LAN address here, which would
        // put a URL on screen that stops working the moment the phone leaves
        // the house.
        if let Some(ip) = tailscale_ipv4() {
            let [first, second, _, _] = ip.octets();
            assert_eq!(first, 100, "{ip}");
            assert!((64..128).contains(&second), "{ip}");
        }
        if let Some(url) = tailscale_url(9999) {
            assert!(url.starts_with("http://100."), "{url}");
            assert!(url.ends_with(":9999"), "{url}");
        }
    }
}
