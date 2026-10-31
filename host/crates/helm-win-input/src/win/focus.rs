//! UI Automation focus watcher (Microsoft Learn: IUIAutomation::AddFocusChangedEventHandler,
//! IUIAutomationFocusChangedEventHandler, IUIAutomationCacheRequest).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl,
    IUIAutomationValuePattern, UIA_ControlTypePropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_IsValuePatternAvailablePropertyId, UIA_ValueIsReadOnlyPropertyId, UIA_ValuePatternId,
};
use windows_core::{Ref, implement};

use crate::error::{Error, Result};
use crate::focus::{DEBOUNCE, Debouncer, is_text_target};

enum Msg {
    Focus(bool),
    Stop,
}

#[implement(IUIAutomationFocusChangedEventHandler)]
struct FocusHandler {
    tx: Sender<Msg>,
}

impl IUIAutomationFocusChangedEventHandler_Impl for FocusHandler_Impl {
    fn HandleFocusChangedEvent(
        &self,
        sender: Ref<IUIAutomationElement>,
    ) -> windows_core::Result<()> {
        // Only cached properties are read here, so no cross-process calls happen on UIA's
        // event thread; the result goes to our own thread via the channel.
        let value = sender.as_ref().map(evaluate).unwrap_or(false);
        let _ = self.tx.send(Msg::Focus(value));
        Ok(())
    }
}

fn evaluate(element: &IUIAutomationElement) -> bool {
    unsafe {
        let control_type = element.CachedControlType().map(|c| c.0).unwrap_or(0);
        let focusable = element
            .CachedIsKeyboardFocusable()
            .map(|b| b.as_bool())
            .unwrap_or(false);
        let read_only = element
            .GetCachedPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok()
            .and_then(|p| p.CachedIsReadOnly().ok())
            .map(|b| b.as_bool());
        is_text_target(control_type, focusable, read_only)
    }
}

pub(crate) struct Worker {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

type Callback = Box<dyn Fn(bool) + Send + 'static>;

impl Worker {
    pub(crate) fn start(callback: Callback) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let handler_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("helm-uia-focus".into())
            .spawn(move || run(handler_tx, rx, ready_tx, callback))
            .map_err(|e| Error::Thread(e.to_string()))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                tx,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::Thread(
                    "focus watcher thread exited during start-up".into(),
                ))
            }
        }
    }

    pub(crate) fn stop(mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct Registration {
    automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    handler: IUIAutomationFocusChangedEventHandler,
}

impl Drop for Registration {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .automation
                .RemoveFocusChangedEventHandler(&self.handler);
        }
    }
}

fn register(tx: Sender<Msg>) -> Result<Registration> {
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
                .map_err(|e| Error::os(e, "CoCreateInstance(CUIAutomation)"))?;
        let cache = automation
            .CreateCacheRequest()
            .map_err(|e| Error::os(e, "CreateCacheRequest"))?;
        for prop in [
            UIA_ControlTypePropertyId,
            UIA_IsKeyboardFocusablePropertyId,
            UIA_IsValuePatternAvailablePropertyId,
            UIA_ValueIsReadOnlyPropertyId,
        ] {
            cache
                .AddProperty(prop)
                .map_err(|e| Error::os(e, "AddProperty"))?;
        }
        cache
            .AddPattern(UIA_ValuePatternId)
            .map_err(|e| Error::os(e, "AddPattern"))?;
        let handler: IUIAutomationFocusChangedEventHandler = FocusHandler { tx }.into();
        automation
            .AddFocusChangedEventHandler(&cache, &handler)
            .map_err(|e| Error::os(e, "AddFocusChangedEventHandler"))?;
        Ok(Registration {
            automation,
            cache,
            handler,
        })
    }
}

fn run(handler_tx: Sender<Msg>, rx: Receiver<Msg>, ready: Sender<Result<()>>, callback: Callback) {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() {
        let _ = ready.send(Err(Error::Os {
            code: hr.0,
            context: "CoInitializeEx",
        }));
        return;
    }
    let _com = ComGuard;
    let reg = match register(handler_tx) {
        Ok(r) => r,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let report = |v: bool| {
        // A panicking callback must not take the watcher (and the UIA registration) down.
        let _ = catch_unwind(AssertUnwindSafe(|| callback(v)));
    };
    let mut debounce = Debouncer::new(DEBOUNCE);
    let initial = unsafe { reg.automation.GetFocusedElementBuildCache(&reg.cache) }
        .map(|e| evaluate(&e))
        .unwrap_or(false);
    if let Some(v) = debounce.force(initial) {
        report(v);
    }

    loop {
        let msg = match debounce.deadline() {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Msg::Focus(v)) => debounce.push(v, Instant::now()),
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if let Some(v) = debounce.poll(Instant::now()) {
            report(v);
        }
    }
    drop(reg); // RemoveFocusChangedEventHandler before CoUninitialize
}
