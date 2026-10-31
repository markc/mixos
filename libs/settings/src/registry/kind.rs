// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Validation and normalisation of one value against its [`Kind`]. A value
//! that passes comes back in canonical form: whole numbers as integers,
//! colours in lower case.
use super::{Alpha, Grammar, Kind};
use crate::model::Diagnostic;
use serde_json::{Map, Value};

fn error(path: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("validation_failed", path, message)
}

/// The longest path a path-kind value may be.
pub const MAX_PATH_BYTES: usize = 4096;
/// The longest argv a spawn entry may have, and each argument.
pub const MAX_ARGV: usize = 32;
pub const MAX_ARG_BYTES: usize = 4096;

/// A whole number, as JSON or Mix data sends it (48 or 48.0).
pub fn whole(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    let n = value.as_f64()?;
    (n.is_finite() && n.fract() == 0.0 && n.abs() < 9.0e15).then_some(n as i64)
}

/// `#rrggbb`, or `#rrggbbaa` where alpha is allowed, in lower case.
pub fn colour(text: &str, alpha: Alpha) -> Option<String> {
    let hex = text.strip_prefix('#')?;
    let fits = hex.len() == 6 || (alpha == Alpha::Any && hex.len() == 8);
    (fits && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| text.to_ascii_lowercase())
}

fn text<'a>(path: &str, value: &'a Value) -> Result<&'a str, Diagnostic> {
    value
        .as_str()
        .ok_or_else(|| error(path, "Expected a string"))
}

pub fn check(kind: &Kind, path: &str, value: &Value) -> Result<Value, Diagnostic> {
    match *kind {
        Kind::Bool => value
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| error(path, "Expected true or false")),
        Kind::Int { min, max, step } => {
            let n = whole(value).ok_or_else(|| error(path, "Expected a whole number"))?;
            if !(min..=max).contains(&n) || (n - min) % step != 0 {
                let grid = if step > 1 {
                    format!(" in steps of {step}")
                } else {
                    String::new()
                };
                return Err(error(path, format!("Expected {min}..{max}{grid}")));
            }
            Ok(n.into())
        }
        Kind::OneOf(set) => {
            let n = whole(value).ok_or_else(|| error(path, "Expected a whole number"))?;
            if set.contains(&n) {
                Ok(n.into())
            } else {
                Err(error(path, format!("Expected one of {set:?}")))
            }
        }
        Kind::Number { min, max, step } => {
            let n = value
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| error(path, "Expected a number"))?;
            if !(min..=max).contains(&n) {
                return Err(error(path, format!("Expected {min}..{max}")));
            }
            if step > 0.0 {
                let steps = ((n - min) / step).round();
                if (min + steps * step - n).abs() > 1e-9 {
                    return Err(error(path, format!("Expected a multiple of {step}")));
                }
            }
            Ok(n.into())
        }
        Kind::Enum(names) => {
            let name = text(path, value)?;
            if names.contains(&name) {
                Ok(name.into())
            } else {
                Err(error(path, format!("Expected one of {}", names.join(", "))))
            }
        }
        Kind::Known(_) => {
            // Unknown names are retained for profiles shared with newer
            // binaries; the shape is the design's plain-name grammar.
            let name = text(path, value)?;
            if design::WindowStyle::valid_name(name) {
                Ok(name.into())
            } else {
                Err(error(
                    path,
                    "Use a plain name of up to 64 bytes: lowercase letters, digits or -",
                ))
            }
        }
        Kind::Colour(alpha) => colour(text(path, value)?, alpha)
            .map(Value::String)
            .ok_or_else(|| error(path, colour_message(alpha))),
        Kind::ColourOr(words, alpha) => {
            let given = text(path, value)?;
            if words.contains(&given) {
                return Ok(given.into());
            }
            colour(given, alpha).map(Value::String).ok_or_else(|| {
                error(
                    path,
                    format!("Expected {} or {}", words.join(", "), colour_message(alpha)),
                )
            })
        }
        Kind::ConnectorOr(words) => {
            let given = text(path, value)?;
            if words.contains(&given) || super::connector(given) {
                Ok(given.into())
            } else {
                Err(error(path, "Expected a connector name"))
            }
        }
        Kind::Str(grammar, max_bytes) => {
            let given = text(path, value)?;
            if given.len() > max_bytes {
                return Err(error(path, format!("At most {max_bytes} bytes")));
            }
            string(grammar, given)
                .ok_or_else(|| error(path, format!("Expected a {}", grammar.name())))
        }
        Kind::Path { directory, images } => {
            let given = text(path, value)?;
            if !given.starts_with('/')
                || given.len() > MAX_PATH_BYTES
                || given.chars().any(char::is_control)
            {
                return Err(error(path, "Expected an absolute path"));
            }
            if images && !directory && !image(given) {
                return Err(error(path, "Expected a PNG, JPEG or WebP image"));
            }
            Ok(given.into())
        }
        Kind::List(of, min, max) => {
            let items = value
                .as_array()
                .ok_or_else(|| error(path, "Expected a list"))?;
            if !(min..=max).contains(&items.len()) {
                return Err(error(path, format!("Expected {min}..{max} items")));
            }
            items
                .iter()
                .map(|item| check(of, path, item))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        Kind::XkbLayout => {
            let pair = value
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or_else(|| error(path, "Expected [layout, variant]"))?;
            let (layout, variant) = (text(path, &pair[0])?, text(path, &pair[1])?);
            let name = |s: &str, empty: bool| {
                (empty || !s.is_empty())
                    && s.len() <= 64
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            };
            if name(layout, false) && name(variant, true) {
                Ok(value.clone())
            } else {
                Err(error(path, "Expected an xkb layout and variant"))
            }
        }
        Kind::Chords => chords(path, value),
        Kind::Spawn => spawn(path, value),
        // The domain module checks the whole domain after the write.
        Kind::AppDomain => Ok(value.clone()),
        Kind::Tunable => tunable(path, value),
        Kind::Catalogue(_) => Err(Diagnostic::new("live_key", path, "Live host state")),
    }
}

fn colour_message(alpha: Alpha) -> &'static str {
    match alpha {
        Alpha::Opaque => "a #rrggbb colour",
        Alpha::Any => "a #rrggbb or #rrggbbaa colour",
    }
}

fn image(path: &str) -> bool {
    let Some((_, ext)) = path.rsplit_once('.') else {
        return false;
    };
    ["png", "jpg", "jpeg", "webp"]
        .iter()
        .any(|e| ext.eq_ignore_ascii_case(e))
}

fn digits(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())
}

/// The canonical string for `grammar`, or None when `given` is not one.
fn string(grammar: Grammar, given: &str) -> Option<Value> {
    let ok = match grammar {
        Grammar::Plain => crate::terminal::valid_family(given),
        Grammar::DirName => {
            !given.is_empty()
                && !given.starts_with('.')
                && given
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
        }
        Grammar::Connector => super::connector(given),
        Grammar::OutputMode => given.split_once('@').is_some_and(|(size, rate)| {
            size.split_once('x')
                .is_some_and(|(w, h)| digits(w, 5) && digits(h, 5))
                && rate
                    .split_once('.')
                    .is_some_and(|(i, f)| digits(i, 3) && f.len() == 3 && digits(f, 3))
        }),
        Grammar::Position => given.split_once(',').is_some_and(|(x, y)| {
            [x, y]
                .iter()
                .all(|n| digits(n.strip_prefix('-').unwrap_or(n), 6))
        }),
        Grammar::Zone => {
            !given.is_empty()
                && !given.starts_with('/')
                && !given.ends_with('/')
                && !given.contains("..")
                && !given.contains("//")
                && given
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
        }
        Grammar::Locale => {
            given.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
                && given
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.@-".contains(&b))
        }
        Grammar::DesktopId => given.strip_suffix(".desktop").is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        }),
        Grammar::Applet => crate::keymap::valid_action(given),
        Grammar::Source => true,
        Grammar::WallpaperColour => return wallpaper_colour(given).map(Value::String),
    };
    ok.then(|| given.into())
}

/// `#rrggbb`, or `gradient:#a,#b@<deg>` with the angle 0–360, lower-cased.
fn wallpaper_colour(given: &str) -> Option<String> {
    let Some(rest) = given.strip_prefix("gradient:") else {
        return colour(given, Alpha::Opaque);
    };
    let (stops, angle) = rest.split_once('@')?;
    let (a, b) = stops.split_once(',')?;
    let (a, b) = (colour(a, Alpha::Opaque)?, colour(b, Alpha::Opaque)?);
    let degrees: u32 = angle.parse().ok().filter(|_| digits(angle, 3))?;
    (degrees <= 360).then(|| format!("gradient:{a},{b}@{degrees}"))
}

fn chords(path: &str, value: &Value) -> Result<Value, Diagnostic> {
    use crate::keymap::{MAX_CHORDS, valid_chord};
    let list = value
        .as_array()
        .ok_or_else(|| error(path, "Expected a list of chords"))?;
    if list.len() > MAX_CHORDS {
        return Err(error(path, format!("At most {MAX_CHORDS} chords")));
    }
    let mut seen: Vec<&str> = Vec::new();
    for chord in list {
        let chord = text(path, chord)?;
        if !valid_chord(chord) {
            return Err(error(
                path,
                format!("{chord:?} is not a chord (Ctrl, Alt, Shift, Super, then one key)"),
            ));
        }
        if seen.contains(&chord) {
            return Err(error(path, format!("{chord:?} is listed twice")));
        }
        seen.push(chord);
    }
    Ok(value.clone())
}

fn spawn(path: &str, value: &Value) -> Result<Value, Diagnostic> {
    let entry = value
        .as_object()
        .ok_or_else(|| error(path, "Expected {command, bindings}"))?;
    if let Some(unknown) = entry
        .keys()
        .find(|k| !["command", "bindings"].contains(&k.as_str()))
    {
        return Err(error(path, format!("Unknown field {unknown:?}")));
    }
    let argv = entry
        .get("command")
        .and_then(Value::as_array)
        .filter(|argv| (1..=MAX_ARGV).contains(&argv.len()))
        .ok_or_else(|| error(path, format!("command must be 1..{MAX_ARGV} arguments")))?;
    for arg in argv {
        let arg = text(path, arg)?;
        if arg.is_empty() || arg.len() > MAX_ARG_BYTES || arg.contains('\0') {
            return Err(error(path, "Each argument must be 1..4096 bytes, no NUL"));
        }
    }
    let bindings = chords(
        path,
        entry.get("bindings").unwrap_or(&Value::Array(Vec::new())),
    )?;
    let mut normal = Map::new();
    normal.insert("command".into(), Value::Array(argv.clone()));
    normal.insert("bindings".into(), bindings);
    Ok(Value::Object(normal))
}

/// A `tune.*` value against its tunable, with ADR R4's codes.
fn tunable(path: &str, value: &Value) -> Result<Value, Diagnostic> {
    let name = path.strip_prefix("tune.").unwrap_or(path);
    let entry = super::tunable(name)
        .ok_or_else(|| Diagnostic::new("unknown_tunable", path, "Unknown tunable"))?;
    if let Some(retired) = entry.retired {
        return Err(Diagnostic::new(
            "unknown_tunable",
            path,
            format!("Retired: {retired}"),
        ));
    }
    let wrong = |what: &str| Diagnostic::new("wrong_kind", path, format!("Expected {what}"));
    let range = |n: f64| match &entry.range {
        Some(design::TunableRange::Number { min, max, .. }) if !(*min..=*max).contains(&n) => Err(
            Diagnostic::new("out_of_range", path, format!("Expected {min}..{max}")),
        ),
        _ => Ok(()),
    };
    match entry.kind {
        "flag" => value.as_bool().map(Value::Bool).ok_or_else(|| wrong("true or false")),
        "radius" => {
            let n = whole(value).ok_or_else(|| wrong("a whole number"))?;
            range(n as f64)?;
            Ok(n.into())
        }
        "number" => {
            let n = value
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| wrong("a number"))?;
            range(n)?;
            Ok(n.into())
        }
        "colour" => value
            .as_str()
            .and_then(|s| colour(s, Alpha::Any))
            .map(Value::String)
            .ok_or_else(|| wrong("a #rrggbb or #rrggbbaa colour")),
        "choice" => {
            let given = value.as_str().ok_or_else(|| wrong("a name"))?;
            match &entry.range {
                Some(design::TunableRange::Choice { variants })
                    if variants.iter().any(|v| v == given) =>
                {
                    Ok(given.into())
                }
                _ => Err(Diagnostic::new(
                    "unknown_choice",
                    path,
                    format!("{given:?} is not a choice of {name}"),
                )),
            }
        }
        other => Err(wrong(other)),
    }
}
