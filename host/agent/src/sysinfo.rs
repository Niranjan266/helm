//! OS name and uptime for `system.info`.

/// Something like "Windows 11 Home 24H2 (build 26100)".
#[cfg(windows)]
pub fn os_string() -> String {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let product = reg_string(KEY, "ProductName").unwrap_or_else(|| "Windows".to_string());
    let display = reg_string(KEY, "DisplayVersion");
    let build = reg_string(KEY, "CurrentBuildNumber");
    // Windows 11 still reports "Windows 10 …" in ProductName; the build number tells them apart.
    let is_11 = build
        .as_deref()
        .and_then(|b| b.parse::<u32>().ok())
        .is_some_and(|b| b >= 22000);
    let product = if is_11 {
        product.replacen("Windows 10", "Windows 11", 1)
    } else {
        product
    };
    let mut out = product;
    if let Some(d) = display {
        out.push(' ');
        out.push_str(&d);
    }
    if let Some(b) = build {
        out.push_str(&format!(" (build {b})"));
    }
    out
}

#[cfg(windows)]
fn reg_string(subkey: &str, value: &str) -> Option<String> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::HSTRING;

    let subkey = HSTRING::from(subkey);
    let value = HSTRING::from(value);
    let mut buf = vec![0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: buffer and size describe valid writable memory.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &subkey,
            &value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let chars = (size as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..chars]);
    let s = s.trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(not(windows))]
pub fn os_string() -> String {
    format!("{} (not Windows)", std::env::consts::OS)
}

/// Seconds since the PC booted (Windows) or since the agent started (elsewhere).
pub fn uptime_s(agent_started: std::time::Instant) -> u64 {
    #[cfg(windows)]
    {
        let _ = agent_started;
        // SAFETY: plain function without arguments.
        unsafe { windows::Win32::System::SystemInformation::GetTickCount64() / 1000 }
    }
    #[cfg(not(windows))]
    {
        agent_started.elapsed().as_secs()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn os_string_is_not_empty() {
        let s = super::os_string();
        assert!(!s.is_empty());
        #[cfg(windows)]
        assert!(s.starts_with("Windows"), "{s}");
    }
}
