//! Launching apps and opening files, folders and URLs.

use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    AO_NONE, ApplicationActivationManager, IApplicationActivationManager, SEE_MASK_FLAG_NO_UI,
    SEE_MASK_INVOKEIDLIST, SHELLEXECUTEINFOW, SHGetIDListFromObject, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR};

use super::{apps, win_err};
use crate::{AppEntry, AppKind, Result, util::Target};

pub(crate) fn launch(app: &AppEntry) -> Result<()> {
    if app.kind == AppKind::Packaged {
        match activate_packaged(&app.app_id) {
            Ok(()) => return Ok(()),
            // Fall through to the shell, which can also start packaged apps.
            Err(first) => return invoke_apps_folder_item(&app.app_id).map_err(|_| first),
        }
    }
    invoke_apps_folder_item(&app.app_id)
}

fn activate_packaged(aumid: &str) -> Result<()> {
    // SAFETY: COM calls on the STA; strings outlive the calls.
    unsafe {
        let mgr: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                .map_err(win_err("CoCreateInstance(ApplicationActivationManager)"))?;
        mgr.ActivateApplication(&HSTRING::from(aumid), PCWSTR::null(), AO_NONE)
            .map_err(win_err(
                "IApplicationActivationManager::ActivateApplication",
            ))?;
    }
    Ok(())
}

/// Invokes the default verb of the AppsFolder item (what clicking it in Start does).
fn invoke_apps_folder_item(app_id: &str) -> Result<()> {
    let item = apps::item_for(app_id)?;
    // SAFETY: the ID list is freed after ShellExecuteExW returns.
    unsafe {
        let pidl = SHGetIDListFromObject(&item).map_err(win_err("SHGetIDListFromObject"))?;
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_FLAG_NO_UI,
            lpIDList: pidl as *mut _,
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let r = ShellExecuteExW(&mut sei).map_err(win_err("ShellExecuteExW(AppsFolder item)"));
        CoTaskMemFree(Some(pidl as *const _));
        r
    }
}

pub(crate) fn open(target: &Target) -> Result<()> {
    let file = match target {
        Target::Url(u) => HSTRING::from(u.as_str()),
        Target::Path(p) => HSTRING::from(p.as_str()),
    };
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI,
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `file` outlives the call; struct is fully initialised.
    unsafe { ShellExecuteExW(&mut sei).map_err(win_err("ShellExecuteExW")) }
}
