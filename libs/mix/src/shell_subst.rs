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
//!   itself (`for f in …`, `x=1 cmd`, `read line`, `${x:=v}`) or the
//!   reference only runs when the shell decides to: the default operand of
//!   `${a:-$b}` and friends, which is rewritten when set and otherwise left
//!   to the shell;
//! * single-quoted text, escaped dollars, comments, the bodies of quoted
//!   shell heredocs (`<<'EOF'`), and positional and special parameters
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
    // Pass 1 only discovers the names the inner command binds, with the
    // same quoting, comment and heredoc rules as the rewrite; pass 2
    // rewrites against them.
    let mut discover = Scan::new(cmd, &lookup, HashSet::new(), true);
    discover.shell(false)?;
    let mut scan = Scan::new(cmd, &lookup, discover.found, false);
    scan.shell(false)?;
    Ok(Prepared {
        cmd: scan.out,
        env: scan.env,
    })
}

/// A placeholder for word text that is quoted or expanded: never part of
/// a name, so `'x'=1` or `$a=b` is no assignment.
const OPAQUE: char = '\u{0}';

/// Words of one shell level, `None` for a command separator.
type Words = Vec<Option<String>>;

fn flush(words: &mut Words, word: &mut String) {
    if !word.is_empty() {
        words.push(Some(std::mem::take(word)));
    }
}

/// A shell heredoc whose body starts after the current line.
struct Heredoc {
    delim: String,
    /// `<<'EOF'` / `<<"EOF"` / `<<\EOF`: the body is literal.
    quoted: bool,
    /// `<<-EOF`: leading tabs are stripped before matching the delimiter.
    dash: bool,
}

struct Scan<'a, F: Fn(&str) -> Option<String>> {
    c: Vec<char>,
    i: usize,
    out: String,
    params: HashMap<String, String>,
    env: Vec<(String, String)>,
    inner: HashSet<String>,
    lookup: &'a F,
    /// Inside a default operand (`${a:-…}`): references there may never
    /// run, so an unset one is left to the shell instead of raising.
    lenient: usize,
    pending: Vec<Heredoc>,
    /// Discovery pass: resolve nothing, raise nothing, and collect the
    /// names the inner command binds into `found`.
    discover: bool,
    found: HashSet<String>,
}

impl<'a, F: Fn(&str) -> Option<String>> Scan<'a, F> {
    fn new(cmd: &str, lookup: &'a F, inner: HashSet<String>, discover: bool) -> Self {
        Scan {
            c: cmd.chars().collect(),
            i: 0,
            out: String::with_capacity(cmd.len()),
            params: HashMap::new(),
            env: Vec::new(),
            inner,
            lookup,
            lenient: 0,
            pending: Vec::new(),
            discover,
            found: HashSet::new(),
        }
    }

    fn peek(&self, ahead: usize) -> Option<char> {
        self.c.get(self.i + ahead).copied()
    }

    fn copy(&mut self, n: usize) {
        for _ in 0..n {
            if let Some(c) = self.peek(0) {
                self.out.push(c);
                self.i += 1;
            }
        }
    }

    /// Unquoted shell text. With `in_subst`, stops (without consuming it)
    /// at the `)` that closes the enclosing `$(`.
    fn shell(&mut self, in_subst: bool) -> Result<(), String> {
        let mut depth = 0usize;
        // The unquoted words of this level, for binding discovery.
        let mut words: Words = Vec::new();
        let mut word = String::new();
        while let Some(c) = self.peek(0) {
            let word_start = word.is_empty();
            match c {
                '\'' => {
                    self.copy(1);
                    while let Some(q) = self.peek(0) {
                        self.copy(1);
                        if q == '\'' {
                            break;
                        }
                    }
                    word.push(OPAQUE);
                }
                '"' => {
                    self.copy(1);
                    self.dquote()?;
                    word.push(OPAQUE);
                }
                '\\' => {
                    self.copy(2);
                    word.push(OPAQUE);
                }
                '#' if word_start => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.copy(1);
                    }
                }
                '\n' => {
                    flush(&mut words, &mut word);
                    words.push(None);
                    self.copy(1);
                    self.heredoc_bodies()?;
                }
                '(' => {
                    flush(&mut words, &mut word);
                    words.push(None);
                    depth += 1;
                    self.copy(1);
                }
                ')' => {
                    if in_subst && depth == 0 {
                        break;
                    }
                    flush(&mut words, &mut word);
                    words.push(None);
                    depth = depth.saturating_sub(1);
                    self.copy(1);
                }
                // `<<<` is a here-string, not a heredoc.
                '<' if self.peek(1) == Some('<') && self.peek(2) == Some('<') => {
                    flush(&mut words, &mut word);
                    self.copy(3);
                }
                '<' if self.peek(1) == Some('<') => {
                    flush(&mut words, &mut word);
                    self.heredoc_op();
                    word.push(OPAQUE);
                }
                '$' => {
                    self.dollar()?;
                    word.push(OPAQUE);
                }
                ';' | '&' | '|' | '`' => {
                    flush(&mut words, &mut word);
                    words.push(None);
                    self.copy(1);
                }
                c if c.is_whitespace() => {
                    flush(&mut words, &mut word);
                    self.copy(1);
                }
                _ => {
                    word.push(c);
                    self.copy(1);
                }
            }
        }
        if self.discover {
            flush(&mut words, &mut word);
            self.found.extend(bindings_of(&words));
        }
        Ok(())
    }

    /// Inside `"…"` (the opening quote already copied): only `\` and `$`
    /// are special.
    fn dquote(&mut self) -> Result<(), String> {
        while let Some(c) = self.peek(0) {
            match c {
                '"' => {
                    self.copy(1);
                    return Ok(());
                }
                '\\' => self.copy(2),
                '$' => self.dollar()?,
                _ => self.copy(1),
            }
        }
        Ok(())
    }

    /// At a `$`.
    fn dollar(&mut self) -> Result<(), String> {
        match self.peek(1) {
            Some('(') => {
                // `$((…))` arithmetic: the same scan, closed by `))`.
                let arithmetic = self.peek(2) == Some('(');
                self.copy(if arithmetic { 3 } else { 2 });
                // A nested substitution is a fresh shell, with fresh quoting.
                let pending = std::mem::take(&mut self.pending);
                self.shell(true)?;
                self.pending = pending;
                self.copy(1);
                if arithmetic && self.peek(0) == Some(')') {
                    self.copy(1);
                }
            }
            Some('{') => self.braced()?,
            Some(n) if n.is_ascii_alphabetic() || n == '_' => {
                let start = self.i + 1;
                let mut end = start;
                while self
                    .c
                    .get(end)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    end += 1;
                }
                let name: String = self.c[start..end].iter().collect();
                match self.reference(&name, true)? {
                    Some(param) => {
                        self.out.push_str("${");
                        self.out.push_str(&param);
                        self.out.push('}');
                    }
                    None => {
                        self.out.push('$');
                        self.out.push_str(&name);
                    }
                }
                self.i = end;
            }
            _ => self.copy(1),
        }
        Ok(())
    }

    /// At the `$` of a `${…}`. A form it does not recognise (`${1}`,
    /// `${@}`, `${!x}`) is copied for the inner shell.
    fn braced(&mut self) -> Result<(), String> {
        let mut j = self.i + 2;
        let length = self.c.get(j) == Some(&'#');
        if length {
            j += 1;
        }
        let start = j;
        if !self
            .c
            .get(j)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
        {
            self.copy(1);
            return Ok(());
        }
        while self
            .c
            .get(j)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            j += 1;
        }
        let name: String = self.c[start..j].iter().collect();
        let next = self.c.get(j).copied();
        if next.is_none() || (length && next != Some('}')) {
            self.copy(1);
            return Ok(());
        }
        // `${name-x}` `${name=x}` `${name?x}` `${name+x}` and their `:` forms
        // handle an unset name themselves, and run their operand only when
        // the shell decides to.
        let op_len = match next {
            Some('-' | '=' | '?' | '+') => 1,
            Some(':') if matches!(self.c.get(j + 1), Some('-' | '=' | '?' | '+')) => 2,
            _ => 0,
        };
        let defaulted = !length && op_len > 0;
        // An assigning form binds the name when it runs.
        if self.discover && op_len > 0 && self.c.get(j + op_len - 1) == Some(&'=') {
            self.found.insert(name.clone());
        }
        let param = self.reference(&name, !defaulted)?;
        self.out.push_str("${");
        if length {
            self.out.push('#');
        }
        self.out.push_str(param.as_deref().unwrap_or(&name));
        self.i = j;
        if next == Some('}') {
            self.copy(1);
            return Ok(());
        }
        self.copy(op_len);
        if defaulted {
            self.lenient += 1;
        }
        let operand = self.operand();
        if defaulted {
            self.lenient -= 1;
        }
        operand
    }

    /// The rest of a `${name<op>…}` up to and including its `}`.
    fn operand(&mut self) -> Result<(), String> {
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            match c {
                '\\' => self.copy(2),
                '\'' => {
                    self.copy(1);
                    while let Some(q) = self.peek(0) {
                        self.copy(1);
                        if q == '\'' {
                            break;
                        }
                    }
                }
                '"' => {
                    self.copy(1);
                    self.dquote()?;
                }
                '$' => self.dollar()?,
                '{' => {
                    depth += 1;
                    self.copy(1);
                }
                '}' => {
                    self.copy(1);
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                }
                _ => self.copy(1),
            }
        }
        Ok(())
    }

    /// At a `<<` heredoc operator: copy it and its delimiter word, and
    /// queue the body for the end of the line.
    fn heredoc_op(&mut self) {
        self.copy(2);
        let dash = self.peek(0) == Some('-');
        if dash {
            self.copy(1);
        }
        while self.peek(0).is_some_and(|c| c == ' ' || c == '\t') {
            self.copy(1);
        }
        // The delimiter word, with shell quote removal: `<<'END TAG'`,
        // `<<"E"OF`, `<<\EOF` and `<<''` are all quoted delimiters.
        let mut delim = String::new();
        let mut quoted = false;
        while let Some(c) = self.peek(0) {
            match c {
                '\'' | '"' => {
                    quoted = true;
                    self.copy(1);
                    while let Some(q) = self.peek(0) {
                        self.copy(1);
                        if q == c {
                            break;
                        }
                        if c == '"' && q == '\\' {
                            if let Some(e) = self.peek(0) {
                                delim.push(e);
                                self.copy(1);
                            }
                            continue;
                        }
                        delim.push(q);
                    }
                }
                '\\' => {
                    quoted = true;
                    self.copy(1);
                    if let Some(e) = self.peek(0) {
                        delim.push(e);
                        self.copy(1);
                    }
                }
                c if c.is_whitespace()
                    || matches!(c, ';' | '&' | '|' | '(' | ')' | '<' | '>') =>
                {
                    break;
                }
                c => {
                    delim.push(c);
                    self.copy(1);
                }
            }
        }
        if quoted || !delim.is_empty() {
            self.pending.push(Heredoc {
                delim,
                quoted,
                dash,
            });
        }
    }

    /// After a newline: the bodies of the heredocs opened on that line. A
    /// quoted one is copied verbatim; an unquoted one expands `$` like a
    /// double-quoted word, and quotes in it are plain text.
    fn heredoc_bodies(&mut self) -> Result<(), String> {
        for doc in std::mem::take(&mut self.pending) {
            while self.i < self.c.len() {
                let end = self.c[self.i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(self.c.len(), |n| self.i + n);
                let line: String = self.c[self.i..end].iter().collect();
                let check = if doc.dash {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                let closer = check == doc.delim;
                if closer || doc.quoted {
                    self.copy(end - self.i);
                } else {
                    while self.i < end {
                        match self.peek(0) {
                            Some('\\') => self.copy(2),
                            Some('$') => self.dollar()?,
                            _ => self.copy(1),
                        }
                    }
                }
                self.copy(1); // the newline
                if closer {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Resolve one referenced `name`: `Some(param)` when it carries a
    /// value into the child, `None` when the inner shell owns it.
    fn reference(&mut self, name: &str, required: bool) -> Result<Option<String>, String> {
        if self.discover || self.inner.contains(name) {
            return Ok(None);
        }
        if let Some(param) = self.params.get(name) {
            return Ok(Some(param.clone()));
        }
        match (self.lookup)(name) {
            Some(value) => {
                let param = format!("{PARAM_PREFIX}{}", self.params.len());
                self.params.insert(name.to_string(), param.clone());
                self.env.push((param.clone(), value));
                Ok(Some(param))
            }
            None if required && self.lenient == 0 => Err(format!(
                "`${name}` inside `$(...)` is not set: it is neither a Mix variable nor an \
                 environment variable (write `${{{name}:-}}` for an intentionally empty value)"
            )),
            None => Ok(None),
        }
    }
}

fn is_name(w: &str) -> bool {
    let mut chars = w.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `NAME` of a `NAME=value` word.
fn assigned(w: &str) -> Option<&str> {
    w.split_once('=').map(|(n, _)| n).filter(|n| is_name(n))
}

/// Names one shell level binds itself, from its unquoted words (quoted and
/// expanded text is [`OPAQUE`], comments and heredoc bodies are absent):
/// assignments in command position (`x=1`, `x=1 cmd`), `for`/`select` loop
/// variables, `read` targets, `local`/`export`/`readonly`/`declare`/
/// `typeset` operands and `getopts`' variable. (`${name:=…}` is found by
/// the scanner itself.) A false positive only hands a name back to
/// `/bin/sh`, which is the old behaviour.
fn bindings_of(tokens: &[Option<String>]) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut command_position = true;
    let mut i = 0;
    while i < tokens.len() {
        let Some(w) = tokens[i].as_deref() else {
            command_position = true;
            i += 1;
            continue;
        };
        if !command_position {
            i += 1;
            continue;
        }
        if let Some(n) = assigned(w) {
            // `x=1` or `x=1 cmd`: still in command position.
            names.insert(n.to_string());
            i += 1;
            continue;
        }
        let word_at = |k: usize| tokens.get(k).and_then(|t| t.as_deref());
        match w {
            // Reserved words that leave the next word in command position.
            "do" | "then" | "else" | "elif" | "if" | "while" | "until" | "!" | "{" | "time" => {
                i += 1;
                continue;
            }
            "for" | "select" => {
                if let Some(n) = word_at(i + 1).filter(|n| is_name(n)) {
                    names.insert(n.to_string());
                }
            }
            "getopts" => {
                if let Some(n) = word_at(i + 2).filter(|n| is_name(n)) {
                    names.insert(n.to_string());
                }
            }
            "read" | "local" | "export" | "readonly" | "declare" | "typeset" => {
                let mut j = i + 1;
                while let Some(arg) = word_at(j) {
                    if arg.starts_with('-') {
                        // `read -p prompt`, `read -d delim` … take a value.
                        if w == "read"
                            && matches!(arg, "-p" | "-d" | "-t" | "-u" | "-n" | "-N" | "-i")
                        {
                            j += 1;
                        }
                    } else if is_name(arg) {
                        names.insert(arg.to_string());
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
        command_position = false;
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
            "outer" => Some("ok".to_string()),
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
        assert_eq!(
            p.env,
            vec![("__mix_subst_0".to_string(), "x y".to_string())]
        );
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
        assert!(prepare("echo ${nosuch%.txt}", scope).is_err());
    }

    #[test]
    fn a_defaulted_reference_does_not_raise() {
        for cmd in [
            "echo ${nosuch:-x}",
            "echo ${nosuch-x}",
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
    fn a_default_operand_is_lazy() {
        // The fallback may never run: an unset name there is the shell's.
        let p = prepare("echo ${outer:-$nosuch}", scope).unwrap();
        assert_eq!(p.cmd, "echo ${__mix_subst_0:-$nosuch}");
        // A set one in the operand still travels as data.
        let p = prepare("echo ${nosuch:-$d}", scope).unwrap();
        assert_eq!(p.cmd, "echo ${nosuch:-${__mix_subst_0}}");
        // `:=` assigns, so a later plain reference is the shell's too.
        let p = prepare("echo ${x:=fallback} $x", scope).unwrap();
        assert_eq!(p.cmd, "echo ${x:=fallback} $x");
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
            "x=1 sh -c 'echo $x'",
            "read -r line < /dev/null; echo $line",
            "export K=v; echo $K",
            "sh -c 'echo $nosuch'",
            "echo hi # a comment naming $nosuch",
            "if true; then y=2; fi; echo $y",
        ] {
            let p = prepare(cmd, scope).unwrap();
            assert_eq!(p.cmd, cmd, "{cmd}");
        }
    }

    #[test]
    fn an_argument_that_looks_like_an_assignment_binds_nothing() {
        // `zz=inner` is an argument to echo, not an assignment.
        let p = prepare("echo zz=inner $d", scope).unwrap();
        assert_eq!(p.cmd, "echo zz=inner ${__mix_subst_0}");
        assert!(prepare("echo d2=inner $nosuch", scope).is_err());
        // Quoted text, comments and heredoc bodies bind nothing either.
        let p = prepare("echo 'note; d=inner' $d", scope).unwrap();
        assert_eq!(p.cmd, "echo 'note; d=inner' ${__mix_subst_0}");
        let p = prepare("echo \"x; d=1\" $d # ; d=2\necho $d", scope).unwrap();
        assert_eq!(p.cmd, "echo \"x; d=1\" ${__mix_subst_0} # ; d=2\necho ${__mix_subst_0}");
        let p = prepare("cat <<'EOF'\nd=1\nEOF\necho $d", scope).unwrap();
        assert_eq!(p.cmd, "cat <<'EOF'\nd=1\nEOF\necho ${__mix_subst_0}");
        // A real assignment with a quoted value still binds.
        let p = prepare("d=\"a b\"; echo $d", scope).unwrap();
        assert_eq!(p.cmd, "d=\"a b\"; echo $d");
        // So does one inside a nested substitution (conservatively, for the
        // whole command).
        let p = prepare("echo $(d=1; echo $d)", scope).unwrap();
        assert_eq!(p.cmd, "echo $(d=1; echo $d)");
    }

    #[test]
    fn a_quoted_heredoc_delimiter_may_hold_spaces_and_quotes() {
        for cmd in [
            "cat <<'END TAG'\n$nosuch\nEND TAG\necho $d",
            "cat <<\"E\"OF\n$nosuch\nEOF\necho $d",
            "cat <<\\EOF\n$nosuch\nEOF\necho $d",
        ] {
            let p = prepare(cmd, scope).unwrap();
            assert!(p.cmd.ends_with("echo ${__mix_subst_0}"), "{cmd}: {}", p.cmd);
            assert!(p.cmd.contains("\n$nosuch\n"), "{cmd}: {}", p.cmd);
        }
        // `<<''` ends at the first empty line.
        let p = prepare("cat <<''\n$nosuch\n\necho $d", scope).unwrap();
        assert!(p.cmd.ends_with("echo ${__mix_subst_0}"), "{}", p.cmd);
    }

    #[test]
    fn a_quoted_shell_heredoc_body_is_literal() {
        let cmd = "cat <<'EOF'\n$d $nosuch it's\nEOF\necho $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(
            p.cmd,
            "cat <<'EOF'\n$d $nosuch it's\nEOF\necho ${__mix_subst_0}"
        );
        // Unquoted: the body expands, and its apostrophe is plain text.
        let p = prepare("cat <<EOF\nit's $d\nEOF", scope).unwrap();
        assert_eq!(p.cmd, "cat <<EOF\nit's ${__mix_subst_0}\nEOF");
        assert!(prepare("cat <<EOF\n$nosuch\nEOF", scope).is_err());
        // `<<-` strips tabs before the delimiter.
        let p = prepare("cat <<-'EOF'\n\t$nosuch\n\tEOF\necho ok", scope).unwrap();
        assert_eq!(p.cmd, "cat <<-'EOF'\n\t$nosuch\n\tEOF\necho ok");
    }

    #[test]
    fn a_nested_substitution_has_its_own_quoting() {
        let p = prepare("echo \"$(echo '$nosuch' $d)\"", scope).unwrap();
        assert_eq!(p.cmd, "echo \"$(echo '$nosuch' ${__mix_subst_0})\"");
        let p = prepare("echo $(( $(echo 1) + 2 )) $d", scope).unwrap();
        assert_eq!(p.cmd, "echo $(( $(echo 1) + 2 )) ${__mix_subst_0}");
        let p = prepare("cat <<< $d", scope).unwrap();
        assert_eq!(p.cmd, "cat <<< ${__mix_subst_0}");
    }

    #[test]
    fn an_environment_name_also_resolves() {
        let p = prepare("echo $HOME $d", scope).unwrap();
        assert_eq!(p.cmd, "echo ${__mix_subst_0} ${__mix_subst_1}");
        assert_eq!(p.env.len(), 2);
    }
}
