//! # helm-win-notify
//!
//! Windows toast notifications for the Helm PC agent (mirroring phone notifications): title,
//! body, source-app attribution, optional image, up to 5 buttons and an inline reply box, with a
//! callback for activation / dismissal.
//!
//! Works for an **unpackaged** app: [`register_app`] writes the AppUserModelID under
//! `HKCU\Software\Classes\AppUserModelId\<aumid>` (display name + icon), then WinRT
//! `ToastNotificationManager::CreateToastNotifierWithId(aumid)` shows toasts under that name.
//!
//! [`model`] (XML, argument encoding, validation) is pure and tested everywhere. On non-Windows
//! targets [`Notifier::new`] returns [`Error::Unsupported`].

pub mod model;

pub use model::{
    AppRegistration, DismissReason, ReplyBox, Toast, ToastAction, ToastEvent, toast_xml, validate_aumid,
};

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// A Windows API call failed.
    Windows { context: &'static str, code: i32, message: String },
    /// Bad AUMID, empty title, too many buttons, relative image path, …
    InvalidInput(String),
    /// Not available on this platform.
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Windows { context, code, message } => {
                write!(f, "{context} failed: {message} (0x{:08X})", *code as u32)
            }
            Error::InvalidInput(m) => write!(f, "invalid input: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::{Notifier, ShownToast, register_app, unregister_app};

#[cfg(not(windows))]
mod stub {
    use super::*;

    pub fn register_app(_reg: &AppRegistration) -> Result<()> {
        Err(Error::Unsupported("toasts need Windows".into()))
    }
    pub fn unregister_app(_aumid: &str) -> Result<()> {
        Err(Error::Unsupported("toasts need Windows".into()))
    }
    pub struct Notifier(());
    pub struct ShownToast(());
    impl Notifier {
        pub fn new(_reg: &AppRegistration) -> Result<Notifier> {
            Err(Error::Unsupported("toasts need Windows".into()))
        }
        pub fn show(&self, _t: &Toast, _cb: impl Fn(ToastEvent) + Send + Sync + 'static) -> Result<ShownToast> {
            Err(Error::Unsupported("toasts need Windows".into()))
        }
    }
}
#[cfg(not(windows))]
pub use stub::{Notifier, ShownToast, register_app, unregister_app};
