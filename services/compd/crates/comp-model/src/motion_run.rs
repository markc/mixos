// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `comp.motion.run`: one Bus call carries a whole move (plan rule: one call,
//! one complex move). Phase 3 accepts exactly one target, a pointer or a
//! window.
//!
//! ```text
//! comp.motion.run {targets: [{pointer: {
//!     output?: "name",
//!     path: [{x, y}, ...],        // 1..=64 output-local points
//!     duration_ms?: 0..=10000,    // default 400
//!     curve?: "linear" | "ease" | "ease-in" | "ease-out" | "ease-in-out"
//!           | "min-jerk" | [x1, y1, x2, y2]   // default "ease-in-out"
//! }}]}
//! comp.motion.run {targets: [{window: {
//!     id, generation,
//!     to: {x?, y?, w?, h?},       // global logical; absent fields stay put
//!     duration_ms?, curve?        // as for pointer
//! }}]}
//! ```
//!
//! A pointer glide starts at `path[0]` (the pointer is placed there at once)
//! and visits the rest in order. A window travels from where it is to `to`.
//! The reply waits until the motion ends.

use std::time::Duration;

use serde_json::Value;

use motion::{CubicBezier, Easing};

use crate::reply::ControlReply;
use crate::request::{args_object, invalid_argument, present};

/// Longest motion a call may ask for (see the plan's VT decision).
pub const MAX_DURATION_MS: u64 = 10_000;
/// Default motion length when `duration_ms` is absent.
pub const DEFAULT_DURATION_MS: u64 = 400;
/// Most points one pointer glide may visit.
pub const MAX_PATH_POINTS: usize = 64;
/// Slack the reply gets beyond the motion itself.
const REPLY_MARGIN: Duration = Duration::from_secs(1);

/// One target of `comp.motion.run`.
#[derive(Clone, Debug, PartialEq)]
pub enum MotionTarget {
    Pointer {
        output: Option<String>,
        path: Vec<(f64, f64)>,
        duration: Duration,
        easing: Easing,
    },
    Window {
        id: u64,
        generation: u64,
        /// Target `x, y, w, h` in global logical coordinates; `None` keeps
        /// the window's current value for that field.
        to: [Option<f64>; 4],
        duration: Duration,
        easing: Easing,
    },
}

/// A parsed `comp.motion.run`.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionSpec {
    pub targets: Vec<MotionTarget>,
}

impl MotionSpec {
    /// The longest any target runs, plus the reply margin.
    pub fn budget(&self) -> Duration {
        self.targets
            .iter()
            .map(|target| match target {
                MotionTarget::Pointer { duration, .. } | MotionTarget::Window { duration, .. } => {
                    *duration
                }
            })
            .max()
            .unwrap_or_default()
            + REPLY_MARGIN
    }
}

/// Parse the verb's arguments.
pub fn parse(args: &Value) -> Result<MotionSpec, ControlReply> {
    let empty = serde_json::Map::new();
    let object = args_object(args, &empty, &["targets"])?;
    let targets = match present(object, "targets") {
        Some(Value::Array(items)) if items.len() == 1 => items,
        _ => {
            return Err(invalid_argument(
                "targets",
                "array of exactly one target",
                "one target per call in this phase",
            ));
        }
    };
    let target = parse_target(&targets[0])?;
    Ok(MotionSpec {
        targets: vec![target],
    })
}

const POINTER_FIELDS: &[&str] = &["output", "path", "duration_ms", "curve"];
const WINDOW_FIELDS: &[&str] = &["id", "generation", "to", "duration_ms", "curve"];
const TO_FIELDS: &[&str] = &["x", "y", "w", "h"];

fn parse_target(value: &Value) -> Result<MotionTarget, ControlReply> {
    let object = match value {
        Value::Object(object) if object.len() == 1 => object,
        _ => {
            return Err(invalid_argument(
                "targets[0]",
                "object with one key: pointer or window",
                "{pointer: {...}} or {window: {...}}",
            ));
        }
    };
    match object.get("pointer") {
        Some(Value::Object(pointer)) => parse_pointer(pointer),
        Some(_) => Err(invalid_argument(
            "targets[0].pointer",
            "object",
            "{path, duration_ms?, curve?, output?}",
        )),
        None => match object.get("window") {
            Some(Value::Object(window)) => parse_window(window),
            _ => Err(invalid_argument(
                "targets[0]",
                "object with one key: pointer or window",
                "{pointer: {...}} or {window: {...}}",
            )),
        },
    }
}

fn parse_pointer(pointer: &serde_json::Map<String, Value>) -> Result<MotionTarget, ControlReply> {
    if pointer.keys().any(|key| !POINTER_FIELDS.contains(&key.as_str())) {
        return Err(invalid_argument(
            "pointer",
            "only output, path, duration_ms and curve",
            "unknown field refused",
        ));
    }
    let output = match present(pointer, "output") {
        None => None,
        Some(Value::String(name)) if !name.is_empty() => Some(name.clone()),
        Some(_) => {
            return Err(invalid_argument(
                "pointer.output",
                "non-empty string",
                "output name",
            ));
        }
    };
    let path = match present(pointer, "path") {
        Some(Value::Array(points)) if (1..=MAX_PATH_POINTS).contains(&points.len()) => points
            .iter()
            .map(parse_point)
            .collect::<Result<Vec<_>, _>>()?,
        _ => {
            return Err(invalid_argument(
                "pointer.path",
                "array of 1..=64 {x, y} points",
                "output-local logical coordinates",
            ));
        }
    };
    let (duration, easing) = parse_timing(pointer)?;
    Ok(MotionTarget::Pointer {
        output,
        path,
        duration,
        easing,
    })
}

fn parse_window(window: &serde_json::Map<String, Value>) -> Result<MotionTarget, ControlReply> {
    if window.keys().any(|key| !WINDOW_FIELDS.contains(&key.as_str())) {
        return Err(invalid_argument(
            "window",
            "only id, generation, to, duration_ms and curve",
            "unknown field refused",
        ));
    }
    let id = window
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_argument("window.id", "integer", "window id"))?;
    let generation = window
        .get("generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_argument("window.generation", "integer", "window generation"))?;
    let to_object = match present(window, "to") {
        Some(Value::Object(to)) => to,
        _ => {
            return Err(invalid_argument(
                "window.to",
                "object with any of x, y, w, h",
                "global logical coordinates",
            ));
        }
    };
    if to_object.keys().any(|key| !TO_FIELDS.contains(&key.as_str())) {
        return Err(invalid_argument(
            "window.to",
            "only x, y, w and h",
            "unknown field refused",
        ));
    }
    let mut to = [None; 4];
    for (slot, key) in to.iter_mut().zip(TO_FIELDS) {
        *slot = match to_object.get(*key) {
            None => None,
            Some(value) => Some(
                value
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| invalid_argument("window.to", "finite number", "logical px"))?,
            ),
        };
    }
    if to.iter().all(Option::is_none) {
        return Err(invalid_argument(
            "window.to",
            "at least one of x, y, w, h",
            "global logical coordinates",
        ));
    }
    if to[2].is_some_and(|w| w <= 0.0) || to[3].is_some_and(|h| h <= 0.0) {
        return Err(invalid_argument(
            "window.to",
            "w and h above zero",
            "logical px",
        ));
    }
    let (duration, easing) = parse_timing(window)?;
    Ok(MotionTarget::Window {
        id,
        generation,
        to,
        duration,
        easing,
    })
}

/// `duration_ms` and `curve`, shared by both target kinds.
fn parse_timing(
    fields: &serde_json::Map<String, Value>,
) -> Result<(Duration, Easing), ControlReply> {
    let duration_ms = match present(fields, "duration_ms") {
        None => DEFAULT_DURATION_MS,
        Some(value) => value
            .as_u64()
            .filter(|ms| *ms <= MAX_DURATION_MS)
            .ok_or_else(|| {
                invalid_argument("duration_ms", "integer 0..=10000", "milliseconds")
            })?,
    };
    let easing = match present(fields, "curve") {
        None => Easing::Bezier(CubicBezier::EASE_IN_OUT),
        Some(value) => parse_curve(value)?,
    };
    Ok((Duration::from_millis(duration_ms), easing))
}

fn parse_point(value: &Value) -> Result<(f64, f64), ControlReply> {
    let bad = || {
        invalid_argument(
            "pointer.path",
            "{x, y} with finite numbers",
            "output-local logical coordinates",
        )
    };
    let Value::Object(point) = value else {
        return Err(bad());
    };
    let x = point.get("x").and_then(Value::as_f64).filter(|v| v.is_finite());
    let y = point.get("y").and_then(Value::as_f64).filter(|v| v.is_finite());
    match (x, y) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(bad()),
    }
}

fn parse_curve(value: &Value) -> Result<Easing, ControlReply> {
    let bad = || {
        invalid_argument(
            "curve",
            "a curve name or [x1, y1, x2, y2]",
            "linear | ease | ease-in | ease-out | ease-in-out | min-jerk",
        )
    };
    match value {
        Value::String(name) => match name.as_str() {
            "linear" => Ok(Easing::Linear),
            "min-jerk" => Ok(Easing::MinJerk),
            "ease" => Ok(Easing::Bezier(CubicBezier::EASE)),
            "ease-in" => Ok(Easing::Bezier(CubicBezier::EASE_IN)),
            "ease-out" => Ok(Easing::Bezier(CubicBezier::EASE_OUT)),
            "ease-in-out" => Ok(Easing::Bezier(CubicBezier::EASE_IN_OUT)),
            _ => Err(bad()),
        },
        Value::Array(numbers) if numbers.len() == 4 => {
            let n: Vec<f64> = numbers
                .iter()
                .map(|v| v.as_f64().ok_or_else(bad))
                .collect::<Result<_, _>>()?;
            CubicBezier::new(n[0], n[1], n[2], n[3])
                .map(Easing::Bezier)
                .map_err(|_| bad())
        }
        _ => Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(args: Value) -> MotionSpec {
        parse(&args).unwrap_or_else(|_| panic!("should parse: {args}"))
    }

    #[test]
    fn parses_a_full_pointer_glide() {
        let spec = ok(json!({"targets": [{"pointer": {
            "output": "o1",
            "path": [{"x": 10, "y": 20}, {"x": 300, "y": 40}],
            "duration_ms": 750,
            "curve": "min-jerk"
        }}]}));
        assert_eq!(
            spec.targets,
            vec![MotionTarget::Pointer {
                output: Some("o1".into()),
                path: vec![(10.0, 20.0), (300.0, 40.0)],
                duration: Duration::from_millis(750),
                easing: Easing::MinJerk,
            }]
        );
    }

    #[test]
    fn parses_a_window_move_and_resize() {
        let spec = ok(json!({"targets": [{"window": {
            "id": 7, "generation": 3,
            "to": {"x": 100, "w": 640.5, "h": 480},
            "duration_ms": 900, "curve": "ease-out"
        }}]}));
        assert_eq!(
            spec.targets,
            vec![MotionTarget::Window {
                id: 7,
                generation: 3,
                to: [Some(100.0), None, Some(640.5), Some(480.0)],
                duration: Duration::from_millis(900),
                easing: Easing::Bezier(CubicBezier::EASE_OUT),
            }]
        );
    }

    #[test]
    fn defaults_are_400ms_and_ease_in_out() {
        let spec = ok(json!({"targets": [{"pointer": {"path": [{"x": 1, "y": 2}]}}]}));
        let MotionTarget::Pointer {
            duration, easing, ..
        } = &spec.targets[0]
        else {
            panic!("pointer target");
        };
        assert_eq!(*duration, Duration::from_millis(DEFAULT_DURATION_MS));
        assert_eq!(*easing, Easing::Bezier(CubicBezier::EASE_IN_OUT));
    }

    #[test]
    fn custom_bezier_curve_is_accepted() {
        let spec = ok(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "curve": [0.05, 0.9, 0.1, 1.1]
        }}]}));
        let MotionTarget::Pointer { easing, .. } = &spec.targets[0] else {
            panic!("pointer target");
        };
        assert!(matches!(easing, Easing::Bezier(_)));
    }

    #[test]
    fn budget_is_the_motion_plus_margin() {
        let spec = ok(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "duration_ms": 2000
        }}]}));
        assert_eq!(spec.budget(), Duration::from_millis(3000));
    }

    #[test]
    fn refuses_bad_input() {
        let refused = |args: Value| parse(&args).is_err();
        // Two targets (phase 3 takes one).
        assert!(refused(json!({"targets": [
            {"pointer": {"path": [{"x": 0, "y": 0}]}},
            {"pointer": {"path": [{"x": 0, "y": 0}]}}
        ]})));
        // Empty path.
        assert!(refused(json!({"targets": [{"pointer": {"path": []}}]})));
        // Over the duration cap.
        assert!(refused(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "duration_ms": 10001
        }}]})));
        // Unknown curve name.
        assert!(refused(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "curve": "bouncy"
        }}]})));
        // Bezier x out of range.
        assert!(refused(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "curve": [-0.1, 0, 0.5, 1]
        }}]})));
        // Non-finite point.
        assert!(refused(json!({"targets": [{"pointer": {"path": [{"x": "a", "y": 0}]}}]})));
        // Unknown pointer field.
        assert!(refused(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "speed": 3
        }}]})));
        // Window: missing id, empty `to`, zero width, unknown `to` field.
        assert!(refused(json!({"targets": [{"window": {
            "generation": 1, "to": {"x": 1}
        }}]})));
        assert!(refused(json!({"targets": [{"window": {
            "id": 1, "generation": 1, "to": {}
        }}]})));
        assert!(refused(json!({"targets": [{"window": {
            "id": 1, "generation": 1, "to": {"w": 0}
        }}]})));
        assert!(refused(json!({"targets": [{"window": {
            "id": 1, "generation": 1, "to": {"z": 1}
        }}]})));
    }
}
