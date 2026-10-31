//! Enumerating `shell:AppsFolder`.

use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItem2, KF_FLAG_DEFAULT,
    SHCreateItemFromParsingName, SHGetKnownFolderItem, SHGetKnownFolderPath, SIGDN_NORMALDISPLAY,
    SIGDN_PARENTRELATIVEPARSING,
};
use windows::core::{GUID, HSTRING, Interface};

use super::{take_co_string, win_err};
use crate::{AppEntry, AppKind, Result, util};

/// `System.Link.TargetParsingPath` (propkey.h): {B9B4B3FC-2B51-4A42-B5D8-324146AFCF25}, 2.
const PKEY_LINK_TARGET_PARSING_PATH: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25),
    pid: 2,
};

/// Upper bound on items read, as a guard against a misbehaving enumerator.
const MAX_ITEMS: usize = 20_000;

/// Enumerates every item in `shell:AppsFolder`. Must run on the STA worker.
pub(crate) fn enumerate() -> Result<Vec<AppEntry>> {
    // SAFETY: COM calls on an initialised STA; all out-params are owned values.
    unsafe {
        let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)
            .map_err(win_err("SHGetKnownFolderItem(AppsFolder)"))?;
        let items: IEnumShellItems = folder
            .BindToHandler(None, &BHID_EnumItems)
            .map_err(win_err("IShellItem::BindToHandler(BHID_EnumItems)"))?;
        let mut out = Vec::new();
        while out.len() < MAX_ITEMS {
            let mut batch: [Option<IShellItem>; 32] = Default::default();
            let mut fetched = 0u32;
            // Returns S_FALSE (still Ok) when fewer than requested remain.
            items
                .Next(&mut batch, Some(&mut fetched))
                .map_err(win_err("IEnumShellItems::Next"))?;
            if fetched == 0 {
                break;
            }
            for item in batch.iter().take(fetched as usize).flatten() {
                if let Some(entry) = entry_for(item) {
                    out.push(entry);
                }
            }
        }
        Ok(util::dedupe_and_sort(out))
    }
}

/// Builds an [`AppEntry`] from one AppsFolder child; `None` if it has no parsing name.
unsafe fn entry_for(item: &IShellItem) -> Option<AppEntry> {
    // SAFETY: COM calls on a valid item; strings are freed by take_co_string.
    unsafe {
        let app_id = take_co_string(item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING).ok()?)?;
        if app_id.is_empty() {
            return None;
        }
        let name = item
            .GetDisplayName(SIGDN_NORMALDISPLAY)
            .ok()
            .and_then(|p| take_co_string(p))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| app_id.clone());
        let kind = util::kind_for(&app_id);
        let target_path = match kind {
            AppKind::Packaged => None,
            AppKind::Desktop => target_path(item, &app_id),
        };
        Some(AppEntry {
            app_id,
            name,
            kind,
            target_path,
        })
    }
}

/// Best-effort file-system target of a desktop app.
unsafe fn target_path(item: &IShellItem, app_id: &str) -> Option<String> {
    // SAFETY: COM calls on a valid item.
    unsafe {
        if let Ok(item2) = item.cast::<IShellItem2>()
            && let Ok(p) = item2.GetString(&PKEY_LINK_TARGET_PARSING_PATH)
            && let Some(s) = take_co_string(p).filter(|s| !s.is_empty())
        {
            return Some(s);
        }
    }
    resolve_known_folder_path(app_id)
}

/// Turns `{KnownFolderGuid}\rest` into an absolute path, or returns absolute paths as-is.
fn resolve_known_folder_path(parsing_name: &str) -> Option<String> {
    if let Some((guid, rest)) = util::split_known_folder_path(parsing_name) {
        let guid = GUID::from_u128(util::parse_guid_u128(guid)?);
        // SAFETY: valid GUID pointer; result freed by take_co_string.
        let base =
            unsafe { take_co_string(SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None).ok()?)? };
        return Some(format!("{}\\{}", base.trim_end_matches('\\'), rest));
    }
    let b = parsing_name.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
        return Some(parsing_name.to_string());
    }
    None
}

/// Creates the shell item for an AppsFolder child from its parsing name.
pub(crate) fn item_for(app_id: &str) -> Result<IShellItem> {
    let path = HSTRING::from(format!("shell:AppsFolder\\{app_id}"));
    // SAFETY: valid NUL-terminated string; COM initialised on this thread.
    unsafe {
        SHCreateItemFromParsingName(&path, None).map_err(win_err("SHCreateItemFromParsingName"))
    }
}
