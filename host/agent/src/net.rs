//! LAN IPv4 addresses to put in the pairing link and mDNS record.

use std::net::Ipv4Addr;

/// Words (split on anything that is not a letter or digit) that mark a virtual or VPN adapter.
const VIRTUAL_WORDS: &[&str] = &["tun", "tap", "wsl", "vbox", "vmnet", "wintun"];

/// Substrings of an adapter name or driver description that mark a virtual, VPN or tunnel
/// adapter: VirtualBox Host-Only, VMware, Hyper-V vEthernet, WSL, Docker, Radmin, Hamachi,
/// ZeroTier, Tailscale, WireGuard, TAP/TUN, generic VPNs.
const VIRTUAL_HINTS: &[&str] = &[
    "vethernet",
    "virtualbox",
    "host-only",
    "vmware",
    "hyper-v",
    "docker",
    "loopback",
    "npcap",
    "vpn",
    "radmin",
    "hamachi",
    "zerotier",
    "tailscale",
    "wireguard",
    "tap-windows",
    "openvpn",
    "virtual adapter",
    "virtual ethernet",
];

/// IANA ifTypes that are never a real LAN link: software loopback, tunnel, propVirtual.
const VIRTUAL_IF_TYPES: &[u32] = &[24, 131, 53];

/// An IPv4 address with what Windows says about the adapter that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterAddr {
    pub name: String,
    /// Driver description (`VirtualBox Host-Only Ethernet Adapter`); empty when unknown.
    pub description: String,
    /// IANA ifType; 0 when unknown.
    pub if_type: u32,
    pub ip: Ipv4Addr,
}

fn is_private(ip: Ipv4Addr) -> bool {
    ip.is_private()
}

/// Is this adapter virtual, a VPN or a tunnel, judged by name, description and type (not the
/// name alone: Windows names a VirtualBox adapter just `Ethernet 2`).
pub fn is_virtual_adapter(a: &AdapterAddr) -> bool {
    if VIRTUAL_IF_TYPES.contains(&a.if_type) {
        return true;
    }
    let text = format!("{} {}", a.name, a.description).to_ascii_lowercase();
    if VIRTUAL_HINTS.iter().any(|h| text.contains(h)) {
        return true;
    }
    if text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| VIRTUAL_WORDS.contains(&w))
    {
        return true;
    }
    // The VirtualBox host-only default network.
    let o = a.ip.octets();
    o[..3] == [192, 168, 56] && text.contains("virtualbox")
}

/// Order candidate addresses: the address of the default route first (if given), then private
/// ranges, then the rest; skip loopback, link-local, unspecified and duplicates. Virtual, VPN
/// and tunnel adapters are dropped unless nothing else is left.
pub fn rank_adapters(candidates: &[AdapterAddr], primary: Option<Ipv4Addr>) -> Vec<Ipv4Addr> {
    let usable: Vec<&AdapterAddr> = candidates
        .iter()
        .filter(|c| {
            let ip = c.ip;
            !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() && !ip.is_broadcast()
        })
        .collect();
    let real: Vec<&AdapterAddr> = usable
        .iter()
        .copied()
        .filter(|c| !is_virtual_adapter(c))
        .collect();
    let chosen = if real.is_empty() { usable } else { real };
    let mut out: Vec<Ipv4Addr> = Vec::new();
    if let Some(p) = primary
        && chosen.iter().any(|c| c.ip == p)
    {
        out.push(p);
    }
    for pass_private in [true, false] {
        for c in &chosen {
            if is_private(c.ip) == pass_private && !out.contains(&c.ip) {
                out.push(c.ip);
            }
        }
    }
    out
}

/// [`rank_adapters`] for candidates known only by interface name.
pub fn rank_addresses(
    candidates: &[(String, Ipv4Addr)],
    primary: Option<Ipv4Addr>,
) -> Vec<Ipv4Addr> {
    let list: Vec<AdapterAddr> = candidates
        .iter()
        .map(|(name, ip)| AdapterAddr {
            name: name.clone(),
            description: String::new(),
            if_type: 0,
            ip: *ip,
        })
        .collect();
    rank_adapters(&list, primary)
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

/// [`rank_adapters`], then USB-tethering addresses (`usb`) moved to the front: a phone
/// plugged in by cable reaches the PC that way even without Wi-Fi (g4 USB pairing).
pub fn rank_adapters_usb_first(
    candidates: &[AdapterAddr],
    primary: Option<Ipv4Addr>,
    usb: &[Ipv4Addr],
) -> Vec<Ipv4Addr> {
    let ranked = rank_adapters(candidates, primary);
    let (mut first, rest): (Vec<Ipv4Addr>, Vec<Ipv4Addr>) =
        ranked.into_iter().partition(|ip| usb.contains(ip));
    first.extend(rest);
    first
}

/// [`rank_addresses`], then USB-tethering addresses moved to the front.
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

/// Non-loopback IPv4 addresses of interfaces that are up: USB tethering first, then the
/// default-route Wi-Fi / Ethernet address, then other real LAN adapters. Virtual adapters
/// (VirtualBox, VMware, Hyper-V, WSL, VPNs) are left out.
pub fn lan_ipv4_addresses() -> Vec<Ipv4Addr> {
    let interfaces = match if_addrs::get_if_addrs() {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!("could not list network interfaces: {e}");
            return Vec::new();
        }
    };
    let adapters: Vec<helm_win_system::net::Adapter> =
        helm_win_system::net::mac_addresses().unwrap_or_default();
    let candidates: Vec<AdapterAddr> = interfaces
        .into_iter()
        .filter(|i| i.is_oper_up() && !i.is_loopback())
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) => {
                let info = adapters.iter().find(|a| a.adapter_name == i.name);
                Some(AdapterAddr {
                    description: info.map(|a| a.description.clone()).unwrap_or_default(),
                    if_type: info.map(|a| a.if_type).unwrap_or(0),
                    name: i.name,
                    ip: v4.ip,
                })
            }
            _ => None,
        })
        .collect();
    rank_adapters_usb_first(&candidates, default_route_address(), &usb_tether_addresses())
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

    fn adapter(name: &str, desc: &str, if_type: u32, ip: [u8; 4]) -> AdapterAddr {
        AdapterAddr {
            name: name.to_string(),
            description: desc.to_string(),
            if_type,
            ip: Ipv4Addr::from(ip),
        }
    }

    #[test]
    fn virtual_adapters_are_detected_by_description_and_type() {
        let virt = [
            adapter("Ethernet 2", "VirtualBox Host-Only Ethernet Adapter", 6, [192, 168, 56, 1]),
            adapter("Ethernet 3", "VMware Virtual Ethernet Adapter for VMnet8", 6, [192, 168, 99, 1]),
            adapter("vEthernet (Default Switch)", "Hyper-V Virtual Ethernet Adapter", 6, [172, 24, 0, 1]),
            adapter("vEthernet (WSL)", "Hyper-V Virtual Ethernet Adapter #2", 6, [172, 30, 0, 1]),
            adapter("Radmin VPN", "Famatech Radmin VPN Ethernet Adapter", 6, [26, 121, 94, 40]),
            adapter("Ethernet 4", "Hamachi Network Interface", 6, [25, 1, 2, 3]),
            adapter("Ethernet 5", "ZeroTier Virtual Port", 6, [10, 147, 1, 1]),
            adapter("Tailscale", "Tailscale Tunnel", 131, [100, 64, 0, 1]),
            adapter("wg0", "WireGuard Tunnel", 131, [10, 66, 0, 2]),
            adapter("Local Area Connection", "TAP-Windows Adapter V9", 6, [10, 8, 0, 6]),
            adapter("Ethernet 6", "Wintun Userspace Tunnel", 6, [10, 9, 0, 1]),
            adapter("Ethernet 7", "Something", 131, [10, 9, 0, 2]),
        ];
        for a in &virt {
            assert!(is_virtual_adapter(a), "{a:?}");
        }
        let real = [
            adapter("Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz", 71, [192, 168, 31, 56]),
            adapter("Ethernet", "Realtek PCIe GbE Family Controller", 6, [192, 168, 1, 5]),
            adapter("Ethernet", "Intel(R) Ethernet Connection (Fortune) I219-V", 6, [10, 0, 0, 5]),
        ];
        for a in &real {
            assert!(!is_virtual_adapter(a), "{a:?}");
        }
    }

    #[test]
    fn same_wifi_pc_with_virtualbox_and_radmin() {
        let c = vec![
            adapter("Ethernet 2", "VirtualBox Host-Only Ethernet Adapter", 6, [192, 168, 56, 1]),
            adapter("Radmin VPN", "Famatech Radmin VPN Ethernet Adapter", 6, [26, 121, 94, 40]),
            adapter("Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz", 71, [192, 168, 31, 56]),
        ];
        assert_eq!(
            rank_adapters(&c, Some(Ipv4Addr::new(192, 168, 31, 56))),
            vec![Ipv4Addr::new(192, 168, 31, 56)]
        );
        // Default route unknown: the real adapter is still the only one left.
        assert_eq!(rank_adapters(&c, None), vec![Ipv4Addr::new(192, 168, 31, 56)]);
    }

    #[test]
    fn default_route_first_among_real_adapters() {
        let c = vec![
            adapter("Ethernet", "Realtek PCIe GbE Family Controller", 6, [192, 168, 1, 5]),
            adapter("Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz", 71, [192, 168, 31, 56]),
        ];
        assert_eq!(
            rank_adapters(&c, Some(Ipv4Addr::new(192, 168, 31, 56))),
            vec![Ipv4Addr::new(192, 168, 31, 56), Ipv4Addr::new(192, 168, 1, 5)]
        );
    }

    #[test]
    fn virtualbox_range_alone_is_kept_when_nothing_else() {
        let c = vec![adapter("Ethernet 2", "VirtualBox Host-Only Ethernet Adapter", 6, [192, 168, 56, 1])];
        assert_eq!(rank_adapters(&c, None), vec![Ipv4Addr::new(192, 168, 56, 1)]);
    }

    #[test]
    fn keeps_virtual_if_nothing_else() {
        let c = vec![("vEthernet".to_string(), Ipv4Addr::new(172, 20, 0, 1))];
        assert_eq!(rank_addresses(&c, None), vec![Ipv4Addr::new(172, 20, 0, 1)]);
    }
}
