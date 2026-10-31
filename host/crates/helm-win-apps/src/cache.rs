//! Simple on-disk PNG icon cache.

use std::path::{Path, PathBuf};

use crate::{Result, util};

/// Stores icon PNGs in a directory, one file per (app id, size).
///
/// Files are written atomically (temp file + rename). The cache never expires entries by
/// itself; call [`IconCache::clear`] after app installs/updates if stale icons matter.
#[derive(Debug, Clone)]
pub struct IconCache {
    dir: PathBuf,
}

impl IconCache {
    /// Creates the cache, creating `dir` (and parents) if needed.
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// The cache directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of the cache file for this app id and size (the size is clamped like
    /// [`crate::app_icon_png`] does).
    pub fn path_for(&self, app_id: &str, size_px: u32) -> PathBuf {
        self.dir
            .join(util::icon_cache_key(app_id, util::clamp_icon_size(size_px)))
    }

    /// Returns cached bytes, if present.
    pub fn get(&self, app_id: &str, size_px: u32) -> Option<Vec<u8>> {
        std::fs::read(self.path_for(app_id, size_px))
            .ok()
            .filter(|b| !b.is_empty())
    }

    /// Stores bytes for this app id and size.
    pub fn put(&self, app_id: &str, size_px: u32, png: &[u8]) -> Result<()> {
        let path = self.path_for(app_id, size_px);
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, png)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Returns the cached icon or computes it with `fetch` and stores it.
    pub fn get_or_insert_with(
        &self,
        app_id: &str,
        size_px: u32,
        fetch: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        if let Some(bytes) = self.get(app_id, size_px) {
            return Ok(bytes);
        }
        let bytes = fetch()?;
        self.put(app_id, size_px, &bytes)?;
        Ok(bytes)
    }

    /// Returns the cached icon or extracts it with [`crate::app_icon_png`].
    pub fn icon_png(&self, app_id: &str, size_px: u32) -> Result<Vec<u8>> {
        self.get_or_insert_with(app_id, size_px, || crate::app_icon_png(app_id, size_px))
    }

    /// Deletes every cached `.png` file in the cache directory.
    pub fn clear(&self) -> Result<()> {
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "png") {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("helm-win-apps-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn put_get_clear() {
        let dir = temp_dir("cache");
        let cache = IconCache::new(&dir).unwrap();
        assert!(cache.get("App_8wekyb3d8bbwe!App", 48).is_none());
        cache
            .put("App_8wekyb3d8bbwe!App", 48, b"png-bytes")
            .unwrap();
        assert_eq!(
            cache.get("app_8wekyb3d8bbwe!app", 48).unwrap(),
            b"png-bytes"
        );
        assert!(cache.get("App_8wekyb3d8bbwe!App", 32).is_none());
        // Clamped sizes share a key.
        assert_eq!(cache.path_for("x", 1), cache.path_for("x", 16));
        assert!(cache.path_for("{G}\\..\\..\\x", 32).starts_with(&dir));

        let mut calls = 0;
        let v = cache
            .get_or_insert_with("other", 32, || {
                calls += 1;
                Ok(b"fresh".to_vec())
            })
            .unwrap();
        assert_eq!(v, b"fresh");
        let v2 = cache
            .get_or_insert_with("other", 32, || panic!("should be cached"))
            .unwrap();
        assert_eq!(v2, b"fresh");
        assert_eq!(calls, 1);

        let err =
            cache.get_or_insert_with("failing", 32, || Err(crate::Error::NotFound("x".into())));
        assert!(err.is_err());
        assert!(cache.get("failing", 32).is_none());

        cache.clear().unwrap();
        assert!(cache.get("other", 32).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
