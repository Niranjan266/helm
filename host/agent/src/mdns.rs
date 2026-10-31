//! mDNS / DNS-SD advertisement of `_helm._tcp.local.` (PROTOCOL.md §3).

use std::net::{IpAddr, Ipv4Addr};

use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_helm._tcp.local.";

pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

/// A DNS label made from the computer name (letters, digits, '-').
fn dns_label(name: &str) -> String {
    let label: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(63)
        .collect();
    let label = label.trim_matches('-').to_string();
    if label.is_empty() {
        "helm-pc".to_string()
    } else {
        label
    }
}

impl Advertiser {
    pub fn start(
        host_id: &str,
        host_name: &str,
        addresses: &[Ipv4Addr],
        port: u16,
    ) -> anyhow::Result<Advertiser> {
        let daemon = ServiceDaemon::new()?;
        let ips: Vec<IpAddr> = addresses.iter().map(|a| IpAddr::V4(*a)).collect();
        let mdns_host = format!("{}.local.", dns_label(host_name));
        let properties = [("v", "1"), ("id", host_id), ("name", host_name)];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            host_name,
            &mdns_host,
            &ips[..],
            port,
            &properties[..],
        )?;
        let fullname = info.get_fullname().to_string();
        daemon.register(info)?;
        tracing::info!("mDNS: advertising {fullname} on port {port}");
        Ok(Advertiser { daemon, fullname })
    }

    pub fn stop(self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
        }
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn labels() {
        assert_eq!(super::dns_label("DESKTOP-AB12"), "DESKTOP-AB12");
        assert_eq!(super::dns_label("My PC!"), "My-PC");
        assert_eq!(super::dns_label("!!"), "helm-pc");
    }
}
