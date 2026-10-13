// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The thumbnailers installed on the system: freedesktop `.thumbnailer`
//! entries (the contract GNOME, COSMIC and glycin share), found in
//! `thumbnailers/` under `$XDG_DATA_HOME` then each of `$XDG_DATA_DIRS`.
//!
//! ```text
//! [Thumbnailer Entry]
//! TryExec=/usr/bin/glycin-thumbnailer
//! Exec=/usr/bin/glycin-thumbnailer --input %u --output %o --size %s
//! MimeType=image/png;image/jpeg;
//! ```
//!
//! Every thumbnail this library makes comes from one of these programs:
//! they cover every format the system knows, and keep decoders MixOS cannot
//! ship (FFmpeg-backed video, for one) in their own process. An `Exec` line
//! becomes an argv, never a shell command, and each placeholder fills
//! exactly one argument.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// One thumbnailer: its argv template and the types it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnailer {
    /// The `.thumbnailer` file it came from.
    pub source: PathBuf,
    /// `Exec`, split into arguments (placeholders still in them).
    pub exec: Vec<String>,
    pub mime_types: Vec<String>,
}

/// What fills an `Exec` template's placeholders.
pub struct Fill<'a> {
    /// `%i`: the input file's path.
    pub input: &'a Path,
    /// `%u`: the input file's URI.
    pub uri: &'a str,
    /// `%o`: where to write the PNG.
    pub output: &'a Path,
    /// `%s`: the largest side wanted, in px.
    pub size: u32,
}

impl Thumbnailer {
    /// The argv to run for `fill`. `None` when the template is empty, or a
    /// placeholder is unknown or names no input (an entry with neither `%i`
    /// nor `%u`, or no `%o`, cannot be run safely).
    pub fn argv(&self, fill: &Fill<'_>) -> Option<Vec<OsString>> {
        use std::os::unix::ffi::OsStrExt;
        let mut out: Vec<OsString> = Vec::with_capacity(self.exec.len());
        let (mut input, mut output) = (false, false);
        for arg in &self.exec {
            let mut built: Vec<u8> = Vec::new();
            let mut chars = arg.chars();
            while let Some(c) = chars.next() {
                if c != '%' {
                    let mut buf = [0u8; 4];
                    built.extend(c.encode_utf8(&mut buf).as_bytes());
                    continue;
                }
                match chars.next()? {
                    'i' => {
                        built.extend(fill.input.as_os_str().as_bytes());
                        input = true;
                    }
                    'u' => {
                        built.extend(fill.uri.as_bytes());
                        input = true;
                    }
                    'o' => {
                        built.extend(fill.output.as_os_str().as_bytes());
                        output = true;
                    }
                    's' => built.extend(fill.size.to_string().as_bytes()),
                    '%' => built.push(b'%'),
                    _ => return None,
                }
            }
            out.push(OsString::from(std::ffi::OsStr::from_bytes(&built)));
        }
        (!out.is_empty() && input && output).then_some(out)
    }
}

/// The installed thumbnailers, by MIME type: for each type the first entry
/// found (search order, then file name).
#[derive(Clone, Debug, Default)]
pub struct Registry {
    by_mime: HashMap<String, Thumbnailer>,
}

impl Registry {
    /// Load from the XDG data directories, keeping entries whose `TryExec`
    /// (or `Exec`'s program) can run.
    pub fn load() -> Registry {
        Registry::load_from(&data_dirs(), &|program| runnable(program))
    }

    /// Load from `dirs` (each searched for `thumbnailers/*.thumbnailer`),
    /// with `can_run` deciding whether an entry's program is available.
    pub fn load_from(dirs: &[PathBuf], can_run: &dyn Fn(&str) -> bool) -> Registry {
        let mut by_mime: HashMap<String, Thumbnailer> = HashMap::new();
        for dir in dirs {
            let Ok(read) = std::fs::read_dir(dir.join("thumbnailers")) else {
                continue;
            };
            let mut files: Vec<PathBuf> = read
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "thumbnailer"))
                .collect();
            files.sort();
            for file in files {
                let Ok(text) = std::fs::read_to_string(&file) else {
                    continue;
                };
                let Some((thumbnailer, try_exec)) = parse(&file, &text) else {
                    continue;
                };
                let program = try_exec.unwrap_or_else(|| thumbnailer.exec[0].clone());
                if !can_run(&program) {
                    continue;
                }
                for mime in &thumbnailer.mime_types {
                    by_mime
                        .entry(mime.clone())
                        .or_insert_with(|| thumbnailer.clone());
                }
            }
        }
        Registry { by_mime }
    }

    pub fn for_mime(&self, mime: &str) -> Option<&Thumbnailer> {
        self.by_mime.get(mime)
    }

    pub fn is_empty(&self) -> bool {
        self.by_mime.is_empty()
    }
}

/// Parse one `.thumbnailer` file: the entry and its `TryExec`, or `None`
/// when it has no usable `Exec` or no types.
pub fn parse(source: &Path, text: &str) -> Option<(Thumbnailer, Option<String>)> {
    let mut in_entry = false;
    let (mut exec, mut try_exec, mut mime) = (None, None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_entry = line == "[Thumbnailer Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // Exec and TryExec are strings: their general escapes are undone
        // first, then Exec's own quoting splits it.
        match key.trim() {
            "Exec" => exec = Some(unescape(value.trim())),
            "TryExec" => try_exec = Some(unescape(value.trim())),
            "MimeType" => mime = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    let exec = split_exec(&exec?)?;
    // The program itself is never a placeholder or built from one.
    if exec.first()?.contains('%') {
        return None;
    }
    let mime_types: Vec<String> = mime?
        .split(';')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned)
        .collect();
    if exec.is_empty() || mime_types.is_empty() {
        return None;
    }
    let thumbnailer = Thumbnailer {
        source: source.to_owned(),
        exec,
        mime_types,
    };
    // A template that cannot be filled (no input, no output, an unknown
    // placeholder) is refused here, so it never claims a type and hides a
    // working entry further down the search path.
    thumbnailer.argv(&Fill {
        input: Path::new("/input"),
        uri: "file:///input",
        output: Path::new("/output"),
        size: 128,
    })?;
    Some((thumbnailer, try_exec.filter(|t| !t.is_empty())))
}

/// Undo a Desktop Entry string value's general escapes: `\s` space, `\n`
/// newline, `\t` tab, `\r` carriage return, `\\` backslash. Any other
/// backslash stays as it is (Exec's quoting rules read it next).
pub fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split an `Exec` value into arguments by the Desktop Entry rules: spaces
/// separate, double quotes group, and inside quotes a backslash escapes
/// `"`, `` ` ``, `$` and `\`. `None` for an unterminated quote.
pub fn split_exec(value: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            e @ ('"' | '`' | '$' | '\\') => current.push(e),
                            other => {
                                current.push('\\');
                                current.push(other);
                            }
                        },
                        other => current.push(other),
                    }
                }
            }
            other => {
                started = true;
                current.push(other);
            }
        }
    }
    if started {
        out.push(current);
    }
    Some(out)
}

/// `$XDG_DATA_HOME` (default `~/.local/share`) then `$XDG_DATA_DIRS`
/// (default `/usr/local/share:/usr/share`).
pub fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        Some(p) if p.is_absolute() => dirs.push(p),
        _ => dirs.extend(home.map(|h| h.join(".local/share"))),
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(
        system
            .split(':')
            .map(PathBuf::from)
            .filter(|p| p.is_absolute()),
    );
    dirs
}

/// Whether `program` (absolute, or a name on `$PATH`) is an executable file.
fn runnable(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    let path = Path::new(program);
    if path.is_absolute() {
        return executable(path);
    }
    if program.contains('/') {
        return false;
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| executable(&dir.join(program)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLYCIN: &str = "[Thumbnailer Entry]\nTryExec=/usr/bin/glycin-thumbnailer\n\
        Exec=/usr/bin/glycin-thumbnailer --input %u --output %o --size %s\n\
        MimeType=image/png;image/jpeg;\n";

    #[test]
    fn an_entry_parses_and_fills_one_argument_per_placeholder() {
        let (t, try_exec) = parse(Path::new("/x/glycin.thumbnailer"), GLYCIN).unwrap();
        assert_eq!(try_exec.as_deref(), Some("/usr/bin/glycin-thumbnailer"));
        assert_eq!(t.mime_types, ["image/png", "image/jpeg"]);
        let input = Path::new("/home/user/-rf my \"file\".png");
        let argv = t
            .argv(&Fill {
                input,
                uri: "file:///home/user/-rf%20my%20%22file%22.png",
                output: Path::new("/run/user/1000/t.png"),
                size: 256,
            })
            .unwrap();
        assert_eq!(
            argv,
            [
                "/usr/bin/glycin-thumbnailer",
                "--input",
                "file:///home/user/-rf%20my%20%22file%22.png",
                "--output",
                "/run/user/1000/t.png",
                "--size",
                "256"
            ]
            .map(OsString::from)
        );
        // %i keeps a hostile name whole: one argument, nothing split.
        let ff = Thumbnailer {
            source: PathBuf::new(),
            exec: split_exec("ffmpegthumbnailer -i %i -o %o -s %s -f").unwrap(),
            mime_types: vec!["video/mp4".into()],
        };
        let argv = ff
            .argv(&Fill {
                input,
                uri: "",
                output: Path::new("/o.png"),
                size: 128,
            })
            .unwrap();
        assert_eq!(argv[2], OsString::from("/home/user/-rf my \"file\".png"));
        assert_eq!(argv.len(), 8);
    }

    #[test]
    fn unusable_entries_are_refused() {
        assert!(parse(Path::new("a"), "[Thumbnailer Entry]\nExec=x %i %o\n").is_none());
        assert!(parse(Path::new("a"), "[Other]\nExec=x %i %o\nMimeType=a/b;\n").is_none());
        let no_output = Thumbnailer {
            source: PathBuf::new(),
            exec: vec!["x".into(), "%i".into()],
            mime_types: vec![],
        };
        let fill = Fill {
            input: Path::new("/a"),
            uri: "file:///a",
            output: Path::new("/o"),
            size: 1,
        };
        assert!(no_output.argv(&fill).is_none());
        let unknown = Thumbnailer {
            exec: vec!["x".into(), "%i".into(), "%o".into(), "%k".into()],
            ..no_output
        };
        assert!(unknown.argv(&fill).is_none());
    }

    #[test]
    fn exec_splits_by_the_desktop_entry_rules() {
        assert_eq!(
            split_exec(r#"a "b c" "d\"e" f\g "#).unwrap(),
            ["a", "b c", "d\"e", "f\\g"]
        );
        assert!(split_exec("a \"open").is_none());
    }

    /// The string escapes are undone before Exec's quoting: four
    /// backslashes in a quoted argument of the file become one, and `\s`
    /// outside quotes separates.
    #[test]
    fn string_escapes_come_before_exec_quoting() {
        let text = "[Thumbnailer Entry]\n\
            Exec=tool \"a\\\\\\\\b\" c\\sd %i %o\n\
            MimeType=image/png;\n";
        let (t, _) = parse(Path::new("a"), text).unwrap();
        assert_eq!(t.exec, ["tool", "a\\b", "c", "d", "%i", "%o"]);
        assert_eq!(unescape(r"x\ty\\z\q"), "x\ty\\z\\q");
    }

    /// The program is never a placeholder: `%i` there would run the file.
    #[test]
    fn a_placeholder_program_is_refused() {
        for exec in ["%i %o", "/usr/bin/%u %i %o", "\"%o\" %i"] {
            let text = format!("[Thumbnailer Entry]\nExec={exec}\nMimeType=image/png;\n");
            assert!(parse(Path::new("a"), &text).is_none(), "{exec}");
        }
    }

    #[test]
    fn the_first_runnable_entry_wins_per_type() {
        let dir = tempfile::tempdir().unwrap();
        let (user, system) = (dir.path().join("user"), dir.path().join("system"));
        for (d, name, program) in [
            (&user, "a.thumbnailer", "missing"),
            (&user, "b.thumbnailer", "user-tool"),
            (&system, "c.thumbnailer", "system-tool"),
        ] {
            std::fs::create_dir_all(d.join("thumbnailers")).unwrap();
            std::fs::write(
                d.join("thumbnailers").join(name),
                format!(
                    "[Thumbnailer Entry]\nExec={program} %i %o\nMimeType=image/png;text/x-c;\n"
                ),
            )
            .unwrap();
        }
        // An unusable template earlier in the path claims nothing.
        std::fs::write(
            user.join("thumbnailers/0-broken.thumbnailer"),
            "[Thumbnailer Entry]\nExec=broken %i\nMimeType=image/png;\n",
        )
        .unwrap();
        let registry = Registry::load_from(&[user, system], &|p| p != "missing");
        assert_eq!(registry.for_mime("image/png").unwrap().exec[0], "user-tool");
        assert!(registry.for_mime("video/mp4").is_none());
    }
}
