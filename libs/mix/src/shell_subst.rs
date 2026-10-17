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
    let mut discover = Scan::new(cmd, &lookup, HashMap::new(), true);
    discover.shell(Close::Top)?;
    let mut scan = Scan::new(cmd, &lookup, discover.found, false);
    scan.shell(Close::Top)?;
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

/// Where one open `case` is in its text: before `in`, in a clause's
/// pattern list (whose `)` is text, not a closer), or in a clause body.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Subject,
    Pattern,
    Body,
}

/// Whether the next word starts a command: after a separator, at the start
/// of a level, or after a reserved word that introduces one.
fn starts_command(words: &Words) -> bool {
    match words.last() {
        None | Some(None) => true,
        Some(Some(w)) => matches!(
            w.as_str(),
            "{" | "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "!" | "time"
        ),
    }
}

/// Finish the word being read. The keywords that open and close a `case`
/// are recognised here, in command position only.
fn finish(words: &mut Words, word: &mut String, cases: &mut Vec<Case>) {
    if !word.is_empty() {
        let at_command = starts_command(words);
        let after_in = matches!(words.last(), Some(Some(w)) if w == "in");
        let top = cases.last().copied();
        match word.as_str() {
            // A statement opens a `case`. A pattern list's `case` is a pattern.
            "case" if at_command && top != Some(Case::Pattern) => cases.push(Case::Subject),
            "in" if top == Some(Case::Subject) => {
                *cases.last_mut().expect("a subject is open") = Case::Pattern;
            }
            // `esac` closes the case where a clause or statement can start,
            // or straight after `in` for an empty case.
            "esac" if matches!(top, Some(Case::Pattern | Case::Body)) && (at_command || after_in) => {
                cases.pop();
            }
            _ => {}
        }
    }
    flush(words, word);
}

/// A shell heredoc whose body starts after the current line.
struct Heredoc {
    delim: String,
    /// `<<'EOF'` / `<<"EOF"` / `<<\EOF`: the body is literal.
    quoted: bool,
    /// `<<-EOF`: leading tabs are stripped before matching the delimiter.
    dash: bool,
}

/// What ends the shell level being scanned.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Close {
    /// The whole command text.
    Top,
    /// The `)` of a `$(...)`, or of `$((...))`.
    Paren,
    /// The closing backtick of a `` `...` `` substitution.
    Backtick,
}

struct Scan<'a, F: Fn(&str) -> Option<String>> {
    c: Vec<char>,
    i: usize,
    out: String,
    params: HashMap<String, String>,
    env: Vec<(String, String)>,
    /// Discovery output of the previous pass: the names each shell level
    /// binds itself, keyed by level number (see `levels`).
    bound: HashMap<usize, HashSet<String>>,
    /// Shell levels entered so far. Both passes enter the same levels in
    /// the same order, so a number names the same level in each.
    levels: usize,
    /// The levels enclosing the current position, innermost last. A name a
    /// level binds is the shell's wherever that level or a level inside it
    /// is scanning, and only there.
    stack: Vec<usize>,
    lookup: &'a F,
    /// Inside a default operand (`${a:-…}`): references there may never
    /// run, so an unset one is left to the shell instead of raising.
    lenient: usize,
    pending: Vec<Heredoc>,
    /// Discovery pass: resolve nothing, raise nothing, and collect the
    /// names each level binds into `found`.
    discover: bool,
    found: HashMap<usize, HashSet<String>>,
}

impl<'a, F: Fn(&str) -> Option<String>> Scan<'a, F> {
    fn new(
        cmd: &str,
        lookup: &'a F,
        bound: HashMap<usize, HashSet<String>>,
        discover: bool,
    ) -> Self {
        Scan {
            c: cmd.chars().collect(),
            i: 0,
            out: String::with_capacity(cmd.len()),
            params: HashMap::new(),
            env: Vec::new(),
            bound,
            levels: 0,
            stack: Vec::new(),
            lookup,
            lenient: 0,
            pending: Vec::new(),
            discover,
            found: HashMap::new(),
        }
    }

    /// Enter a shell level: it gets the next number, and the scan of its
    /// text sits on top of the enclosing levels.
    fn enter(&mut self) -> usize {
        let level = self.levels;
        self.levels += 1;
        self.stack.push(level);
        level
    }

    /// The innermost level being scanned.
    fn current_level(&self) -> usize {
        self.stack.last().copied().unwrap_or(0)
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

    /// Unquoted shell text of one level. Stops (without consuming it) at
    /// the character that closes `close`, when there is one.
    fn shell(&mut self, close: Close) -> Result<(), String> {
        let level = self.enter();
        // The unquoted words of this level, for binding discovery.
        let mut words: Words = Vec::new();
        let mut word = String::new();
        // The `case` statements open in this level, innermost last.
        let mut cases: Vec<Case> = Vec::new();
        while let Some(c) = self.peek(0) {
            let word_start = word.is_empty();
            match c {
                '`' if close == Close::Backtick => break,
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
                    finish(&mut words, &mut word, &mut cases);
                    words.push(None);
                    self.copy(1);
                    self.heredoc_bodies()?;
                }
                // In a `case` pattern list, `(` and `)` are pattern text.
                '(' if cases.last() == Some(&Case::Pattern) => {
                    finish(&mut words, &mut word, &mut cases);
                    self.copy(1);
                }
                '(' => {
                    finish(&mut words, &mut word, &mut cases);
                    words.push(None);
                    self.copy(1);
                    self.subshell()?;
                }
                // The word before a `)` is finished first: `esac)` closes
                // its `case` before the `)` is judged.
                ')' => {
                    finish(&mut words, &mut word, &mut cases);
                    if cases.last() == Some(&Case::Pattern) {
                        words.push(None);
                        self.copy(1);
                        *cases.last_mut().expect("a pattern is open") = Case::Body;
                    } else {
                        if close == Close::Paren {
                            break;
                        }
                        words.push(None);
                        self.copy(1);
                    }
                }
                // `<<<` is a here-string, not a heredoc.
                '<' if self.peek(1) == Some('<') && self.peek(2) == Some('<') => {
                    finish(&mut words, &mut word, &mut cases);
                    self.copy(3);
                }
                '<' if self.peek(1) == Some('<') => {
                    finish(&mut words, &mut word, &mut cases);
                    self.heredoc_op();
                    word.push(OPAQUE);
                }
                '$' => {
                    self.dollar()?;
                    word.push(OPAQUE);
                }
                '`' => {
                    self.copy(1);
                    self.backtick()?;
                    word.push(OPAQUE);
                }
                ';' | '&' | '|' => {
                    // The pending word is finished first, so an `esac` before
                    // this `;;` has already closed its own case.
                    finish(&mut words, &mut word, &mut cases);
                    words.push(None);
                    // `;;` ends a clause: the next pattern list follows.
                    if c == ';' && self.peek(1) == Some(';') && cases.last() == Some(&Case::Body)
                    {
                        self.copy(2);
                        *cases.last_mut().expect("a body is open") = Case::Pattern;
                    } else {
                        self.copy(1);
                    }
                }
                c if c.is_whitespace() => {
                    finish(&mut words, &mut word, &mut cases);
                    self.copy(1);
                }
                _ => {
                    word.push(c);
                    self.copy(1);
                }
            }
        }
        if self.discover {
            finish(&mut words, &mut word, &mut cases);
            self.found
                .entry(level)
                .or_default()
                .extend(bindings_of(&words));
        }
        self.stack.pop();
        Ok(())
    }

    /// The text of a `` `…` `` substitution, the opening backtick already
    /// copied. It is a shell of its own, with its own quoting, and the
    /// closing backtick is copied after it.
    fn backtick(&mut self) -> Result<(), String> {
        let pending = std::mem::take(&mut self.pending);
        self.shell(Close::Backtick)?;
        self.pending = pending;
        self.copy(1);
        Ok(())
    }

    /// Inside `"…"` (the opening quote already copied): `\`, `$` and the
    /// backtick are special.
    fn dquote(&mut self) -> Result<(), String> {
        while let Some(c) = self.peek(0) {
            match c {
                '"' => {
                    self.copy(1);
                    return Ok(());
                }
                '\\' => self.copy(2),
                '$' => self.dollar()?,
                '`' => {
                    self.copy(1);
                    self.backtick()?;
                }
                _ => self.copy(1),
            }
        }
        Ok(())
    }

    /// At a `$`.
    fn dollar(&mut self) -> Result<(), String> {
        match self.peek(1) {
            Some('(') if self.peek(2) == Some('(') => {
                self.copy(3);
                self.arithmetic()?;
            }
            Some('(') => {
                // A nested substitution is a fresh shell, with fresh quoting.
                self.copy(2);
                let pending = std::mem::take(&mut self.pending);
                self.shell(Close::Paren)?;
                self.pending = pending;
                self.copy(1);
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
            // `$$` is the PID: the second `$` is part of that parameter.
            Some('$') => self.copy(2),
            _ => self.copy(1),
        }
        Ok(())
    }

    /// The text of a `$((…))`, from just after its `$((`. It is arithmetic,
    /// not a command, so it runs in the enclosing level: an assignment in it
    /// binds in that level, and a reference in it resolves there.
    fn arithmetic(&mut self) -> Result<(), String> {
        // The arithmetic's own text. A nested substitution is another level
        // and contributes nothing to this one, so it is left out here.
        let mut own = String::new();
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            match c {
                ')' if depth == 0 => break,
                ')' => {
                    depth -= 1;
                    own.push(c);
                    self.copy(1);
                }
                '(' => {
                    depth += 1;
                    own.push(c);
                    self.copy(1);
                }
                '$' => {
                    own.push(' ');
                    self.dollar()?;
                }
                '`' => {
                    own.push(' ');
                    self.copy(1);
                    self.backtick()?;
                }
                _ => {
                    own.push(c);
                    self.copy(1);
                }
            }
        }
        if self.discover {
            let level = self.current_level();
            self.found
                .entry(level)
                .or_default()
                .extend(arith_bindings(&own));
        }
        // The two closing parentheses.
        self.copy(1);
        if self.peek(0) == Some(')') {
            self.copy(1);
        }
        Ok(())
    }

    /// The body of a `( … )` subshell, from just after its `(`. A subshell
    /// is a shell of its own: its bindings stay in its own level.
    fn subshell(&mut self) -> Result<(), String> {
        let pending = std::mem::take(&mut self.pending);
        self.shell(Close::Paren)?;
        self.pending = pending;
        self.copy(1);
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
            let level = self.current_level();
            self.found.entry(level).or_default().insert(name.clone());
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
                '`' => {
                    self.copy(1);
                    self.backtick()?;
                }
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
                        // Inside double quotes a backslash escapes only
                        // `$`, backtick, `"`, `\` and newline; before any
                        // other character it is literal text.
                        if c == '"' && q == '\\' {
                            match self.peek(0) {
                                // A line continuation is removed, not kept.
                                Some('\n') => self.copy(1),
                                Some(e @ ('$' | '`' | '"' | '\\')) => {
                                    delim.push(e);
                                    self.copy(1);
                                }
                                _ => delim.push('\\'),
                            }
                            continue;
                        }
                        delim.push(q);
                    }
                }
                '\\' => {
                    self.copy(1);
                    match self.peek(0) {
                        // A line continuation is removed before the word is
                        // read, so it leaves the delimiter unquoted.
                        Some('\n') => self.copy(1),
                        Some(e) => {
                            quoted = true;
                            delim.push(e);
                            self.copy(1);
                        }
                        None => quoted = true,
                    }
                }
                c if c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '<' | '>') => {
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
                let (line, end) = self.logical_line(!doc.quoted);
                let check = if doc.dash {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                let closer = check == doc.delim;
                if closer || doc.quoted {
                    self.copy(end - self.i);
                } else {
                    // An expansion may run past this line (a `$(` that
                    // closes on a later one), so the line ends at the first
                    // newline the scan reaches, not at `end`.
                    while let Some(c) = self.peek(0) {
                        match c {
                            '\n' => break,
                            '\\' => self.copy(2),
                            '$' => self.dollar()?,
                            '`' => {
                                self.copy(1);
                                self.backtick()?;
                            }
                            _ => self.copy(1),
                        }
                    }
                }
                if self.peek(0) == Some('\n') {
                    self.copy(1);
                }
                if closer {
                    break;
                }
            }
        }
        Ok(())
    }

    /// The body line from `self.i` as the shell reads it, and the index of
    /// the newline that ends it (or the end of the text). With `joined`, a
    /// backslash-newline pair is removed from the text, as an unquoted
    /// heredoc body is read; other backslash pairs stay as they are.
    fn logical_line(&self, joined: bool) -> (String, usize) {
        let mut text = String::new();
        let mut i = self.i;
        while let Some(&c) = self.c.get(i) {
            if c == '\n' {
                break;
            }
            if c == '\\' {
                match self.c.get(i + 1) {
                    Some(&'\n') if joined => {
                        i += 2;
                        continue;
                    }
                    Some(&next) if next != '\n' => {
                        text.push(c);
                        text.push(next);
                        i += 2;
                        continue;
                    }
                    _ => {}
                }
            }
            text.push(c);
            i += 1;
        }
        (text, i)
    }

    /// Whether a level that encloses this position binds `name` itself.
    fn bound_by_scope(&self, name: &str) -> bool {
        self.stack
            .iter()
            .any(|level| self.bound.get(level).is_some_and(|s| s.contains(name)))
    }

    /// Resolve one referenced `name`: `Some(param)` when it carries a
    /// value into the child, `None` when the inner shell owns it.
    fn reference(&mut self, name: &str, required: bool) -> Result<Option<String>, String> {
        if self.discover || self.bound_by_scope(name) {
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

/// Names an arithmetic expression assigns: `x=…` and `x op=…`, but not a
/// comparison (`==`, `!=`, `<=`, `>=`).
fn arith_bindings(text: &str) -> HashSet<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut names = HashSet::new();
    for (k, &c) in chars.iter().enumerate() {
        if c != '=' {
            continue;
        }
        // `==` and `!=` compare; the `=` of `==` is seen twice.
        if chars.get(k + 1) == Some(&'=') {
            continue;
        }
        let prev = if k > 0 { chars[k - 1] } else { ' ' };
        if prev == '=' || prev == '!' {
            continue;
        }
        // The operator of a compound or shift assignment sits before the
        // `=`: `+=`, `<<=`, `>>=`. A `<=` or `>=` is a comparison.
        let mut end = k;
        if matches!(prev, '<' | '>') {
            if !(k >= 2 && chars[k - 2] == prev) {
                continue;
            }
            end = k - 2;
        } else if "+-*/%&|^".contains(prev) {
            end = k - 1;
        }
        // Whitespace may separate the name from the operator: `zz = 1`.
        while end > 0 && chars[end - 1].is_whitespace() {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && (chars[start - 1].is_ascii_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }
        let name: String = chars[start..end].iter().collect();
        if is_name(&name) {
            names.insert(name);
        }
    }
    names
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
        assert_eq!(
            p.cmd,
            "echo \"x; d=1\" ${__mix_subst_0} # ; d=2\necho ${__mix_subst_0}"
        );
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
    fn a_binding_in_a_nested_substitution_stays_there() {
        // The inner `d=inner` owns only the inner `$d`; the outer `$d`
        // is still the Mix variable.
        let p = prepare("echo $(d=inner; echo $d); echo $d", scope).unwrap();
        assert_eq!(p.cmd, "echo $(d=inner; echo $d); echo ${__mix_subst_0}");
        assert_eq!(
            p.env,
            vec![("__mix_subst_0".to_string(), "x y".to_string())]
        );
        // A binding in an enclosing level still covers a nested reference.
        let p = prepare("d=1; echo $(echo $d)", scope).unwrap();
        assert_eq!(p.cmd, "d=1; echo $(echo $d)");
    }

    #[test]
    fn a_backtick_inside_double_quotes_has_its_own_quoting() {
        let p = prepare("echo \"`printf '%s' '$nosuch'`\"", scope).unwrap();
        assert_eq!(p.cmd, "echo \"`printf '%s' '$nosuch'`\"");
        let p = prepare("echo \"`echo $d`\"", scope).unwrap();
        assert_eq!(p.cmd, "echo \"`echo ${__mix_subst_0}`\"");
        // Unquoted backticks are a substitution too.
        let p = prepare("echo `echo $d` $d", scope).unwrap();
        assert_eq!(p.cmd, "echo `echo ${__mix_subst_0}` ${__mix_subst_0}");
        assert!(prepare("echo `echo $nosuch`", scope).is_err());
    }

    #[test]
    fn a_heredoc_delimiter_keeps_a_backslash_before_plain_text() {
        // In double quotes `\O` is not an escape: the delimiter is `E\OF`.
        let cmd = "cat <<\"E\\OF\"\n$nosuch\nE\\OF\necho $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(
            p.cmd,
            "cat <<\"E\\OF\"\n$nosuch\nE\\OF\necho ${__mix_subst_0}"
        );
    }

    #[test]
    fn a_multiline_substitution_in_a_heredoc_body_leaves_the_next_expansion() {
        let p = prepare("cat <<EOF\n$(printf hi\n)$d\nEOF", scope).unwrap();
        assert_eq!(p.cmd, "cat <<EOF\n$(printf hi\n)${__mix_subst_0}\nEOF");
    }

    #[test]
    fn arithmetic_assigns_in_the_enclosing_level() {
        // `zz` is in no scope; the assignment makes it the shell's.
        let p = prepare("echo $((zz=1)); echo $zz", scope).unwrap();
        assert_eq!(p.cmd, "echo $((zz=1)); echo $zz");
        assert!(p.env.is_empty());
        // A comparison is not an assignment.
        assert!(prepare("echo $((zz==1)) $zz", scope).is_err());
        // Arithmetic that only reads still resolves a Mix name.
        let p = prepare("echo $(($d)) $d", scope).unwrap();
        assert_eq!(p.cmd, "echo $((${__mix_subst_0})) ${__mix_subst_0}");
    }

    #[test]
    fn arithmetic_assignments_in_every_spelling_bind_in_the_enclosing_level() {
        for cmd in [
            "echo $((zz = 1)); echo $zz",
            "echo $((zz += 1)); echo $zz",
            "echo $((zz<<=1)); echo $zz",
            "echo $((zz >>= 1)); echo $zz",
        ] {
            let p = prepare(cmd, scope).unwrap();
            assert_eq!(p.cmd, cmd, "{cmd}");
        }
        // A nested substitution's assignment is that level's, not ours.
        let cmd = "echo $(( $(d=inner; echo 2) + 0 )); echo $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(
            p.cmd,
            "echo $(( $(d=inner; echo 2) + 0 )); echo ${__mix_subst_0}"
        );
    }

    #[test]
    fn a_case_pattern_does_not_close_the_enclosing_scope() {
        let cmd = "(case x in x) d=inner; echo $d;; esac); echo $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(
            p.cmd,
            "(case x in x) d=inner; echo $d;; esac); echo ${__mix_subst_0}"
        );
        // Inside a function body the pattern's `)` is text too. The body's
        // `d=inner` binds in the enclosing level (the conservative rule for
        // a function body), so the final `$d` stays the shell's.
        let cmd = "f() { case $1 in a) d=inner;; esac; }; echo $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(p.cmd, cmd);
    }

    #[test]
    fn a_subshell_binds_in_its_own_level() {
        let p = prepare("(d=inner; echo $d); echo $d", scope).unwrap();
        assert_eq!(p.cmd, "(d=inner; echo $d); echo ${__mix_subst_0}");
        assert_eq!(
            p.env,
            vec![("__mix_subst_0".to_string(), "x y".to_string())]
        );
    }

    #[test]
    fn an_escaped_newline_is_not_part_of_a_heredoc_delimiter() {
        let cmd = "cat <<\"E\\\nOF\"\n$nosuch\nEOF\necho $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(
            p.cmd,
            "cat <<\"E\\\nOF\"\n$nosuch\nEOF\necho ${__mix_subst_0}"
        );
    }

    #[test]
    fn a_continuation_joined_terminator_ends_an_unquoted_heredoc() {
        // The shell reads `EO\<newline>F` as `EOF`, so the next line is a
        // command, and its single-quoted text is the shell's.
        let cmd = "cat <<EOF\n$d\nEO\\\nF\necho '$nosuch'";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(
            p.cmd,
            "cat <<EOF\n${__mix_subst_0}\nEO\\\nF\necho '$nosuch'"
        );
    }

    #[test]
    fn a_backtick_in_an_operand_or_heredoc_has_its_own_context() {
        let cmd = "echo ${missing:-`d=inner; echo $d`}";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(p.cmd, cmd);
        let cmd = "cat <<EOF\n`printf '%s' '$nosuch'`\nEOF";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(p.cmd, cmd);
    }

    #[test]
    fn an_empty_case_closes_before_its_subshell_does() {
        let cmd = "(d=inner; case x in esac); echo $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(p.cmd, "(d=inner; case x in esac); echo ${__mix_subst_0}");
    }

    #[test]
    fn a_pattern_named_case_opens_nothing() {
        let cmd = "(case x in a) :;; case) d=inner;; esac); echo $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(
            p.cmd,
            "(case x in a) :;; case) d=inner;; esac); echo ${__mix_subst_0}"
        );
    }

    #[test]
    fn a_nested_esac_before_a_clause_terminator_closes_only_its_own_case() {
        let cmd = "(case z in x) case y in y) :;; esac;; z) d=inner;; esac); echo $d";
        let p = prepare(cmd, scope).unwrap();
        assert_eq!(
            p.cmd,
            "(case z in x) case y in y) :;; esac;; z) d=inner;; esac); echo ${__mix_subst_0}"
        );
    }

    #[test]
    fn a_double_dollar_is_one_parameter() {
        // `$$` is the PID: the text after it is not a reference.
        let p = prepare("echo $$nosuch $(echo $$x)", scope).unwrap();
        assert_eq!(p.cmd, "echo $$nosuch $(echo $$x)");
    }

    #[test]
    fn an_environment_name_also_resolves() {
        let p = prepare("echo $HOME $d", scope).unwrap();
        assert_eq!(p.cmd, "echo ${__mix_subst_0} ${__mix_subst_1}");
        assert_eq!(p.env.len(), 2);
    }
}
