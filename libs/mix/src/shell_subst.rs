// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `$name` references inside a `$(...)` command substitution.
//!
//! `$(...)` hands its text to `/bin/sh -c`, and `/bin/sh` knows only the
//! process environment. A Mix local therefore vanished (`$(find $d)` ran
//! with `$d` empty) and an unset name expanded to "" without a word. Both
//! command-substitution paths (Mix code and shell-command words) now pass
//! the text through [`prepare`] first:
//!
//! * a `$name` / `${name}` that resolves (Mix scope first, then the
//!   environment — the same order as a bare shell word) is rewritten to a
//!   private parameter, and its value travels in the child's environment.
//!   The value is DATA: `/bin/sh` expands a parameter without re-parsing it
//!   for operators, so a value holding `; rm -rf /` cannot become syntax;
//! * a name that resolves nowhere raises, unless the inner command binds it
//!   itself (`for f in …`, `x=1`, `read line`) or the reference supplies its
//!   own default (`${name:-x}`, `${name-x}`, `${name:=x}`, `${name?msg}`,
//!   `${name:+x}`);
//! * single-quoted text, escaped dollars, positional and special parameters
//!   (`$1`, `$@`, `$?`, `$$`, …) belong to the inner shell and pass through.

use std::collections::{HashMap, HashSet};

/// A substitution ready to spawn: the rewritten command text plus the
/// environment entries that carry the referenced values.
#[derive(Debug, PartialEq, Eq)]
pub struct Prepared {
    pub cmd: String,
    pub env: Vec<(String, String)>,
}

/// Prefix of the private parameters the rewrite introduces.
const PARAM_PREFIX: &str = "__mix_subst_";

/// Rewrite every resolvable `$name` in `cmd` and refuse an unset one.
/// `lookup` is the shell-word resolver: Mix scope, then the environment.
pub fn prepare(cmd: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<Prepared, String> {
    let inner = inner_bindings(cmd);
    let chars: Vec<char> = cmd.chars().collect();
    let mut out = String::with_capacity(cmd.len());
    let mut params: HashMap<String, String> = HashMap::new();
    let mut env: Vec<(String, String)> = Vec::new();
    // One quote state per `$(...)` nesting level: a nested substitution
    // starts a fresh quoting context, exactly as in sh.
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let (mut in_single, mut in_double) = (false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            out.push(c);
            i += 1;
            continue;
        }
        match c {
            '\\' => {
                out.push(c);
                if let Some(&next) = chars.get(i + 1) {
                    out.push(next);
                }
                i += 2;
                continue;
            }
            '\'' if !in_double => in_single = true,
            '"' => in_double = !in_double,
            ')' if !in_double => {
                if let Some((s, d)) = stack.pop() {
                    in_single = s;
                    in_double = d;
                }
            }
            '$' => {
                match chars.get(i + 1) {
                    Some('(') if chars.get(i + 2) != Some(&'(') => {
                        stack.push((in_single, in_double));
                        in_single = false;
                        in_double = false;
                        out.push_str("$(");
                        i += 2;
                        continue;
                    }
                    Some('{') => {
                        if let Some(consumed) = braced(&chars, i, &inner, &lookup, &mut out, &mut params, &mut env)? {
                            i += consumed;
                            continue;
                        }
                    }
                    Some(&n) if n.is_ascii_alphabetic() || n == '_' => {
                        let name: String = chars[i + 1..]
                            .iter()
                            .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                            .collect();
                        let len = name.chars().count();
                        match reference(&name, true, &inner, &lookup, &mut params, &mut env)? {
                            Some(param) => {
                                out.push_str("${");
                                out.push_str(&param);
                                out.push('}');
                            }
                            None => {
                                out.push('$');
                                out.push_str(&name);
                            }
                        }
                        i += 1 + len;
                        continue;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        out.push(c);
        i += 1;
    }
    Ok(Prepared { cmd: out, env })
}

/// A `${…}` form starting at `chars[at]` (the `$`). Pushes its rewrite onto
/// `out` and returns how many chars it consumed, or `None` to leave a form
/// it does not recognise (`${1}`, `${@}`, `${!x}`) to the inner shell.
fn braced(
    chars: &[char],
    at: usize,
    inner: &HashSet<String>,
    lookup: &impl Fn(&str) -> Option<String>,
    out: &mut String,
    params: &mut HashMap<String, String>,
    env: &mut Vec<(String, String)>,
) -> Result<Option<usize>, String> {
    let mut j = at + 2;
    let length = chars.get(j) == Some(&'#');
    if length {
        j += 1;
    }
    let start = j;
    if !chars
        .get(j)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
    {
        return Ok(None);
    }
    while chars
        .get(j)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
    {
        j += 1;
    }
    let name: String = chars[start..j].iter().collect();
    let next = chars.get(j).copied();
    // `${name-x}` `${name=x}` `${name?x}` `${name+x}` and their `:` forms
    // handle an unset name themselves.
    let defaulted = !length
        && match next {
            Some('-' | '=' | '?' | '+') => true,
            Some(':') => matches!(chars.get(j + 1), Some('-' | '=' | '?' | '+')),
            _ => false,
        };
    if next.is_none() || (length && next != Some('}')) {
        return Ok(None);
    }
    let param = reference(&name, !defaulted, inner, lookup, params, env)?;
    out.push_str("${");
    if length {
        out.push('#');
    }
    out.push_str(param.as_deref().unwrap_or(&name));
    Ok(Some(j - at))
}

/// Resolve one referenced `name`: `Some(param)` when it carries a value
/// into the child, `None` when the inner shell owns it.
fn reference(
    name: &str,
    required: bool,
    inner: &HashSet<String>,
    lookup: &impl Fn(&str) -> Option<String>,
    params: &mut HashMap<String, String>,
    env: &mut Vec<(String, String)>,
) -> Result<Option<String>, String> {
    if inner.contains(name) {
        return Ok(None);
    }
    if let Some(param) = params.get(name) {
        return Ok(Some(param.clone()));
    }
    match lookup(name) {
        Some(value) => {
            let param = format!("{PARAM_PREFIX}{}", params.len());
            params.insert(name.to_string(), param.clone());
            env.push((param.clone(), value));
            Ok(Some(param))
        }
        None if required => Err(format!(
            "`${name}` inside `$(...)` is not set: it is neither a Mix variable nor an \
             environment variable (write `${{{name}:-}}` for an intentionally empty value)"
        )),
        None => Ok(None),
    }
}

/// Names the inner command binds itself, so a reference to them is the
/// inner shell's business: `name=…` assignments, `for`/`select` loop
/// variables, `read` targets, `local`/`export`/`readonly`/`declare`/
/// `typeset` operands and `getopts`' variable. Word-level and quote-blind
/// on purpose: a false positive only hands a name back to `/bin/sh`, which
/// is the old behaviour.
fn inner_bindings(cmd: &str) -> HashSet<String> {
    let words: Vec<&str> = cmd
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`'))
        .filter(|w| !w.is_empty())
        .collect();
    fn is_name(w: &str) -> bool {
        let mut chars = w.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    fn assigned(w: &str) -> Option<&str> {
        w.split_once('=').map(|(n, _)| n).filter(|n| is_name(n))
    }
    let mut names = HashSet::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].trim_matches(|c| c == '"' || c == '\'');
        if let Some(n) = assigned(w) {
            names.insert(n.to_string());
        }
        match w {
            "for" | "select" => {
                if let Some(n) = words.get(i + 1).filter(|n| is_name(n)) {
                    names.insert((*n).to_string());
                }
            }
            "getopts" => {
                if let Some(n) = words.get(i + 2).filter(|n| is_name(n)) {
                    names.insert((*n).to_string());
                }
            }
            "read" | "local" | "export" | "readonly" | "declare" | "typeset" => {
                let mut j = i + 1;
                while let Some(arg) = words.get(j) {
                    if arg.starts_with('-') {
                        // `read -p prompt`, `read -d delim` … take a value.
                        if w == "read" && matches!(*arg, "-p" | "-d" | "-t" | "-u" | "-n" | "-N" | "-i") {
                            j += 1;
                        }
                    } else if is_name(arg) {
                        names.insert((*arg).to_string());
                    } else if let Some(n) = assigned(arg) {
                        names.insert(n.to_string());
                    } else {
                        break;
                    }
                    j += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(name: &str) -> Option<String> {
        match name {
            "d" => Some("x y".to_string()),
            "HOME" => Some("/home/user".to_string()),
            "evil" => Some("; rm -rf / #".to_string()),
            _ => None,
        }
    }

    #[test]
    fn a_resolved_name_travels_as_a_private_parameter() {
        let p = prepare("echo $d pre-$d-post ${d}", scope).unwrap();
        assert_eq!(
            p.cmd,
            "echo ${__mix_subst_0} pre-${__mix_subst_0}-post ${__mix_subst_0}"
        );
        assert_eq!(p.env, vec![("__mix_subst_0".to_string(), "x y".to_string())]);
    }

    #[test]
    fn a_value_is_data_not_syntax() {
        let p = prepare("echo $evil", scope).unwrap();
        assert_eq!(p.cmd, "echo ${__mix_subst_0}");
        assert_eq!(p.env[0].1, "; rm -rf / #");
    }

    #[test]
    fn an_unset_name_raises() {
        let err = prepare("echo $nosuch", scope).unwrap_err();
        assert!(err.contains("`$nosuch`"), "{err}");
        assert!(err.contains("${nosuch:-}"), "{err}");
        assert!(prepare("echo ${nosuch}", scope).is_err());
        assert!(prepare("echo ${#nosuch}", scope).is_err());
        assert!(prepare("echo \"$nosuch\"", scope).is_err());
    }

    #[test]
    fn a_defaulted_reference_does_not_raise() {
        for cmd in [
            "echo ${nosuch:-x}",
            "echo ${nosuch-x}",
            "echo ${nosuch:=x}",
            "echo ${nosuch:+x}",
            "echo ${nosuch?gone}",
        ] {
            let p = prepare(cmd, scope).unwrap();
            assert_eq!(p.cmd, cmd);
            assert!(p.env.is_empty());
        }
        let p = prepare("echo ${d:-x}", scope).unwrap();
        assert_eq!(p.cmd, "echo ${__mix_subst_0:-x}");
    }

    #[test]
    fn the_inner_shell_keeps_its_own_text() {
        for cmd in [
            "echo '$nosuch'",
            "echo \\$nosuch",
            "echo \"\\$nosuch\"",
            "echo $1 $@ $? $$ $# $! ${1} ${@}",
            "echo $((1 + 2))",
            "for f in a b; do echo $f; done",
            "x=1; echo $x",
            "read -r line < /dev/null; echo $line",
            "export K=v; echo $K",
            "sh -c 'echo $nosuch'",
        ] {
            let p = prepare(cmd, scope).unwrap();
            assert_eq!(p.cmd, cmd, "{cmd}");
        }
    }

    #[test]
    fn a_nested_substitution_has_its_own_quoting() {
        let p = prepare("echo \"$(echo '$nosuch' $d)\"", scope).unwrap();
        assert_eq!(p.cmd, "echo \"$(echo '$nosuch' ${__mix_subst_0})\"");
    }

    #[test]
    fn an_environment_name_also_resolves() {
        let p = prepare("echo $HOME $d", scope).unwrap();
        assert_eq!(p.cmd, "echo ${__mix_subst_0} ${__mix_subst_1}");
        assert_eq!(p.env.len(), 2);
    }
}
