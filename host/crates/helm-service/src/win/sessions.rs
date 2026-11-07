//! Who is signed in: the SIDs of users with a Windows session, and our own SID.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_SESSION_INFOW, WTSEnumerateSessionsW, WTSFreeMemory,
    WTSQueryUserToken,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PWSTR;

/// String SID of the user a token belongs to.
fn token_sid(token: HANDLE) -> Option<String> {
    let mut len = 0u32;
    // SAFETY: size query into a local.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
    if len == 0 {
        return None;
    }
    // u64 backing keeps TOKEN_USER suitably aligned.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: buf holds at least `len` bytes.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
        .ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        out
    }
}

/// SID of the user running this process (the desktop app, before it asks for elevation).
pub fn current_user_sid() -> Result<String, String> {
    let mut token = HANDLE::default();
    // SAFETY: pseudo-handle for this process; out-pointer to a local.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
        .map_err(|e| format!("OpenProcessToken: {e}"))?;
    let sid = token_sid(token);
    // SAFETY: we own the token handle.
    unsafe {
        let _ = CloseHandle(token);
    }
    sid.ok_or_else(|| "could not read this user's SID".into())
}

/// SIDs of every user with a session on this PC (active, locked or disconnected).
/// `None` if the sessions could not be listed. Needs LocalSystem (WTSQueryUserToken).
pub fn signed_in_sids() -> Option<Vec<String>> {
    let mut info: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: out-pointers to locals; freed with WTSFreeMemory below.
    unsafe { WTSEnumerateSessionsW(Some(WTS_CURRENT_SERVER_HANDLE), 0, 1, &mut info, &mut count) }
        .ok()?;
    let mut sids = Vec::new();
    // SAFETY: the OS returned `count` entries at `info`.
    let sessions = unsafe { std::slice::from_raw_parts(info, count as usize) };
    for s in sessions {
        if s.SessionId == 0 {
            continue; // services
        }
        let mut token = HANDLE::default();
        // SAFETY: out-pointer to a local. Fails (ERROR_NO_TOKEN) for the logon screen.
        if unsafe { WTSQueryUserToken(s.SessionId, &mut token) }.is_ok() {
            if let Some(sid) = token_sid(token) {
                sids.push(sid);
            }
            // SAFETY: we own the token handle.
            unsafe {
                let _ = CloseHandle(token);
            }
        }
    }
    // SAFETY: memory from WTSEnumerateSessionsW.
    unsafe { WTSFreeMemory(info.cast()) };
    Some(sids)
}

/// Whether `owner_sid` has a session (`None` = could not tell).
pub fn owner_signed_in(owner_sid: &str) -> Option<bool> {
    signed_in_sids().map(|v| v.iter().any(|s| s.eq_ignore_ascii_case(owner_sid)))
}
