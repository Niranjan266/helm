//! Protecting the private key at rest.
//!
//! On Windows the key is encrypted with DPAPI for the current user, so only this Windows
//! account on this machine can read it. On other platforms (development only) it is stored
//! as-is.

/// Name of the protection scheme written into identity.json.
#[cfg(windows)]
pub const SCHEME: &str = "dpapi";
#[cfg(not(windows))]
pub const SCHEME: &str = "none";

#[cfg(windows)]
pub fn protect(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };
    use windows::core::w;

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plain.len())?,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points at `plain`, which outlives the call; `output` is filled by the
    // OS and freed below with LocalFree as the documentation requires.
    unsafe {
        CryptProtectData(
            &input,
            w!("Helm agent key"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        Ok(take_blob(output))
    }
}

/// Name of the machine-scope scheme (see [`protect_machine`]).
pub const MACHINE_SCHEME: &str = "dpapi-machine";

/// Encrypt for *this computer* rather than this user (`CRYPTPROTECT_LOCAL_MACHINE`), so the
/// sign-in service (LocalSystem, before anyone has signed in) can decrypt it. Any process on
/// this PC that can read the file can decrypt it, so callers must only write the result
/// where file permissions already restrict it to the user, SYSTEM and administrators.
#[cfg(windows)]
pub fn protect_machine(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
        CryptProtectData,
    };
    use windows::core::w;

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plain.len())?,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        CryptProtectData(
            &input,
            w!("Helm agent key (sign-in service)"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN | CRYPTPROTECT_LOCAL_MACHINE,
            &mut output,
        )?;
        Ok(take_blob(output))
    }
}

#[cfg(not(windows))]
pub fn protect_machine(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(plain.to_vec())
}

#[cfg(windows)]
pub fn unprotect(cipher: &[u8]) -> anyhow::Result<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(cipher.len())?,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )?;
        Ok(take_blob(output))
    }
}

/// Copy an OS-allocated blob into a Vec and free the OS memory.
#[cfg(windows)]
unsafe fn take_blob(blob: windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Vec<u8> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    if blob.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: the OS returned `cbData` valid bytes at `pbData`.
    let data = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec() };
    // SAFETY: the buffer was allocated by DPAPI with LocalAlloc.
    unsafe {
        LocalFree(Some(HLOCAL(blob.pbData as *mut core::ffi::c_void)));
    }
    data
}

#[cfg(not(windows))]
pub fn protect(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(plain.to_vec())
}

#[cfg(not(windows))]
pub fn unprotect(cipher: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(cipher.to_vec())
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let secret = b"not really a key";
        let sealed = super::protect(secret).unwrap();
        #[cfg(windows)]
        assert_ne!(sealed.as_slice(), secret.as_slice());
        assert_eq!(super::unprotect(&sealed).unwrap(), secret);
    }

    #[test]
    fn machine_round_trip() {
        let secret = b"not really a key either";
        let sealed = super::protect_machine(secret).unwrap();
        #[cfg(windows)]
        assert_ne!(sealed.as_slice(), secret.as_slice());
        assert_eq!(super::unprotect(&sealed).unwrap(), secret);
    }
}
