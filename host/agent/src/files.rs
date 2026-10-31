//! File transfers (PROTOCOL §9.2, §9.3): uploads into `Downloads\Helm`, downloads of files
//! under the user's profile folder, binary frame layout, safe names and SHA-256 checks.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::router::RpcError;
use crate::session::Session;

/// Largest file that can be sent either way (4 GiB).
pub const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024 * 1024;
/// Largest data part of one binary frame.
pub const MAX_CHUNK: usize = 256 * 1024;
/// `[16 id][8 offset BE]`.
pub const HEADER_LEN: usize = 24;
/// Uploads one session may have open at once.
pub const MAX_OPEN_UPLOADS: usize = 8;
/// Binary frames of one download queued for the socket at once.
const DOWNLOAD_WINDOW: usize = 8;

pub type TransferId = [u8; 16];

pub fn new_transfer_id() -> TransferId {
    let mut id = [0u8; 16];
    id.copy_from_slice(&crate::util::random_bytes(16));
    id
}

pub fn parse_transfer_id(hex_id: &str) -> Option<TransferId> {
    let bytes = hex::decode(hex_id.trim()).ok()?;
    bytes.try_into().ok()
}

/// Build one binary frame.
pub fn encode_frame(id: &TransferId, offset: u64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + data.len());
    out.extend_from_slice(id);
    out.extend_from_slice(&offset.to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// Split one binary frame into (id, offset, data).
pub fn decode_frame(frame: &[u8]) -> Result<(TransferId, u64, &[u8]), String> {
    if frame.len() < HEADER_LEN {
        return Err("binary frame shorter than its 24-byte header".into());
    }
    let data = &frame[HEADER_LEN..];
    if data.len() > MAX_CHUNK {
        return Err(format!("binary frame data is over {MAX_CHUNK} bytes"));
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(&frame[..16]);
    let mut off = [0u8; 8];
    off.copy_from_slice(&frame[16..24]);
    Ok((id, u64::from_be_bytes(off), data))
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Turn a name from the phone into a safe single file name, or refuse it.
///
/// Refused: empty, path separators, `.`/`..`, control characters, over 200 characters.
/// Replaced with `_`: `< > : " | ? *` (`:` would otherwise open an NTFS stream).
/// Windows device names (`CON`, `NUL.txt`, …) get a leading `_`.
pub fn safe_file_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("file name is empty".into());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("file name must not contain a path".into());
    }
    if name == "." || name == ".." {
        return Err("file name is not allowed".into());
    }
    if name.chars().any(char::is_control) {
        return Err("file name contains control characters".into());
    }
    if name.chars().count() > 200 {
        return Err("file name is longer than 200 characters".into());
    }
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    // Windows drops trailing dots and spaces, which could turn "a.." into a different name.
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        return Err("file name is not allowed".into());
    }
    let stem = out.split('.').next().unwrap_or("").trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        out.insert(0, '_');
    }
    Ok(out)
}

/// `name`, `name (1).ext`, `name (2).ext`, …
fn numbered(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

/// Create an empty file with a name that is free in `dir` (atomic: `create_new`).
fn reserve_name(dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    for n in 0..10_000 {
        let path = dir.join(numbered(name, n));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free file name"))
}

/// An upload in progress.
pub struct Upload {
    pub name: String,
    pub size: u64,
    pub sha256: [u8; 32],
    temp: PathBuf,
    file: Option<tokio::fs::File>,
    written: u64,
    hasher: Sha256,
    failed: Option<String>,
}

impl Upload {
    /// Start an upload: `dir` is created if needed and a hidden temp file is opened in it.
    pub async fn begin(
        dir: &Path,
        id: &TransferId,
        name: String,
        size: u64,
        sha256: [u8; 32],
    ) -> std::io::Result<Upload> {
        tokio::fs::create_dir_all(dir).await?;
        let temp = dir.join(format!(".helm-{}.part", hex::encode(id)));
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .await?;
        Ok(Upload {
            name,
            size,
            sha256,
            temp,
            file: Some(file),
            written: 0,
            hasher: Sha256::new(),
            failed: None,
        })
    }

    fn fail(&mut self, why: String) {
        if self.failed.is_none() {
            tracing::debug!("upload of {} failed: {why}", self.name);
            self.failed = Some(why);
            self.file = None;
        }
    }

    /// Append one frame's data. Frames must arrive in order (offset = bytes so far).
    pub async fn write(&mut self, offset: u64, data: &[u8]) {
        if self.failed.is_some() {
            return;
        }
        if offset != self.written {
            return self.fail(format!(
                "data at offset {offset}, expected offset {}",
                self.written
            ));
        }
        if self.written + data.len() as u64 > self.size {
            return self.fail("more data than the announced size".into());
        }
        let Some(file) = self.file.as_mut() else {
            return self.fail("upload file is closed".into());
        };
        if let Err(e) = file.write_all(data).await {
            return self.fail(format!("write failed: {e}"));
        }
        self.hasher.update(data);
        self.written += data.len() as u64;
    }

    /// Check size and hash, then move the file to its final name in `dir`.
    pub async fn finish(mut self, dir: &Path) -> Result<PathBuf, String> {
        let result = self.finish_inner(dir).await;
        if result.is_err() {
            self.file = None;
            let _ = tokio::fs::remove_file(&self.temp).await;
        }
        result
    }

    async fn finish_inner(&mut self, dir: &Path) -> Result<PathBuf, String> {
        if let Some(why) = self.failed.take() {
            return Err(why);
        }
        if self.written != self.size {
            return Err(format!("received {} of {} bytes", self.written, self.size));
        }
        let mut file = self.file.take().ok_or("upload file is closed")?;
        file.flush().await.map_err(|e| e.to_string())?;
        file.sync_all().await.map_err(|e| e.to_string())?;
        drop(file);
        let digest: [u8; 32] = std::mem::take(&mut self.hasher).finalize().into();
        if digest != self.sha256 {
            return Err("sha256 does not match the received data".into());
        }
        let dir = dir.to_path_buf();
        let name = self.name.clone();
        let temp = self.temp.clone();
        tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
            let target = reserve_name(&dir, &name).map_err(|e| e.to_string())?;
            std::fs::rename(&temp, &target).map_err(|e| {
                let _ = std::fs::remove_file(&target);
                e.to_string()
            })?;
            Ok(target)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// Throw the partial file away (session closed).
    pub fn discard(mut self) {
        self.file = None;
        let _ = std::fs::remove_file(&self.temp);
    }
}

fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// Check a `file.download` path: an existing regular file under `root` (the user's profile),
/// not inside `data_dir` (keys and paired devices), at most [`MAX_FILE_SIZE`].
/// Returns the canonical path, its size and its file name.
pub fn check_download(
    path: &str,
    root: &Path,
    data_dir: &Path,
) -> Result<(PathBuf, u64, String), RpcError> {
    let raw = Path::new(path.trim());
    if path.trim().is_empty() || !raw.is_absolute() {
        return Err(RpcError::bad_params("path must be an absolute path"));
    }
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(RpcError::bad_params("path must not contain .."));
    }
    let canonical =
        std::fs::canonicalize(raw).map_err(|_| RpcError::bad_params("file not found"))?;
    let root = std::fs::canonicalize(root)
        .map_err(|e| RpcError::internal(format!("profile folder unavailable: {e}")))?;
    if !is_within(&canonical, &root) {
        return Err(RpcError::forbidden(
            "only files inside the user's profile folder can be downloaded",
        ));
    }
    if let Ok(data) = std::fs::canonicalize(data_dir)
        && is_within(&canonical, &data)
    {
        return Err(RpcError::forbidden(
            "the Helm data folder cannot be downloaded",
        ));
    }
    let meta = std::fs::metadata(&canonical).map_err(|_| RpcError::bad_params("file not found"))?;
    if !meta.is_file() {
        return Err(RpcError::bad_params("path is not a file"));
    }
    if meta.len() > MAX_FILE_SIZE {
        return Err(RpcError::bad_params("file is larger than 4 GiB"));
    }
    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    Ok((canonical, meta.len(), name))
}

/// SHA-256 of a whole file (blocking).
pub fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// Send the file as binary frames, then `file.done{transfer_id}` to this session.
pub async fn stream_download(session: Arc<Session>, id: TransferId, path: PathBuf, size: u64) {
    let id_hex = hex::encode(id);
    let result = async {
        let mut file = tokio::fs::File::open(&path).await?;
        let window = Arc::new(tokio::sync::Semaphore::new(DOWNLOAD_WINDOW));
        let mut offset = 0u64;
        let mut buf = vec![0u8; MAX_CHUNK];
        while offset < size {
            let want = ((size - offset) as usize).min(MAX_CHUNK);
            file.read_exact(&mut buf[..want]).await?;
            let permit = window
                .clone()
                .acquire_owned()
                .await
                .map_err(std::io::Error::other)?;
            if !session.send_binary(encode_frame(&id, offset, &buf[..want]), permit) {
                return Err(std::io::Error::other("session closed"));
            }
            offset += want as u64;
        }
        // Wait until every frame has gone out before announcing the end.
        let _ = window.acquire_many(DOWNLOAD_WINDOW as u32).await;
        Ok::<(), std::io::Error>(())
    }
    .await;
    match result {
        Ok(()) => {
            session.send_event("file.done", serde_json::json!({"transfer_id": id_hex}));
        }
        Err(e) => {
            tracing::warn!("download of {} failed: {e}", path.display());
            session.send_event(
                "file.done",
                serde_json::json!({"transfer_id": id_hex, "error": e.to_string()}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_file_name("photo.jpg").unwrap(), "photo.jpg");
        assert_eq!(safe_file_name(" a:b?.txt ").unwrap(), "a_b_.txt");
        assert_eq!(safe_file_name("CON").unwrap(), "_CON");
        assert_eq!(safe_file_name("nul.txt").unwrap(), "_nul.txt");
        assert_eq!(safe_file_name("x...").unwrap(), "x");
        for bad in [
            "", "..", ".", "../a", "a/b", "a\\b", "C:\\x", "a\u{0}b", "...", " ",
        ] {
            assert!(safe_file_name(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn frames_round_trip() {
        let id = [7u8; 16];
        let f = encode_frame(&id, 0x0102_0304_0506_0708, b"abc");
        assert_eq!(&f[16..24], &[1, 2, 3, 4, 5, 6, 7, 8]);
        let (i, o, d) = decode_frame(&f).unwrap();
        assert_eq!((i, o, d), (id, 0x0102_0304_0506_0708, &b"abc"[..]));
        assert!(decode_frame(&f[..20]).is_err());
        assert!(decode_frame(&vec![0u8; HEADER_LEN + MAX_CHUNK + 1]).is_err());
    }

    #[test]
    fn numbered_names() {
        assert_eq!(numbered("a.txt", 0), "a.txt");
        assert_eq!(numbered("a.txt", 2), "a (2).txt");
        assert_eq!(numbered(".bashrc", 1), ".bashrc (1)");
        assert_eq!(numbered("noext", 1), "noext (1)");
    }

    #[test]
    fn download_paths_are_checked() {
        let profile = tempfile::tempdir().unwrap();
        let data = profile.path().join("AppData").join("Helm");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("identity.json"), b"secret").unwrap();
        std::fs::write(profile.path().join("ok.txt"), b"hi").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x.txt"), b"no").unwrap();

        let ok = profile.path().join("ok.txt");
        let (_, size, name) = check_download(ok.to_str().unwrap(), profile.path(), &data).unwrap();
        assert_eq!((size, name.as_str()), (2, "ok.txt"));

        let x = outside.path().join("x.txt");
        let e = check_download(x.to_str().unwrap(), profile.path(), &data).unwrap_err();
        assert_eq!(e.code, "forbidden");
        let id = data.join("identity.json");
        let e = check_download(id.to_str().unwrap(), profile.path(), &data).unwrap_err();
        assert_eq!(e.code, "forbidden");
        let trav = format!("{}\\..\\x.txt", profile.path().display());
        assert_eq!(
            check_download(&trav, profile.path(), &data)
                .unwrap_err()
                .code,
            "bad_params"
        );
        assert_eq!(
            check_download("relative.txt", profile.path(), &data)
                .unwrap_err()
                .code,
            "bad_params"
        );
        let dir = profile.path().to_str().unwrap().to_string();
        assert_eq!(
            check_download(&dir, profile.path(), &data)
                .unwrap_err()
                .code,
            "bad_params"
        );
    }
}
