//! Network adapters (for Wake-on-LAN) and the WoL magic packet.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// A network adapter with a 6-byte hardware address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adapter {
    /// Friendly name, e.g. `Wi-Fi` or `Ethernet`.
    pub adapter_name: String,
    /// Driver description, e.g. `Intel(R) Wi-Fi 6 AX201 160MHz`.
    pub description: String,
    /// MAC as `AA:BB:CC:DD:EE:FF`.
    pub mac: String,
    /// IPv4 addresses assigned to the adapter.
    pub ipv4: Vec<String>,
    /// Operational status is "up".
    pub is_up: bool,
    /// IANA ifType (6 = Ethernet, 71 = Wi-Fi, ...).
    pub if_type: u32,
    /// The MAC has the "locally administered" bit set: a random / virtual
    /// address (Wi-Fi MAC randomisation, VPN or VM adapters). Wake-on-LAN
    /// normally needs a wired adapter's burned-in (not locally administered) MAC.
    pub locally_administered: bool,
}

/// Adapters with a 6-byte MAC (loopback and tunnels skipped), up ones first.
pub fn mac_addresses() -> Result<Vec<Adapter>> {
    let mut v = imp::adapters()?;
    v.sort_by_key(|a| (!a.is_up, a.ipv4.is_empty()));
    Ok(v)
}

/// Format a MAC as `AA:BB:CC:DD:EE:FF`.
pub fn format_mac(mac: &[u8; 6]) -> String {
    mac.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Parse `AA:BB:CC:DD:EE:FF`, `aa-bb-cc-dd-ee-ff`, `aabb.ccdd.eeff` or `aabbccddeeff`.
pub fn parse_mac(s: &str) -> Result<[u8; 6]> {
    let hex: String = s.chars().filter(|c| !matches!(c, ':' | '-' | '.' | ' ')).collect();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::InvalidArgument(format!("bad MAC address {s:?}")));
    }
    let mut out = [0u8; 6];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| Error::InvalidArgument(format!("bad MAC address {s:?}")))?;
    }
    Ok(out)
}

/// Wake-on-LAN magic packet: 6 x `0xFF` then the MAC repeated 16 times
/// (send it as a UDP broadcast, usually to port 9).
pub fn wol_magic_packet(mac: &[u8; 6]) -> [u8; 102] {
    let mut p = [0xFFu8; 102];
    for chunk in p[6..].chunks_exact_mut(6) {
        chunk.copy_from_slice(mac);
    }
    p
}

#[cfg(windows)]
mod imp {
    use std::net::Ipv4Addr;

    use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::Networking::WinSock::{AF_INET, AF_UNSPEC, SOCKADDR_IN};

    use super::{Adapter, format_mac};
    use crate::{Error, Result};

    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
    const IF_TYPE_TUNNEL: u32 = 131;

    pub(super) fn adapters() -> Result<Vec<Adapter>> {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size: u32 = 16 * 1024;
        // u64 storage keeps the buffer 8-byte aligned for IP_ADAPTER_ADDRESSES_LH.
        let mut buf: Vec<u64>;
        let mut tries = 0;
        loop {
            buf = vec![0u64; (size as usize).div_ceil(8)];
            let rc = unsafe {
                GetAdaptersAddresses(
                    AF_UNSPEC.0 as u32,
                    flags,
                    None,
                    Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH),
                    &mut size,
                )
            };
            if rc == ERROR_SUCCESS.0 {
                break;
            }
            tries += 1;
            if rc != ERROR_BUFFER_OVERFLOW.0 || tries > 4 {
                return Err(Error::Os { code: rc as i32, message: "GetAdaptersAddresses failed".into() });
            }
        }

        let mut out = Vec::new();
        let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while let Some(a) = unsafe { cur.as_ref() } {
            cur = a.Next;
            if a.PhysicalAddressLength != 6 || a.IfType == IF_TYPE_SOFTWARE_LOOPBACK || a.IfType == IF_TYPE_TUNNEL {
                continue;
            }
            let mut mac = [0u8; 6];
            mac.copy_from_slice(&a.PhysicalAddress[..6]);
            if mac == [0; 6] {
                continue;
            }
            let mut ipv4 = Vec::new();
            let mut u = a.FirstUnicastAddress;
            while let Some(ua) = unsafe { u.as_ref() } {
                u = ua.Next;
                let sa = ua.Address.lpSockaddr;
                if sa.is_null() {
                    continue;
                }
                unsafe {
                    if (*sa).sa_family == AF_INET {
                        let sin = &*(sa as *const SOCKADDR_IN);
                        let b = sin.sin_addr.S_un.S_un_b;
                        ipv4.push(Ipv4Addr::new(b.s_b1, b.s_b2, b.s_b3, b.s_b4).to_string());
                    }
                }
            }
            out.push(Adapter {
                adapter_name: unsafe { a.FriendlyName.to_string() }.unwrap_or_default(),
                description: unsafe { a.Description.to_string() }.unwrap_or_default(),
                mac: format_mac(&mac),
                ipv4,
                is_up: a.OperStatus == IfOperStatusUp,
                if_type: a.IfType,
                locally_administered: mac[0] & 0x02 != 0,
            });
        }
        Ok(out)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Adapter;
    use crate::{Error, Result};

    pub(super) fn adapters() -> Result<Vec<Adapter>> {
        Err(Error::Unsupported("adapter list requires Windows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_packet_layout() {
        let mac = [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB];
        let p = wol_magic_packet(&mac);
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xFF; 6]);
        for i in 0..16 {
            assert_eq!(&p[6 + i * 6..12 + i * 6], &mac, "repetition {i}");
        }
    }

    #[test]
    fn mac_parse_and_format() {
        let want = [0xAA, 0xBB, 0xCC, 0x01, 0x02, 0x03];
        for s in ["AA:BB:CC:01:02:03", "aa-bb-cc-01-02-03", "aabb.cc01.0203", "aabbcc010203"] {
            assert_eq!(parse_mac(s).unwrap(), want, "{s}");
        }
        assert_eq!(format_mac(&want), "AA:BB:CC:01:02:03");
        for bad in ["", "AA:BB:CC:01:02", "AA:BB:CC:01:02:03:04", "GG:BB:CC:01:02:03", "é1:BB:CC:01:02:03"] {
            assert!(parse_mac(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn serde_shape() {
        let a = Adapter {
            adapter_name: "Wi-Fi".into(),
            description: "x".into(),
            mac: "AA:BB:CC:01:02:03".into(),
            ipv4: vec!["192.168.1.5".into()],
            is_up: true,
            if_type: 71,
            locally_administered: false,
        };
        let v = serde_json::to_value(a).unwrap();
        assert_eq!(v["adapter_name"], "Wi-Fi");
        assert_eq!(v["ipv4"][0], "192.168.1.5");
        assert_eq!(v["is_up"], true);
    }
}
