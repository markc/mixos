// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `comp.motion.run`: one Bus call carries a whole move (plan rule: one call,
//! one complex move). Phase 2 accepts one pointer target; window targets
//! follow in phase 3.
//!
//! ```text
//! comp.motion.run {targets: [{pointer: {
//!     output?: "name",
//!     path: [{x, y}, ...],        // 1..=64 output-local points
//!     duration_ms?: 0..=10000,    // default 400
//!     curve?: "linear" | "ease" | "ease-in" | "ease-out" | "ease-in-out"
//!           | "min-jerk" | [x1, y1, x2, y2]   // default "ease-in-out"
//! }}]}
//! ```
//!
//! The glide starts at `path[0]` (the pointer is placed there at once) and
//! visits the rest in order. The reply waits until the glide ends.

use std::time::Duration;

use serde_json::Value;

use motion::{CubicBezier, Easing};

use crate::reply::ControlReply;
use crate::request::{args_object, invalid_argument, present};

/// Longest glide a call may ask for (see the plan's VT decision).
pub const MAX_DURATION_MS: u64 = 10_000;
/// Default glide length when `duration_ms` is absent.
pub const DEFAULT_DURATION_MS: u64 = 400;
/// Most points one glide may visit.
pub const MAX_PATH_POINTS: usize = 64;
/// Slack the reply gets beyond the glide itself.
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
                MotionTarget::Pointer { duration, .. } => *duration,
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

fn parse_target(value: &Value) -> Result<MotionTarget, ControlReply> {
    let object = match value {
        Value::Object(object) if object.len() == 1 => object,
        _ => {
            return Err(invalid_argument(
                "targets[0]",
                "object with one key: pointer",
                "{pointer: {...}}",
            ));
        }
    };
    let Some(Value::Object(pointer)) = object.get("pointer") else {
        return Err(invalid_argument(
            "targets[0].pointer",
            "object",
            "{path, duration_ms?, curve?, output?}",
        ));
    };
    if pointer
        .keys()
        .any(|key| !POINTER_FIELDS.contains(&key.as_str()))
    {
        return Err(invalid_argument(
            "pointer",
            "only output, path, duration_ms and curve",
            "unknown field refused",
        ));
    }
    let fields = pointer;

    let output = match present(fields, "output") {
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

    let path = match present(fields, "path") {
        Some(Value::Array(points))
            if (1..=MAX_PATH_POINTS).contains(&points.len()) =>
        {
            points.iter().map(parse_point).collect::<Result<Vec<_>, _>>()?
        }
        _ => {
            return Err(invalid_argument(
                "pointer.path",
                "array of 1..=64 {x, y} points",
                "output-local logical coordinates",
            ));
        }
    };

    let duration_ms = match present(fields, "duration_ms") {
        None => DEFAULT_DURATION_MS,
        Some(value) => value
            .as_u64()
            .filter(|ms| *ms <= MAX_DURATION_MS)
            .ok_or_else(|| {
                invalid_argument("pointer.duration_ms", "integer 0..=10000", "milliseconds")
            })?,
    };

    let easing = match present(fields, "curve") {
        None => Easing::Bezier(CubicBezier::EASE_IN_OUT),
        Some(value) => parse_curve(value)?,
    };

    Ok(MotionTarget::Pointer {
        output,
        path,
        duration: Duration::from_millis(duration_ms),
        easing,
    })
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
            "pointer.curve",
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
    fn defaults_are_400ms_and_ease_in_out() {
        let spec = ok(json!({"targets": [{"pointer": {"path": [{"x": 1, "y": 2}]}}]}));
        let MotionTarget::Pointer {
            duration, easing, ..
        } = &spec.targets[0];
        assert_eq!(*duration, Duration::from_millis(DEFAULT_DURATION_MS));
        assert_eq!(*easing, Easing::Bezier(CubicBezier::EASE_IN_OUT));
    }

    #[test]
    fn custom_bezier_curve_is_accepted() {
        let spec = ok(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "curve": [0.05, 0.9, 0.1, 1.1]
        }}]}));
        let MotionTarget::Pointer { easing, .. } = &spec.targets[0];
        assert!(matches!(easing, Easing::Bezier(_)));
    }

    #[test]
    fn budget_is_the_glide_plus_margin() {
        let spec = ok(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "duration_ms": 2000
        }}]}));
        assert_eq!(spec.budget(), Duration::from_millis(3000));
    }

    #[test]
    fn refuses_bad_input() {
        let refused = |args: Value| parse(&args).is_err();
        // Two targets (phase 2 takes one).
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
        // Unknown field.
        assert!(refused(json!({"targets": [{"pointer": {
            "path": [{"x": 0, "y": 0}], "speed": 3
        }}]})));
    }
}
