//! Deck action JSON validation (PROTOCOL.md §9.3 and §11).
//!
//! An action is a JSON object with exactly one key, for example
//! `{"hotkey":"ctrl+shift+esc"}` or `{"macro":[{"text":"hi"},{"delay_ms":100}]}`.
//! This module only checks shape and limits; running actions is the agent's job.

use serde_json::Value;

/// Every action kind the agent understands.
pub const ACTION_KINDS: &[&str] = &[
    "hotkey",
    "text",
    "launch",
    "open",
    "media",
    "volume_step",
    "delay_ms",
    "macro",
    "repeat",
    "remote",
];

/// Maximum steps in a `macro` (PROTOCOL.md §11).
pub const MAX_MACRO_STEPS: usize = 50;
/// Maximum `repeat.times`.
pub const MAX_REPEAT: u64 = 100;
/// Maximum single `delay_ms`.
pub const MAX_DELAY_MS: u64 = 60_000;
/// Maximum nesting of `macro` / `repeat`.
pub const MAX_DEPTH: usize = 4;
/// Maximum serialized size accepted by [`parse_action`].
pub const MAX_ACTION_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActionError {
    #[error("action is not valid JSON: {0}")]
    Json(String),
    #[error("action is larger than {MAX_ACTION_BYTES} bytes")]
    TooLarge,
    #[error("action must be an object with exactly one key")]
    Shape,
    #[error("unknown action kind `{0}`")]
    UnknownKind(String),
    #[error("bad value for `{kind}`: {reason}")]
    BadValue { kind: String, reason: &'static str },
    #[error("actions nested deeper than {MAX_DEPTH}")]
    TooDeep,
}

/// Parse and validate an action from JSON text.
pub fn parse_action(text: &str) -> Result<Value, ActionError> {
    if text.len() > MAX_ACTION_BYTES {
        return Err(ActionError::TooLarge);
    }
    let v: Value = serde_json::from_str(text).map_err(|e| ActionError::Json(e.to_string()))?;
    validate_action(&v)?;
    Ok(v)
}

/// Validate an already-parsed action.
pub fn validate_action(v: &Value) -> Result<(), ActionError> {
    validate(v, 0)
}

fn bad(kind: &str, reason: &'static str) -> ActionError {
    ActionError::BadValue {
        kind: kind.to_string(),
        reason,
    }
}

fn validate(v: &Value, depth: usize) -> Result<(), ActionError> {
    if depth > MAX_DEPTH {
        return Err(ActionError::TooDeep);
    }
    let obj = v.as_object().ok_or(ActionError::Shape)?;
    if obj.len() != 1 {
        return Err(ActionError::Shape);
    }
    let (kind, val) = obj.iter().next().ok_or(ActionError::Shape)?;
    let k = kind.as_str();
    match k {
        "hotkey" | "launch" | "open" | "remote" => match val.as_str() {
            Some(s) if !s.trim().is_empty() => Ok(()),
            _ => Err(bad(k, "expected a non-empty string")),
        },
        "text" => val
            .as_str()
            .map(|_| ())
            .ok_or_else(|| bad(k, "expected a string")),
        "media" => match val {
            Value::String(s) if !s.is_empty() => Ok(()),
            Value::Object(m) if m.len() == 1 && m.get("seek_ms").is_some_and(Value::is_u64) => {
                Ok(())
            }
            _ => Err(bad(k, "expected an action name or {\"seek_ms\":n}")),
        },
        "volume_step" => match val.as_f64() {
            Some(f) if (-1.0..=1.0).contains(&f) => Ok(()),
            _ => Err(bad(k, "expected a number in -1..=1")),
        },
        "delay_ms" => match val.as_u64() {
            Some(n) if n <= MAX_DELAY_MS => Ok(()),
            _ => Err(bad(k, "expected an integer 0..=60000")),
        },
        "macro" => {
            let steps = val.as_array().ok_or_else(|| bad(k, "expected an array"))?;
            if steps.is_empty() || steps.len() > MAX_MACRO_STEPS {
                return Err(bad(k, "expected 1..=50 steps"));
            }
            steps.iter().try_for_each(|s| validate(s, depth + 1))
        }
        "repeat" => {
            let m = val
                .as_object()
                .ok_or_else(|| bad(k, "expected an object"))?;
            if m.len() != 2 {
                return Err(bad(k, "expected {\"times\":n,\"action\":…}"));
            }
            match m.get("times").and_then(Value::as_u64) {
                Some(n) if (1..=MAX_REPEAT).contains(&n) => {}
                _ => return Err(bad(k, "times must be 1..=100")),
            }
            let inner = m.get("action").ok_or_else(|| bad(k, "missing action"))?;
            validate(inner, depth + 1)
        }
        other => Err(ActionError::UnknownKind(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_protocol_examples() {
        for v in [
            json!({"hotkey":"ctrl+shift+esc"}),
            json!({"text":""}),
            json!({"launch":"Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"}),
            json!({"open":"https://example.com"}),
            json!({"media":"toggle_play_pause"}),
            json!({"media":{"seek_ms":1000}}),
            json!({"volume_step":-0.05}),
            json!({"delay_ms":250}),
            json!({"remote":"obs.scene:Main"}),
            json!({"macro":[{"text":"hi"},{"delay_ms":10},{"hotkey":"enter"}]}),
            json!({"repeat":{"times":3,"action":{"hotkey":"down"}}}),
        ] {
            validate_action(&v).unwrap_or_else(|e| panic!("{v}: {e}"));
        }
    }

    #[test]
    fn rejects_bad_actions() {
        assert_eq!(validate_action(&json!([])), Err(ActionError::Shape));
        assert_eq!(
            validate_action(&json!({"a":1,"b":2})),
            Err(ActionError::Shape)
        );
        assert!(matches!(
            validate_action(&json!({"rm":"-rf"})),
            Err(ActionError::UnknownKind(_))
        ));
        assert!(validate_action(&json!({"volume_step":3})).is_err());
        assert!(validate_action(&json!({"delay_ms":-1})).is_err());
        assert!(validate_action(&json!({"hotkey":" "})).is_err());
        let many: Vec<_> = (0..51).map(|_| json!({"delay_ms":1})).collect();
        assert!(validate_action(&json!({ "macro": many })).is_err());
        assert!(validate_action(&json!({"repeat":{"times":0,"action":{"text":"x"}}})).is_err());
        let mut deep = json!({"text":"x"});
        for _ in 0..6 {
            deep = json!({ "macro": [deep] });
        }
        assert_eq!(validate_action(&deep), Err(ActionError::TooDeep));
        assert!(matches!(parse_action("{"), Err(ActionError::Json(_))));
        let big = format!("{{\"text\":\"{}\"}}", "a".repeat(MAX_ACTION_BYTES));
        assert_eq!(parse_action(&big), Err(ActionError::TooLarge));
    }
}
