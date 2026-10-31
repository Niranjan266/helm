//! Phase 3 file browser: `fs.list`, `fs.delete`, `fs.mkdir`, `fs.open` (admin).
//!
//! Every path is canonicalized (symlinks / junctions resolved) and must lie inside a root:
//! the user's profile folder or a fixed drive. The Helm data folder (keys, paired devices)
//! is never listed, opened or changed. `fs.delete` removes files and *empty* folders only.

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::str_param;
use crate::platform::blocking;
use crate::router::{MethodSpec, Module, RequestContext, RpcError, Tier};

pub struct FsModule;

const fn m(name: &'static str, min_tier: Tier) -> MethodSpec {
    MethodSpec { name, min_tier }
}

const METHODS: &[MethodSpec] = &[
    m("fs.list", Tier::Admin),
    m("fs.delete", Tier::Admin),
    m("fs.mkdir", Tier::Admin),
    m("fs.open", Tier::Admin),
];

/// Most entries returned by one `fs.list`.
pub const MAX_ENTRIES: usize = 5000;

/// Turn `\\?\C:\x` into `C:\x` and `\\?\UNC\s\x` into `\\s\x` for display.
pub fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    }
}

/// What a path may touch.
pub struct Scope {
    pub roots: Vec<PathBuf>,
    /// Canonical Helm data folder (blocked), if it exists.
    pub blocked: Option<PathBuf>,
}

impl Scope {
    pub fn new(profile: &Path, drives: &[PathBuf], data_dir: &Path) -> Scope {
        let mut roots: Vec<PathBuf> = Vec::new();
        for r in std::iter::once(profile).chain(drives.iter().map(PathBuf::as_path)) {
            if let Ok(c) = std::fs::canonicalize(r)
                && !roots.contains(&c)
            {
                roots.push(c);
            }
        }
        Scope {
            roots,
            blocked: std::fs::canonicalize(data_dir).ok(),
        }
    }

    fn is_blocked(&self, p: &Path) -> bool {
        self.blocked.as_ref().is_some_and(|b| p.starts_with(b))
    }

    /// Canonicalize an existing path from the phone and check it is allowed.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, RpcError> {
        let canonical = std::fs::canonicalize(check_raw(path)?)
            .map_err(|_| RpcError::bad_params("path not found"))?;
        self.check(&canonical)?;
        Ok(canonical)
    }

    fn check(&self, canonical: &Path) -> Result<(), RpcError> {
        if !self.roots.iter().any(|r| canonical.starts_with(r)) {
            return Err(RpcError::forbidden(
                "only the user's profile folder and fixed drives are reachable",
            ));
        }
        if self.is_blocked(canonical) {
            return Err(RpcError::forbidden("the Helm data folder is off limits"));
        }
        Ok(())
    }

    fn is_root(&self, canonical: &Path) -> bool {
        self.roots.iter().any(|r| r == canonical)
    }
}

fn check_raw(path: &str) -> Result<&Path, RpcError> {
    let raw = Path::new(path.trim());
    if path.trim().is_empty() || !raw.is_absolute() {
        return Err(RpcError::bad_params("path must be an absolute path"));
    }
    if raw.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(RpcError::bad_params("path must not contain .."));
    }
    Ok(raw)
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `fs.list`: `""` lists the roots.
pub fn list(scope: &Scope, path: &str) -> Result<Value, RpcError> {
    if path.trim().is_empty() {
        let entries: Vec<Value> = scope
            .roots
            .iter()
            .map(|r| {
                let p = display_path(r);
                json!({"name": p, "path": p, "is_dir": true, "size": 0, "modified_ms": 0})
            })
            .collect();
        return Ok(json!({"entries": entries}));
    }
    let dir = scope.resolve(path)?;
    if !dir.is_dir() {
        return Err(RpcError::bad_params("path is not a folder"));
    }
    let read = std::fs::read_dir(&dir)
        .map_err(|e| RpcError::bad_params(format!("cannot list folder: {e}")))?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in read.flatten() {
        let full = entry.path();
        if scope.is_blocked(&full) {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(meta) = std::fs::metadata(&full) else {
            continue;
        };
        entries.push(json!({
            "name": entry.file_name().to_string_lossy(),
            "path": display_path(&full),
            "is_dir": meta.is_dir(),
            "size": if meta.is_dir() { 0 } else { meta.len() },
            "modified_ms": modified_ms(&meta),
        }));
    }
    let key = |v: &Value| {
        (
            !v["is_dir"].as_bool().unwrap_or(false),
            v["name"].as_str().unwrap_or("").to_lowercase(),
        )
    };
    entries.sort_by_key(key);
    Ok(json!({"entries": entries, "path": display_path(&dir), "truncated": truncated}))
}

/// `fs.delete`: a file, or an empty folder; never a root.
pub fn delete(scope: &Scope, path: &str) -> Result<PathBuf, RpcError> {
    let raw = check_raw(path)?;
    // Do not follow a final symlink/junction: delete the link itself, checked by its parent.
    let meta =
        std::fs::symlink_metadata(raw).map_err(|_| RpcError::bad_params("path not found"))?;
    let parent = raw
        .parent()
        .ok_or_else(|| RpcError::forbidden("a drive root cannot be deleted"))?;
    let name = raw
        .file_name()
        .ok_or_else(|| RpcError::bad_params("path has no name"))?;
    let target = std::fs::canonicalize(parent)
        .map_err(|_| RpcError::bad_params("path not found"))?
        .join(name);
    scope.check(&target)?;
    if !meta.file_type().is_symlink() {
        let canonical = scope.resolve(path)?;
        if scope.is_root(&canonical) {
            return Err(RpcError::forbidden("a root folder cannot be deleted"));
        }
    }
    let result = if meta.is_dir() {
        std::fs::remove_dir(raw)
    } else {
        std::fs::remove_file(raw)
    };
    result.map_err(|e| match e.kind() {
        std::io::ErrorKind::DirectoryNotEmpty => {
            RpcError::bad_params("folder is not empty (only empty folders can be deleted)")
        }
        _ => RpcError::bad_params(format!("cannot delete: {e}")),
    })?;
    Ok(target)
}

/// `fs.mkdir`: create one new folder inside an existing allowed folder.
pub fn mkdir(scope: &Scope, path: &str) -> Result<PathBuf, RpcError> {
    let raw = check_raw(path)?;
    let parent = raw
        .parent()
        .ok_or_else(|| RpcError::bad_params("path has no parent folder"))?;
    let name = raw
        .file_name()
        .ok_or_else(|| RpcError::bad_params("path has no name"))?
        .to_string_lossy()
        .into_owned();
    let safe = crate::files::safe_file_name(&name).map_err(RpcError::bad_params)?;
    if safe != name {
        return Err(RpcError::bad_params(
            "folder name contains characters Windows does not allow",
        ));
    }
    let parent = scope.resolve(&parent.to_string_lossy())?;
    if !parent.is_dir() {
        return Err(RpcError::bad_params("parent is not a folder"));
    }
    let target = parent.join(&name);
    scope.check(&target)?;
    std::fs::create_dir(&target).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => RpcError::bad_params("already exists"),
        _ => RpcError::bad_params(format!("cannot create folder: {e}")),
    })?;
    Ok(target)
}

#[async_trait]
impl Module for FsModule {
    fn methods(&self) -> &'static [MethodSpec] {
        METHODS
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
        ctx: &RequestContext,
    ) -> Result<Value, RpcError> {
        let state = ctx.state.clone();
        let path = str_param(&params, "path")?.to_string();
        let st = state.clone();
        let method_name = method.to_string();
        let (result, touched) = blocking(move || {
            let drives = st.platform.fixed_drives().unwrap_or_default();
            let scope = Scope::new(&st.download_root, &drives, &st.data_dir);
            match method_name.as_str() {
                "fs.list" => Ok((list(&scope, &path)?, None)),
                "fs.delete" => Ok((json!({}), Some(delete(&scope, &path)?))),
                "fs.mkdir" => Ok((json!({}), Some(mkdir(&scope, &path)?))),
                "fs.open" => {
                    let target = scope.resolve(&path)?;
                    st.platform.open_target(&display_path(&target))?;
                    Ok((json!({}), Some(target)))
                }
                other => Err(RpcError::unknown_method(other)),
            }
        })
        .await?;
        if let Some(p) = touched {
            let event = method.replace('.', "_");
            state.audit.record(
                &event,
                Some(&ctx.device_id),
                Some(&ctx.peer),
                &format!("path={}", display_path(&p)),
            );
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_strips_verbatim_prefix() {
        assert_eq!(display_path(Path::new(r"\\?\C:\Users\a")), r"C:\Users\a");
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\srv\share")),
            r"\\srv\share"
        );
        assert_eq!(display_path(Path::new(r"C:\x")), r"C:\x");
    }

    #[test]
    fn scope_rules() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        let data = profile.join("AppData").join("Helm");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("identity.json"), "{}").unwrap();
        std::fs::write(profile.join("a.txt"), "hi").unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let scope = Scope::new(&profile, &[], &data);
        let p = |x: &Path| x.to_string_lossy().into_owned();

        assert_eq!(
            list(&scope, "").unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let l = list(&scope, &p(&profile)).unwrap();
        let names: Vec<&str> = l["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["AppData", "a.txt"]);
        // The data folder is hidden from its parent and refused directly.
        let appdata = list(&scope, &p(&profile.join("AppData"))).unwrap();
        assert!(appdata["entries"].as_array().unwrap().is_empty());
        assert_eq!(list(&scope, &p(&data)).unwrap_err().code, "forbidden");
        assert_eq!(
            delete(&scope, &p(&data.join("identity.json")))
                .unwrap_err()
                .code,
            "forbidden"
        );
        assert_eq!(list(&scope, &p(&outside)).unwrap_err().code, "forbidden");
        assert_eq!(list(&scope, "relative").unwrap_err().code, "bad_params");
        let dotdot = format!("{}\\..\\outside", p(&profile));
        assert_eq!(list(&scope, &dotdot).unwrap_err().code, "bad_params");
        assert_eq!(delete(&scope, &p(&profile)).unwrap_err().code, "forbidden");

        let made = profile.join("New");
        mkdir(&scope, &p(&made)).unwrap();
        assert!(made.is_dir());
        assert_eq!(mkdir(&scope, &p(&made)).unwrap_err().code, "bad_params");
        assert_eq!(
            mkdir(&scope, &p(&profile.join("bad:name")))
                .unwrap_err()
                .code,
            "bad_params"
        );
        assert_eq!(
            mkdir(&scope, &p(&outside.join("x"))).unwrap_err().code,
            "forbidden"
        );
        std::fs::write(made.join("f"), "x").unwrap();
        assert_eq!(delete(&scope, &p(&made)).unwrap_err().code, "bad_params");
        delete(&scope, &p(&made.join("f"))).unwrap();
        delete(&scope, &p(&made)).unwrap();
        assert!(!made.exists());
    }
}
