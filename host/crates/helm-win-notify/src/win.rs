//! WinRT toasts + HKCU AUMID registration.

use std::sync::{Arc, OnceLock};

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastDismissalReason, ToastDismissedEventArgs, ToastFailedEventArgs, ToastNotification,
    ToastNotificationManager, ToastNotifier,
};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegSetValueExW,
};
use windows::core::{HSTRING, IInspectable, Interface, PCWSTR};

use crate::model::{self, AppRegistration, DismissReason, REPLY_INPUT_ID, Toast, ToastEvent};
use crate::{Error, Result};

fn win_err(context: &'static str, e: windows::core::Error) -> Error {
    Error::Windows { context, code: e.code().0, message: e.message() }
}

/// Keep the process MTA alive so WinRT calls work from any thread (no per-thread CoInitialize).
fn ensure_mta() -> Result<()> {
    static MTA: OnceLock<std::result::Result<(), (i32, String)>> = OnceLock::new();
    MTA.get_or_init(|| match unsafe { CoIncrementMTAUsage() } {
        // The cookie is intentionally leaked: the MTA stays up for the process lifetime.
        Ok(_cookie) => Ok(()),
        Err(e) => Err((e.code().0, e.message())),
    })
    .clone()
    .map_err(|(code, message)| Error::Windows { context: "CoIncrementMTAUsage", code, message })
}

fn key_path(aumid: &str) -> HSTRING {
    HSTRING::from(format!(r"Software\Classes\AppUserModelId\{aumid}"))
}

fn set_string(key: HKEY, name: &str, value: &str) -> Result<()> {
    let name = HSTRING::from(name);
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2) };
    unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) }
        .ok()
        .map_err(|e| win_err("RegSetValueExW", e))
}

/// Register (or update) the AUMID for the current user: `DisplayName` and optional `IconUri`.
/// No admin rights needed. Idempotent.
pub fn register_app(reg: &AppRegistration) -> Result<()> {
    model::validate_aumid(&reg.aumid)?;
    if reg.display_name.trim().is_empty() {
        return Err(Error::InvalidInput("display name is empty".into()));
    }
    let path = key_path(&reg.aumid);
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    }
    .ok()
    .map_err(|e| win_err("RegCreateKeyExW", e))?;
    let res = (|| {
        set_string(key, "DisplayName", &reg.display_name)?;
        if let Some(icon) = &reg.icon_path {
            if !icon.is_absolute() {
                return Err(Error::InvalidInput("icon path must be absolute".into()));
            }
            set_string(key, "IconUri", &icon.to_string_lossy())?;
        }
        Ok(())
    })();
    unsafe {
        let _ = RegCloseKey(key);
    }
    res
}

/// Remove the AUMID registration. Missing key is not an error.
pub fn unregister_app(aumid: &str) -> Result<()> {
    model::validate_aumid(aumid)?;
    let path = key_path(aumid);
    let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
    if r.is_ok() || r == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(win_err("RegDeleteTreeW", r.into()))
    }
}

/// Shows toasts under one registered AUMID. `Send + Sync`.
pub struct Notifier {
    notifier: ToastNotifier,
    aumid: String,
}

/// A toast that was shown; keep it to [`ShownToast::hide`] it. Dropping it does not hide it,
/// and the callback stays registered while Windows keeps the toast.
pub struct ShownToast {
    toast: ToastNotification,
    notifier: ToastNotifier,
}

impl ShownToast {
    /// Remove the toast from the screen and Action Center.
    pub fn hide(&self) -> Result<()> {
        self.notifier.Hide(&self.toast).map_err(|e| win_err("ToastNotifier.Hide", e))
    }
}

impl Notifier {
    /// Register the AUMID in HKCU ([`register_app`]) and create the notifier.
    pub fn new(reg: &AppRegistration) -> Result<Notifier> {
        register_app(reg)?;
        ensure_mta()?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(&reg.aumid))
            .map_err(|e| win_err("CreateToastNotifierWithId", e))?;
        Ok(Notifier { notifier, aumid: reg.aumid.clone() })
    }

    pub fn aumid(&self) -> &str {
        &self.aumid
    }

    /// Show a toast. `on_event` runs on a system thread when the user clicks the body, a button
    /// or sends a reply ([`ToastEvent::Activated`]), or when the toast is dismissed / fails.
    ///
    /// Activation events arrive only while this process runs (no COM activator is registered).
    pub fn show(&self, toast: &Toast, on_event: impl Fn(ToastEvent) + Send + Sync + 'static) -> Result<ShownToast> {
        let xml = model::toast_xml(toast)?;
        let doc = XmlDocument::new().map_err(|e| win_err("XmlDocument", e))?;
        doc.LoadXml(&HSTRING::from(xml)).map_err(|e| win_err("XmlDocument.LoadXml", e))?;
        let t = ToastNotification::CreateToastNotification(&doc).map_err(|e| win_err("CreateToastNotification", e))?;
        if let Some(tag) = &toast.tag {
            t.SetTag(&HSTRING::from(tag)).map_err(|e| win_err("ToastNotification.Tag", e))?;
        }
        if let Some(group) = &toast.group {
            t.SetGroup(&HSTRING::from(group)).map_err(|e| win_err("ToastNotification.Group", e))?;
        }

        let cb: Arc<dyn Fn(ToastEvent) + Send + Sync> = Arc::new(on_event);
        let c = cb.clone();
        t.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(move |_, args| {
            let (action_id, reply) = match args.as_ref().and_then(|a| a.cast::<ToastActivatedEventArgs>().ok()) {
                Some(a) => {
                    let arg = a.Arguments().map(|s| s.to_string()).unwrap_or_default();
                    (model::parse_argument(&arg), reply_text(&a))
                }
                None => (None, None),
            };
            c(ToastEvent::Activated { action_id, reply });
            Ok(())
        }))
        .map_err(|e| win_err("ToastNotification.Activated", e))?;
        let c = cb.clone();
        t.Dismissed(&TypedEventHandler::<ToastNotification, ToastDismissedEventArgs>::new(move |_, args| {
            let reason = match args.as_ref().and_then(|a| a.Reason().ok()) {
                Some(ToastDismissalReason::ApplicationHidden) => DismissReason::ApplicationHidden,
                Some(ToastDismissalReason::TimedOut) => DismissReason::TimedOut,
                _ => DismissReason::UserCanceled,
            };
            c(ToastEvent::Dismissed { reason });
            Ok(())
        }))
        .map_err(|e| win_err("ToastNotification.Dismissed", e))?;
        let c = cb;
        t.Failed(&TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(move |_, args| {
            let message = match args.as_ref().and_then(|a| a.ErrorCode().ok()) {
                Some(hr) => format!("{} (0x{:08X})", hr.message(), hr.0 as u32),
                None => "unknown error".into(),
            };
            c(ToastEvent::Failed { message });
            Ok(())
        }))
        .map_err(|e| win_err("ToastNotification.Failed", e))?;

        self.notifier.Show(&t).map_err(|e| win_err("ToastNotifier.Show", e))?;
        Ok(ShownToast { toast: t, notifier: self.notifier.clone() })
    }

    /// Remove every toast of this app with `tag` (and `group`, if given) from Action Center.
    pub fn remove(&self, tag: &str, group: Option<&str>) -> Result<()> {
        let history = ToastNotificationManager::History().map_err(|e| win_err("ToastNotificationManager.History", e))?;
        let tag = HSTRING::from(tag);
        let id = HSTRING::from(&self.aumid);
        match group {
            Some(g) => history.RemoveGroupedTagWithId(&tag, &HSTRING::from(g), &id),
            None => history.RemoveGroupedTagWithId(&tag, &HSTRING::new(), &id),
        }
        .map_err(|e| win_err("ToastNotificationHistory.Remove", e))
    }

    /// Remove all of this app's toasts from Action Center.
    pub fn clear(&self) -> Result<()> {
        let history = ToastNotificationManager::History().map_err(|e| win_err("ToastNotificationManager.History", e))?;
        history.ClearWithId(&HSTRING::from(&self.aumid)).map_err(|e| win_err("ToastNotificationHistory.Clear", e))
    }
}

fn reply_text(args: &ToastActivatedEventArgs) -> Option<String> {
    let input = args.UserInput().ok()?;
    let v = input.Lookup(&HSTRING::from(REPLY_INPUT_ID)).ok()?;
    let s = v.cast::<IPropertyValue>().ok()?.GetString().ok()?.to_string();
    Some(s)
}

// Compile-time check: the agent keeps a Notifier in shared state.
#[allow(dead_code)]
fn _assert_send_sync() {
    fn is<T: Send + Sync>() {}
    is::<Notifier>();
    is::<ShownToast>();
}
