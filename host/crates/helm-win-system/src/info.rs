//! OS version, uptime and battery.

use serde::{Deserialize, Serialize};

use crate::Result;

/// Operating system identification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsInfo {
    /// e.g. `Windows 11 Home Single Language`.
    pub product_name: String,
    /// e.g. `25H2` (empty on old builds).
    pub display_version: String,
    /// Build number, e.g. `26200`.
    pub build: u32,
    /// Update build revision, e.g. `6899`.
    pub ubr: u32,
    /// e.g. `10.0.26200.6899`.
    pub version: String,
}

/// Battery status. Only produced when the machine has a system battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    /// Charge 0..=100, `None` if unknown.
    pub percent: Option<u8>,
    /// Currently charging.
    pub charging: bool,
    /// On AC power, `None` if unknown.
    pub ac_online: Option<bool>,
    /// Estimated seconds left on battery, `None` if unknown or on AC.
    pub seconds_remaining: Option<u32>,
    /// Battery saver is on.
    pub saver_on: bool,
}

/// OS product name, display version and build, read from
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
///
/// The registry still says "Windows 10" on Windows 11; the name is corrected
/// for builds >= 22000.
pub fn os_info() -> Result<OsInfo> {
    imp::os_info()
}

/// Seconds since boot (`GetTickCount64`; includes time asleep).
pub fn uptime_s() -> Result<u64> {
    imp::uptime_s()
}

/// Battery status, or `None` when the PC has no battery (desktop).
pub fn battery() -> Result<Option<Battery>> {
    imp::battery()
}

/// Fix the registry's "Windows 10" product name on Windows 11 builds.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn fix_product_name(name: &str, build: u32) -> String {
    if build >= 22000 && name.starts_with("Windows 10") {
        format!("Windows 11{}", &name["Windows 10".len()..])
    } else {
        name.to_string()
    }
}

/// Decode `SYSTEM_POWER_STATUS` fields (pure so it can be unit-tested).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn decode_battery(ac_line: u8, flag: u8, percent: u8, system_status: u8, life_time: u32) -> Option<Battery> {
    const NO_BATTERY: u8 = 128;
    const UNKNOWN: u8 = 255;
    const CHARGING: u8 = 8;
    if flag == UNKNOWN || flag & NO_BATTERY != 0 {
        return None;
    }
    let ac_online = match ac_line {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    Some(Battery {
        percent: (percent <= 100).then_some(percent),
        charging: flag & CHARGING != 0,
        ac_online,
        seconds_remaining: (life_time != u32::MAX && ac_online != Some(true)).then_some(life_time),
        saver_on: system_status == 1,
    })
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
    use windows::Win32::System::SystemInformation::GetTickCount64;
    use windows::core::PCWSTR;

    use super::{Battery, OsInfo, decode_battery, fix_product_name};
    use crate::worker::{to_wide, wide_to_string};
    use crate::{Error, Result};

    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

    fn reg_string(value: &str) -> Option<String> {
        let key = to_wide(KEY);
        let val = to_wide(value);
        let mut buf = vec![0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key.as_ptr()),
                PCWSTR(val.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr() as *mut _),
                Some(&mut len),
            )
        };
        (rc == ERROR_SUCCESS).then(|| wide_to_string(&buf))
    }

    fn reg_dword(value: &str) -> Option<u32> {
        let key = to_wide(KEY);
        let val = to_wide(value);
        let mut v = 0u32;
        let mut len = 4u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key.as_ptr()),
                PCWSTR(val.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut v as *mut u32 as *mut _),
                Some(&mut len),
            )
        };
        (rc == ERROR_SUCCESS).then_some(v)
    }

    pub(super) fn os_info() -> Result<OsInfo> {
        let raw_name =
            reg_string("ProductName").ok_or_else(|| Error::NotFound("ProductName registry value".into()))?;
        let build = reg_string("CurrentBuildNumber")
            .or_else(|| reg_string("CurrentBuild"))
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let ubr = reg_dword("UBR").unwrap_or(0);
        let major = reg_dword("CurrentMajorVersionNumber").unwrap_or(10);
        let minor = reg_dword("CurrentMinorVersionNumber").unwrap_or(0);
        Ok(OsInfo {
            product_name: fix_product_name(&raw_name, build),
            display_version: reg_string("DisplayVersion").unwrap_or_default(),
            build,
            ubr,
            version: format!("{major}.{minor}.{build}.{ubr}"),
        })
    }

    pub(super) fn uptime_s() -> Result<u64> {
        Ok(unsafe { GetTickCount64() } / 1000)
    }

    pub(super) fn battery() -> Result<Option<Battery>> {
        let mut s = SYSTEM_POWER_STATUS::default();
        unsafe { GetSystemPowerStatus(&mut s)? };
        Ok(decode_battery(s.ACLineStatus, s.BatteryFlag, s.BatteryLifePercent, s.SystemStatusFlag, s.BatteryLifeTime))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Battery, OsInfo};
    use crate::{Error, Result};

    pub(super) fn os_info() -> Result<OsInfo> {
        Err(Error::Unsupported("os_info requires Windows"))
    }
    pub(super) fn uptime_s() -> Result<u64> {
        Err(Error::Unsupported("uptime requires Windows"))
    }
    pub(super) fn battery() -> Result<Option<Battery>> {
        Err(Error::Unsupported("battery requires Windows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_name_fix() {
        assert_eq!(fix_product_name("Windows 10 Pro", 26100), "Windows 11 Pro");
        assert_eq!(fix_product_name("Windows 10 Pro", 19045), "Windows 10 Pro");
        assert_eq!(fix_product_name("Windows 11 Home", 26200), "Windows 11 Home");
    }

    #[test]
    fn battery_decode() {
        // Desktop: no system battery.
        assert_eq!(decode_battery(1, 128, 255, 0, u32::MAX), None);
        // Unknown status.
        assert_eq!(decode_battery(1, 255, 255, 0, u32::MAX), None);
        // Laptop charging at 40 % on AC.
        let b = decode_battery(1, 8, 40, 0, u32::MAX).unwrap();
        assert_eq!(b, Battery { percent: Some(40), charging: true, ac_online: Some(true), seconds_remaining: None, saver_on: false });
        // On battery, 1 h left, saver on.
        let b = decode_battery(0, 1, 90, 1, 3600).unwrap();
        assert_eq!(b.seconds_remaining, Some(3600));
        assert_eq!(b.ac_online, Some(false));
        assert!(!b.charging && b.saver_on);
        // Percent unknown is not coerced to 0.
        assert_eq!(decode_battery(255, 0, 255, 0, u32::MAX).unwrap().percent, None);
    }
}
