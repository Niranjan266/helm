//! Test double for [`PresentBackend`]: canned foreground window and PowerPoint state, every
//! laser call recorded. Nothing touches the PC.

use std::sync::{Arc, Mutex};

use super::{PresentBackend, PresentHub, SlideInfo};
use crate::platform::{ForegroundInfo, PResult, Rect};
use crate::router::RpcError;

/// What the fake laser overlay was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub enum LaserCall {
    Show { rect: Rect, x: f32, y: f32, trail: bool },
    Hide,
}

#[derive(Debug, Default)]
struct State {
    foreground: Option<ForegroundInfo>,
    /// `None` = automation unsupported; `Some(None)` = PowerPoint without a presentation.
    powerpoint: Option<Option<SlideInfo>>,
    laser: Vec<LaserCall>,
}

#[derive(Debug, Default)]
pub struct FakePresent {
    state: Mutex<State>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl FakePresent {
    /// Pretend `exe_name` with `title` is in front.
    pub fn set_foreground(&self, exe_name: &str, title: &str) {
        lock(&self.state).foreground = Some(ForegroundInfo {
            hwnd: 4242,
            pid: 4343,
            exe: Some(format!("C:\\Fake\\{exe_name}")),
            exe_name: exe_name.into(),
            title: title.into(),
        });
    }

    /// `None` = automation unsupported; `Some(None)` = no open presentation.
    pub fn set_powerpoint(&self, info: Option<Option<SlideInfo>>) {
        lock(&self.state).powerpoint = info;
    }

    pub fn laser_log(&self) -> Vec<LaserCall> {
        lock(&self.state).laser.clone()
    }
}

impl PresentBackend for FakePresent {
    fn foreground(&self) -> PResult<Option<ForegroundInfo>> {
        Ok(lock(&self.state).foreground.clone())
    }
    fn powerpoint(&self) -> PResult<Option<SlideInfo>> {
        lock(&self.state)
            .powerpoint
            .clone()
            .ok_or_else(|| RpcError::unsupported("PowerPoint is not running (fake)"))
    }
    fn laser_show(&self, rect: Rect, x: f32, y: f32, trail: bool) -> PResult<()> {
        lock(&self.state).laser.push(LaserCall::Show { rect, x, y, trail });
        Ok(())
    }
    fn laser_hide(&self) -> PResult<()> {
        lock(&self.state).laser.push(LaserCall::Hide);
        Ok(())
    }
}

/// A hub over a [`FakePresent`] (held by `FakePlatform`).
#[derive(Debug)]
pub struct FakePresenter {
    hub: Arc<PresentHub>,
    fake: Arc<FakePresent>,
}

impl Default for FakePresenter {
    fn default() -> Self {
        let fake = Arc::new(FakePresent::default());
        FakePresenter {
            hub: Arc::new(PresentHub::new(fake.clone())),
            fake,
        }
    }
}

impl FakePresenter {
    pub fn hub(&self) -> Arc<PresentHub> {
        self.hub.clone()
    }
    pub fn fake(&self) -> Arc<FakePresent> {
        self.fake.clone()
    }
    pub fn parts(self) -> (Arc<PresentHub>, Arc<FakePresent>) {
        (self.hub, self.fake)
    }
}
