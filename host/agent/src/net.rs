//! LAN IPv4 addresses to put in the pairing link and mDNS record.

use std::net::Ipv4Addr;

/// Adapter names that are almost always virtual (VMs, containers, VPN-ish bridges).
const VIRTUAL_HINTS: &[&str] = &[
    "vethernet",
    "virtualbox",
    "vmware",
    "hyper-v",
    "wsl",
    "docker",
    "vbox",
    "loopback",
    "npcap",
    "vpn",
];

fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private()
}

fn looks_virtual(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    VIRTUAL_HINTS.iter().any(|h| lower.contains(h))
}

/// Order candidate addresses: the address of the default route first (if given), then private
/// ranges, then the rest; skip loopback, link-local, unspecified and duplicates. Virtual
/// adapters are dropped unless nothing else is left.
pub fn rank_addresses(
    candidates: &[(String, Ipv4Addr)],
    primary: Option<Ipv4Addr>,
) -> Vec<Ipv4Addr> {
    let usable: Vec<&(String, Ipv4Addr)> = candidates
        .iter()
        .filter(|(_, ip)| {
            !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() && !ip.is_broadcast()
        })
        .collect();
    let real: Vec<&(String, Ipv4Addr)> = usable
        .iter()
        .copied()
        .filter(|(name, _)| !looks_virtual(name))
        .collect();
    let chosen = if real.is_empty() { usable } else { real };
    let mut out: Vec<Ipv4Addr> = Vec::new();
    if let Some(p) = primary
        && chosen.iter().any(|(_, ip)| *ip == p)
    {
        out.push(p);
    }
    for pass_private in [true, false] {
        for (_, ip) in &chosen {
            if is_private(*ip) == pass_private && !out.contains(ip) {
                out.push(*ip);
            }
        }
    }
    out
}

/// Adapter name / description hints of a phone's USB tethering link as Windows sees it
/// (Android "Remote NDIS based Internet Sharing Device", NCM, iPhone "Apple Mobile Device
/// Ethernet").
const USB_TETHER_HINTS: &[&str] = &[
    "remote ndis",
    "rndis",
    "usb ethernet",
    "ncm",
    "android",
    "apple mobile device ethernet",
    "iphone usb",
];

/// g4: does this adapter look like a phone's USB tethering link?
pub fn is_usb_tether(name: &str, description: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let d = description.to_ascii_lowercase();
    USB_TETHER_HINTS
        .iter()
        .any(|h| n.contains(h) || d.contains(h))
}

/// [`rank_addresses`], then USB-tethering addresses (`usb`) moved to the front: a phone
/// plugged in by cable reaches the PC that way even without Wi-Fi (g4 USB pairing).
pub fn rank_addresses_usb_first(
    candidates: &[(String, Ipv4Addr)],
    primary: Option<Ipv4Addr>,
    usb: &[Ipv4Addr],
) -> Vec<Ipv4Addr> {
    let ranked = rank_addresses(candidates, primary);
    let (mut first, rest): (Vec<Ipv4Addr>, Vec<Ipv4Addr>) =
        ranked.into_iter().partition(|ip| usb.contains(ip));
    first.extend(rest);
    first
}

/// IPv4 addresses on USB tethering adapters (Windows: by adapter description).
pub fn usb_tether_addresses() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let descriptions: Vec<(String, String)> = helm_win_system::net::mac_addresses()
        .map(|list| {
            list.into_iter()
                .map(|a| (a.adapter_name, a.description))
                .collect()
        })
        .unwrap_or_default();
    interfaces
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_loopback())
        .filter_map(|i| {
            let if_addrs::IfAddr::V4(v4) = &i.addr else {
                return None;
            };
            let desc = descriptions
                .iter()
                .find(|(n, _)| *n == i.name)
                .map(|(_, d)| d.as_str())
                .unwrap_or("");
            is_usb_tether(&i.name, desc).then_some(v4.ip)
        })
        .collect()
}

/// Non-loopback IPv4 addresses of interfaces that are up: USB tethering first, then LAN.
pub fn lan_ipv4_addresses() -> Vec<Ipv4Addr> {
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!("could not list network interfaces: {e}");
            return Vec::new();
        }
    };
    let candidates: Vec<(String, Ipv4Addr)> = interfaces
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_loopback())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) => Some((i.name, v4.ip)),
            _ => None,
        })
        .collect();
    rank_addresses_usb_first(&candidates, default_route_address(), &usb_tether_addresses())
}

/// The local address the OS would use to reach the internet (the "main" LAN address).
/// Connecting a UDP socket sends no packets; it only asks the routing table.
fn default_route_address() -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) if !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_private_first_and_skips_junk() {
        let c = vec![
            ("Ethernet".to_string(), Ipv4Addr::new(203, 0, 113, 9)),
            ("Wi-Fi".to_string(), Ipv4Addr::new(192, 168, 1, 7)),
            ("vEthernet (WSL)".to_string(), Ipv4Addr::new(172, 20, 0, 1)),
            ("lo".to_string(), Ipv4Addr::new(127, 0, 0, 1)),
            ("Wi-Fi 2".to_string(), Ipv4Addr::new(169, 254, 3, 3)),
            ("Wi-Fi".to_string(), Ipv4Addr::new(192, 168, 1, 7)),
        ];
        assert_eq!(
            rank_addresses(&c, None),
            vec![Ipv4Addr::new(192, 168, 1, 7), Ipv4Addr::new(203, 0, 113, 9)]
        );
    }

    #[test]
    fn default_route_address_goes_first() {
        let c = vec![
            ("Ethernet 2".to_string(), Ipv4Addr::new(192, 168, 56, 1)),
            ("Wi-Fi".to_string(), Ipv4Addr::new(192, 168, 31, 56)),
            ("Radmin VPN".to_string(), Ipv4Addr::new(26, 1, 2, 3)),
        ];
        assert_eq!(
            rank_addresses(&c, Some(Ipv4Addr::new(192, 168, 31, 56))),
            vec![
                Ipv4Addr::new(192, 168, 31, 56),
                Ipv4Addr::new(192, 168, 56, 1)
            ]
        );
    }

    #[test]
    fn usb_tethering_goes_first() {
        assert!(is_usb_tether(
            "Ethernet 3",
            "Remote NDIS based Internet Sharing Device"
        ));
        assert!(is_usb_tether("Ethernet 4", "Apple Mobile Device Ethernet"));
        assert!(is_usb_tether("rndis0", ""));
        assert!(!is_usb_tether("Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz"));
        assert!(!is_usb_tether("Ethernet", "Realtek PCIe GbE Family Controller"));
        let c = vec![
            ("Wi-Fi".to_string(), Ipv4Addr::new(192, 168, 1, 7)),
            ("Ethernet 3".to_string(), Ipv4Addr::new(192, 168, 42, 33)),
        ];
        let usb = [Ipv4Addr::new(192, 168, 42, 33)];
        assert_eq!(
            rank_addresses_usb_first(&c, Some(Ipv4Addr::new(192, 168, 1, 7)), &usb),
            vec![Ipv4Addr::new(192, 168, 42, 33), Ipv4Addr::new(192, 168, 1, 7)]
        );
        // A USB address that the ranking dropped (e.g. link-local) is not resurrected.
        let usb = [Ipv4Addr::new(169, 254, 1, 1)];
        assert_eq!(
            rank_addresses_usb_first(&c, None, &usb),
            vec![Ipv4Addr::new(192, 168, 1, 7), Ipv4Addr::new(192, 168, 42, 33)]
        );
    }

    #[test]
    fn keeps_virtual_if_nothing_else() {
        let c = vec![("vEthernet".to_string(), Ipv4Addr::new(172, 20, 0, 1))];
        assert_eq!(rank_addresses(&c, None), vec![Ipv4Addr::new(172, 20, 0, 1)]);
    }
}
