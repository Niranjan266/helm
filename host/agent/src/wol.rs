//! Wake-on-LAN sender (`wol.send`, PROTOCOL §12): an awake agent wakes another PC on its LAN.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

/// UDP port for magic packets ("discard").
pub const WOL_PORT: u16 = 9;

/// Where to send: the limited broadcast plus each LAN interface's directed broadcast.
/// Returns `(local source address, destination)` pairs; a `None` source = any interface.
pub fn targets() -> Vec<(Option<Ipv4Addr>, Ipv4Addr)> {
    let mut out: Vec<(Option<Ipv4Addr>, Ipv4Addr)> = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                if v4.ip.is_loopback() || v4.ip.is_link_local() {
                    continue;
                }
                out.push((Some(v4.ip), Ipv4Addr::BROADCAST));
                if let Some(b) = v4.broadcast
                    && b != Ipv4Addr::BROADCAST
                {
                    out.push((Some(v4.ip), b));
                }
            }
        }
    }
    if out.is_empty() {
        out.push((None, Ipv4Addr::BROADCAST));
    }
    out.dedup();
    out
}

/// Broadcast `packet` on every LAN interface. Succeeds if at least one send worked.
pub fn broadcast(packet: &[u8; 102]) -> std::io::Result<usize> {
    let mut sent = 0;
    let mut last_err = None;
    for (src, dst) in targets() {
        let r = (|| {
            let sock = UdpSocket::bind(SocketAddrV4::new(src.unwrap_or(Ipv4Addr::UNSPECIFIED), 0))?;
            sock.set_broadcast(true)?;
            sock.send_to(packet, SocketAddrV4::new(dst, WOL_PORT))
        })();
        match r {
            Ok(_) => sent += 1,
            Err(e) => last_err = Some(e),
        }
    }
    match (sent, last_err) {
        (0, Some(e)) => Err(e),
        _ => Ok(sent),
    }
}
