use serde::{Deserialize, Serialize};

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

impl MouseButton {
    pub(crate) const ALL: [MouseButton; 5] = [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::X1,
        MouseButton::X2,
    ];

    pub(crate) fn bit(self) -> u8 {
        match self {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
            MouseButton::X1 => 8,
            MouseButton::X2 => 16,
        }
    }
}

/// One input frame from the phone (ROADMAP §1.1: `{"type":"in","k":"<kind>",…}`).
///
/// The `k` field selects the kind. Extra fields such as `"type":"in"` are ignored when
/// deserializing, so the agent can feed the whole frame straight in.
///
/// | `k`  | variant       | fields                                   |
/// |------|---------------|------------------------------------------|
/// | `mm` | `MouseMove`   | `dx`, `dy` – pixels, relative, fractional |
/// | `mb` | `MouseButton` | `button` (`left`…`x2`), `down`           |
/// | `ms` | `Scroll`      | `dx`, `dy` – wheel notches, fractional   |
/// | `kd` | `KeyDown`     | `vk` – Windows virtual-key code          |
/// | `ku` | `KeyUp`       | `vk`                                     |
/// | `tx` | `Text`        | `text`                                   |
/// | `sc` | `Shortcut`    | `keys` – VKs pressed in order, released in reverse |
///
/// Scroll direction follows the Windows wheel: `dy > 0` turns the wheel forward
/// (scrolls up), `dx > 0` scrolls right.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum InputEvent {
    #[serde(rename = "mm")]
    MouseMove { dx: f32, dy: f32 },
    #[serde(rename = "mb")]
    MouseButton { button: MouseButton, down: bool },
    #[serde(rename = "ms")]
    Scroll {
        #[serde(default)]
        dx: f32,
        #[serde(default)]
        dy: f32,
    },
    #[serde(rename = "kd")]
    KeyDown { vk: u16 },
    #[serde(rename = "ku")]
    KeyUp { vk: u16 },
    #[serde(rename = "tx")]
    Text { text: String },
    #[serde(rename = "sc")]
    Shortcut { keys: Vec<u16> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(s: &str) -> InputEvent {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn mouse_move_shape() {
        let ev = parse(r#"{"k":"mm","dx":1.5,"dy":-2}"#);
        assert_eq!(ev, InputEvent::MouseMove { dx: 1.5, dy: -2.0 });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["k"], "mm");
        assert_eq!(v["dx"].as_f64(), Some(1.5));
        assert_eq!(v["dy"].as_f64(), Some(-2.0));
    }

    #[test]
    fn full_frame_with_type_field_is_accepted() {
        let ev = parse(r#"{"type":"in","k":"mb","button":"x2","down":true}"#);
        assert_eq!(
            ev,
            InputEvent::MouseButton {
                button: MouseButton::X2,
                down: true
            }
        );
    }

    #[test]
    fn all_tags_round_trip() {
        let cases = vec![
            (InputEvent::MouseMove { dx: 1.0, dy: 2.0 }, "mm"),
            (
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    down: false,
                },
                "mb",
            ),
            (InputEvent::Scroll { dx: 0.0, dy: -0.25 }, "ms"),
            (InputEvent::KeyDown { vk: 0x41 }, "kd"),
            (InputEvent::KeyUp { vk: 0x41 }, "ku"),
            (
                InputEvent::Text {
                    text: "hi 😀".into(),
                },
                "tx",
            ),
            (
                InputEvent::Shortcut {
                    keys: vec![0x11, 0x43],
                },
                "sc",
            ),
        ];
        for (ev, tag) in cases {
            let v = serde_json::to_value(&ev).unwrap();
            assert_eq!(v["k"], tag);
            let back: InputEvent = serde_json::from_value(v).unwrap();
            assert_eq!(back, ev);
        }
    }

    #[test]
    fn exact_shapes() {
        assert_eq!(
            serde_json::to_value(InputEvent::MouseButton {
                button: MouseButton::Middle,
                down: true
            })
            .unwrap(),
            json!({"k":"mb","button":"middle","down":true})
        );
        assert_eq!(
            serde_json::to_value(InputEvent::KeyDown { vk: 13 }).unwrap(),
            json!({"k":"kd","vk":13})
        );
        assert_eq!(
            serde_json::to_value(InputEvent::Text {
                text: "a\nb".into()
            })
            .unwrap(),
            json!({"k":"tx","text":"a\nb"})
        );
        assert_eq!(
            serde_json::to_value(InputEvent::Shortcut { keys: vec![91, 68] }).unwrap(),
            json!({"k":"sc","keys":[91,68]})
        );
    }

    #[test]
    fn scroll_axes_default_to_zero() {
        assert_eq!(
            parse(r#"{"k":"ms","dy":1}"#),
            InputEvent::Scroll { dx: 0.0, dy: 1.0 }
        );
        assert_eq!(
            parse(r#"{"k":"ms","dx":-0.5}"#),
            InputEvent::Scroll { dx: -0.5, dy: 0.0 }
        );
    }

    #[test]
    fn rejects_unknown_kind_and_bad_values() {
        assert!(serde_json::from_str::<InputEvent>(r#"{"k":"zz"}"#).is_err());
        assert!(serde_json::from_str::<InputEvent>(r#"{"k":"kd","vk":70000}"#).is_err());
        assert!(
            serde_json::from_str::<InputEvent>(r#"{"k":"mb","button":"back","down":true}"#)
                .is_err()
        );
    }
}
