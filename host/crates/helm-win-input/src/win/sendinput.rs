//! `SendInput` sink (Microsoft Learn: SendInput, INPUT, MOUSEINPUT, KEYBDINPUT).

use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC_EX,
    MOUSE_EVENT_FLAGS, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};

use crate::error::{Error, Result};
use crate::event::MouseButton;
use crate::inject::{INJECTED_MARKER, Op, Sink};
use crate::keys::is_extended_vk;

/// Inputs per `SendInput` call; long texts are split into several batches.
const BATCH: usize = 1024;

const XBUTTON1: u32 = 0x0001;
const XBUTTON2: u32 = 0x0002;

/// Sends operations with `SendInput`. Stateless, so it is trivially `Send`.
pub struct SendInputSink;

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_MARKER,
            },
        },
    }
}

fn keyboard(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INJECTED_MARKER,
            },
        },
    }
}

fn to_input(op: Op) -> INPUT {
    match op {
        Op::Move { dx, dy } => mouse(dx, dy, 0, MOUSEEVENTF_MOVE),
        Op::Wheel { delta } => mouse(0, 0, delta as u32, MOUSEEVENTF_WHEEL),
        Op::HWheel { delta } => mouse(0, 0, delta as u32, MOUSEEVENTF_HWHEEL),
        Op::Button { button, down } => {
            let (flags, data) = match (button, down) {
                (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
                (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
                (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                (MouseButton::X1, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
                (MouseButton::X1, false) => (MOUSEEVENTF_XUP, XBUTTON1),
                (MouseButton::X2, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
                (MouseButton::X2, false) => (MOUSEEVENTF_XUP, XBUTTON2),
            };
            mouse(0, 0, data, flags)
        }
        Op::Key { vk, down } => {
            // MAPVK_VK_TO_VSC_EX puts 0xE0/0xE1 in the high byte for extended keys.
            let mapped = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC_EX) };
            let scan = (mapped & 0xFF) as u16;
            let prefix = (mapped >> 8) & 0xFF;
            let mut flags = KEYBD_EVENT_FLAGS(0);
            if prefix == 0xE0 || prefix == 0xE1 || is_extended_vk(vk) {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            keyboard(vk, scan, flags)
        }
        Op::Unicode { unit, down } => {
            let mut flags = KEYEVENTF_UNICODE;
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            keyboard(0, unit, flags)
        }
    }
}

impl Sink for SendInputSink {
    fn send(&mut self, ops: &[Op]) -> Result<()> {
        for chunk in ops.chunks(BATCH) {
            let inputs: Vec<INPUT> = chunk.iter().copied().map(to_input).collect();
            let inserted = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
            if inserted as usize != inputs.len() {
                let code = unsafe { GetLastError() };
                // SendInput returns 0 when UIPI blocks it; the docs note GetLastError may not
                // say why, and in practice it is either 0 or ERROR_ACCESS_DENIED.
                if inserted == 0 && (code == ERROR_ACCESS_DENIED || code.0 == 0) {
                    return Err(Error::BlockedByElevation);
                }
                return Err(Error::SendInputFailed {
                    code: code.0,
                    inserted,
                    requested: inputs.len() as u32,
                });
            }
        }
        Ok(())
    }
}
