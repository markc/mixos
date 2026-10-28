// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};
use mail_parser::{Message, MessageParser, MimeHeaders, PartType};

use super::{EXTENSIONS, MAX_ACTIONS, MAX_REDIRECTS, MAX_SCRIPT_BYTES, MAX_STEPS};

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    String(String),
    Number(u64),
    Tag(String),
    Punct(char),
}

fn lex(src: &str) -> Result<Vec<Token>> {
    ensure!(src.len() <= MAX_SCRIPT_BYTES, "script exceeds 65536 bytes");
    ensure!(!src.contains('\0'), "NUL in script");
    let mut p = 0;
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    while p < bytes.len() {
        match bytes[p] {
            b if b.is_ascii_whitespace() => p += 1,
            b'#' => {
                while p < bytes.len() && bytes[p] != b'\n' {
                    p += 1;
                }
            }
            b'/' if bytes.get(p + 1) == Some(&b'*') => {
                let end = src[p + 2..]
                    .find("*/")
                    .ok_or_else(|| anyhow::anyhow!("unterminated comment"))?;
                p += end + 4;
            }
            b'"' => {
                p += 1;
                let mut value = Vec::new();
                let mut closed = false;
                while p < bytes.len() {
                    let b = bytes[p];
                    p += 1;
                    if b == b'"' {
                        closed = true;
                        break;
                    }
                    if b == b'\\' {
                        let next = *bytes
                            .get(p)
                            .ok_or_else(|| anyhow::anyhow!("trailing escape"))?;
                        value.push(next);
                        p += 1;
                    } else {
                        value.push(b);
                    }
                }
                ensure!(closed, "unterminated quoted string");
                out.push(Token::String(String::from_utf8(value)?));
            }
            b if b.is_ascii_digit() => {
                let start = p;
                while p < bytes.len() && bytes[p].is_ascii_digit() {
                    p += 1;
                }
                let mut n: u64 = src[start..p].parse()?;
                if let Some(factor) = bytes.get(p).and_then(|b| match b.to_ascii_lowercase() {
                    b'k' => Some(1024),
                    b'm' => Some(1024 * 1024),
                    b'g' => Some(1024 * 1024 * 1024),
                    _ => None,
                }) {
                    n = n
                        .checked_mul(factor)
                        .ok_or_else(|| anyhow::anyhow!("number overflow"))?;
                    p += 1;
                }
                out.push(Token::Number(n));
            }
            b':' if !src[p..].starts_with(":\r\n") => {
                p += 1;
                let start = p;
                while p < bytes.len() && (bytes[p].is_ascii_alphanumeric() || bytes[p] == b'_') {
                    p += 1;
                }
                ensure!(p > start, "empty tag");
                out.push(Token::Tag(src[start..p].to_ascii_lowercase()));
            }
            b if b.is_ascii_alphabetic() || b == b'_' => {
                let start = p;
                while p < bytes.len() && (bytes[p].is_ascii_alphanumeric() || bytes[p] == b'_') {
                    p += 1;
                }
                let word = src[start..p].to_ascii_lowercase();
                if word == "text" && bytes.get(p) == Some(&b':') {
                    p += 1;
                    // RFC 5228 multi-line strings: optional whitespace/comment,
                    // dot terminator and dot unstuffing, canonical CRLF.
                    let end = src[p..]
                        .find('\n')
                        .ok_or_else(|| anyhow::anyhow!("missing text newline"))?
                        + p;
                    let tail = src[p..end].trim();
                    ensure!(
                        tail.is_empty() || tail.starts_with('#'),
                        "invalid text marker"
                    );
                    p = end + 1;
                    let mut value = String::new();
                    loop {
                        let end = src[p..].find('\n').map(|n| p + n).unwrap_or(src.len());
                        let line = src[p..end].trim_end_matches('\r');
                        p = (end + 1).min(src.len());
                        if line == "." {
                            break;
                        }
                        ensure!(end < src.len(), "unterminated text string");
                        value.push_str(line.strip_prefix('.').unwrap_or(line));
                        value.push_str("\r\n");
                    }
                    out.push(Token::String(value));
                } else {
                    out.push(Token::Word(word));
                }
            }
            b if b"{}[](),;".contains(&b) => {
                out.push(Token::Punct(b as char));
                p += 1;
            }
            _ => bail!("invalid token at byte {p}"),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Default)]
struct Args {
    tags: BTreeMap<String, Vec<String>>,
    values: Vec<Vec<String>>,
}
impl Args {
    fn has(&self, tag: &str) -> bool {
        self.tags.contains_key(tag)
    }
    fn tag(&self, tag: &str) -> Option<&str> {
        self.tags.get(tag)?.first().map(String::as_str)
    }
    fn value(&self, n: usize) -> &str {
        &self.values[n][0]
    }
}

#[derive(Debug, Clone)]
enum Test {
    Bool(bool),
    Not(Box<Test>),
    Group(bool, Vec<Test>),
    Match(String, Args),
}
#[derive(Debug, Clone)]
enum Statement {
    If(Vec<(Test, Vec<Statement>)>, Vec<Statement>),
    Action(String, Args),
}
#[derive(Debug, Clone)]
pub struct Script {
    code: Vec<Statement>,
    variables: bool,
}

struct Parser {
    tokens: Vec<Token>,
    p: usize,
    required: BTreeSet<String>,
    depth: usize,
}
impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.p)
    }
    fn take(&mut self) -> Result<Token> {
        let token = self
            .peek()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unexpected end of script"))?;
        self.p += 1;
        Ok(token)
    }
    fn punct(&mut self, c: char) -> bool {
        if self.peek() == Some(&Token::Punct(c)) {
            self.p += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, c: char) -> Result<()> {
        ensure!(self.punct(c), "expected {c}");
        Ok(())
    }
    fn word(&mut self) -> Result<String> {
        match self.take()? {
            Token::Word(w) => Ok(w),
            _ => bail!("expected command or test"),
        }
    }
    fn strings(&mut self) -> Result<Vec<String>> {
        let list = self.punct('[');
        let mut values = Vec::new();
        loop {
            match self.take()? {
                Token::String(s) => values.push(s),
                _ => bail!("expected string"),
            }
            if !list {
                break;
            }
            if self.punct(']') {
                break;
            }
            self.expect(',')?;
        }
        Ok(values)
    }
    fn need(&self, extension: &str) -> Result<()> {
        ensure!(
            self.required.contains(extension),
            "missing require {extension:?}"
        );
        Ok(())
    }
    fn args(&mut self, name: &str, test: bool) -> Result<Args> {
        let mut args = Args::default();
        let (arity, flags, valued): (usize, &[&str], &[&str]) = match name {
            "stop" | "discard" => (0, &[], &[]),
            "keep" => (0, &[], &["flags"]),
            "fileinto" => (1, &["copy", "create"], &["flags"]),
            "redirect" => (1, &["copy"], &[]),
            "set" => (
                2,
                &[
                    "lower",
                    "upper",
                    "lowerfirst",
                    "upperfirst",
                    "quotewildcard",
                    "length",
                ],
                &[],
            ),
            "setflag" | "addflag" | "removeflag" => (1, &[], &[]),
            "vacation" => (
                1,
                &["mime"],
                &["days", "addresses", "subject", "from", "handle"],
            ),
            "reject" | "ereject" => (1, &[], &[]),
            "exists" | "mailboxexists" => (1, &[], &[]),
            "size" => (1, &["over", "under"], &[]),
            "address" | "envelope" | "header" | "string" | "hasflag" => (
                if name == "hasflag" { 1 } else { 2 },
                &[
                    "is",
                    "contains",
                    "matches",
                    "localpart",
                    "domain",
                    "all",
                    "last",
                ],
                &["comparator", "count", "value", "index"],
            ),
            "body" => (
                1,
                &["is", "contains", "matches", "raw", "text"],
                &["comparator", "content", "count", "value"],
            ),
            _ => bail!(
                "unsupported {} {name}",
                if test { "test" } else { "action" }
            ),
        };
        while let Some(Token::Tag(tag)) = self.peek().cloned() {
            self.p += 1;
            ensure!(!args.has(&tag), "duplicate tag :{tag}");
            let values = if flags.contains(&tag.as_str()) {
                Vec::new()
            } else if valued.contains(&tag.as_str()) {
                if tag == "days" || tag == "index" {
                    match self.take()? {
                        Token::Number(n) if n > 0 || tag == "days" => vec![n.to_string()],
                        _ => bail!("expected positive number"),
                    }
                } else {
                    self.strings()?
                }
            } else {
                bail!("unsupported tag :{tag} for {name}");
            };
            if tag == "copy" {
                self.need("copy")?;
            }
            if tag == "create" {
                self.need("mailbox")?;
            }
            if tag == "flags" {
                self.need("imap4flags")?;
            }
            if tag == "index" || tag == "last" {
                self.need("index")?;
            }
            if tag == "count" || tag == "value" {
                self.need("relational")?;
            }
            if tag == "comparator" {
                ensure!(values.len() == 1, "comparator needs one name");
                ensure!(
                    ["i;ascii-casemap", "i;octet", "i;ascii-numeric"].contains(&values[0].as_str()),
                    "unsupported comparator"
                );
                if values[0] == "i;ascii-numeric" {
                    self.need("comparator-i;ascii-numeric")?;
                }
            }
            if ["count", "value"].contains(&tag.as_str()) {
                ensure!(
                    values.len() == 1
                        && ["gt", "ge", "lt", "le", "eq", "ne"].contains(&values[0].as_str()),
                    "invalid relational operator"
                );
            }
            if ["subject", "from", "handle"].contains(&tag.as_str()) {
                ensure!(values.len() == 1, "tag needs one string");
            }
            args.tags.insert(tag, values);
        }
        for _ in 0..arity {
            if name == "size" {
                match self.take()? {
                    Token::Number(n) => args.values.push(vec![n.to_string()]),
                    _ => bail!("size requires number"),
                }
            } else {
                args.values.push(self.strings()?);
            }
        }
        if ["setflag", "addflag", "removeflag", "hasflag"].contains(&name)
            && matches!(self.peek(), Some(Token::String(_) | Token::Punct('[')))
        {
            self.need("variables")?;
            args.values.push(self.strings()?);
            if name != "hasflag" {
                ensure!(args.values[0].len() == 1, "flag variable must be one name");
            }
        }
        for n in match name {
            "fileinto" | "redirect" | "vacation" | "reject" | "ereject" => vec![0],
            "set" => vec![0, 1],
            _ => Vec::new(),
        } {
            ensure!(args.values[n].len() == 1, "{name} requires a single string");
        }
        let match_count = ["is", "contains", "matches", "count", "value"]
            .iter()
            .filter(|t| args.has(t))
            .count();
        ensure!(match_count <= 1, "conflicting match types");
        ensure!(
            args.tag("comparator") != Some("i;ascii-numeric")
                || !(args.has("contains") || args.has("matches")),
            "numeric comparator supports only equality and ordering"
        );
        ensure!(
            ["all", "localpart", "domain"]
                .iter()
                .filter(|t| args.has(t))
                .count()
                <= 1,
            "conflicting address parts"
        );
        if !["address", "envelope"].contains(&name) {
            ensure!(
                !["all", "localpart", "domain"].iter().any(|t| args.has(t)),
                "address tag on non-address test"
            );
        }
        if args.has("last") {
            ensure!(args.has("index"), ":last requires :index");
        }
        if args.has("index") {
            ensure!(
                ["header", "address"].contains(&name),
                "index requires header or address"
            );
        }
        if name == "size" {
            ensure!(
                args.has("over") != args.has("under"),
                "size requires :over or :under"
            );
        }
        if name == "body" {
            ensure!(
                ["raw", "text", "content"]
                    .iter()
                    .filter(|t| args.has(t))
                    .count()
                    <= 1,
                "conflicting body transforms"
            );
        }
        if args.has("count") {
            ensure!(
                args.tag("comparator") == Some("i;ascii-numeric"),
                ":count requires i;ascii-numeric"
            );
        }
        if name == "set" {
            let key = args.value(0);
            ensure!(
                !key.is_empty()
                    && key.bytes().enumerate().all(|(i, b)| b.is_ascii_alphabetic()
                        || b == b'_'
                        || (i > 0 && b.is_ascii_digit())),
                "invalid variable name"
            );
            ensure!(
                !(args.has("lower") && args.has("upper"))
                    && !(args.has("lowerfirst") && args.has("upperfirst")),
                "conflicting variable modifiers"
            );
        }
        Ok(args)
    }
    fn test(&mut self) -> Result<Test> {
        self.depth += 1;
        ensure!(self.depth <= 32, "nesting limit");
        let name = self.word()?;
        let test = match name.as_str() {
            "true" => Test::Bool(true),
            "false" => Test::Bool(false),
            "not" => Test::Not(Box::new(self.test()?)),
            "allof" | "anyof" => {
                self.expect('(')?;
                let mut tests = vec![self.test()?];
                while self.punct(',') {
                    tests.push(self.test()?);
                }
                self.expect(')')?;
                Test::Group(name == "allof", tests)
            }
            "header" | "address" | "exists" | "size" | "envelope" | "body" | "string"
            | "hasflag" | "mailboxexists" => {
                match name.as_str() {
                    "envelope" | "body" => self.need(&name)?,
                    "string" => self.need("variables")?,
                    "hasflag" => self.need("imap4flags")?,
                    "mailboxexists" => self.need("mailbox")?,
                    _ => {}
                }
                let args = self.args(&name, true)?;
                if name == "envelope" {
                    ensure!(
                        args.values[0]
                            .iter()
                            .all(|s| ["from", "to"].contains(&s.to_ascii_lowercase().as_str())),
                        "unsupported envelope part"
                    );
                }
                Test::Match(name, args)
            }
            _ => bail!("unknown test {name}"),
        };
        self.depth -= 1;
        Ok(test)
    }
    fn block(&mut self) -> Result<Vec<Statement>> {
        self.expect('{')?;
        self.depth += 1;
        ensure!(self.depth <= 32, "nesting limit");
        let code = self.code(false)?;
        self.expect('}')?;
        self.depth -= 1;
        Ok(code)
    }
    fn code(&mut self, top: bool) -> Result<Vec<Statement>> {
        let mut code = Vec::new();
        while self.peek().is_some() && self.peek() != Some(&Token::Punct('}')) {
            let name = self.word()?;
            match name.as_str() {
                "require" => {
                    ensure!(
                        top && code.is_empty(),
                        "require must precede executable statements"
                    );
                    for extension in self.strings()? {
                        ensure!(
                            EXTENSIONS.split_whitespace().any(|e| e == extension),
                            "unsupported extension {extension}"
                        );
                        self.required.insert(extension);
                    }
                    self.expect(';')?;
                }
                "if" => {
                    let test = self.test()?;
                    let body = self.block()?;
                    let mut branches = vec![(test, body)];
                    while self.peek() == Some(&Token::Word("elsif".into())) {
                        self.p += 1;
                        let test = self.test()?;
                        branches.push((test, self.block()?));
                    }
                    let otherwise = if self.peek() == Some(&Token::Word("else".into())) {
                        self.p += 1;
                        self.block()?
                    } else {
                        Vec::new()
                    };
                    code.push(Statement::If(branches, otherwise));
                }
                "stop" | "keep" | "discard" | "fileinto" | "redirect" | "vacation" | "set"
                | "setflag" | "addflag" | "removeflag" | "reject" | "ereject" => {
                    match name.as_str() {
                        "fileinto" | "vacation" | "reject" | "ereject" => self.need(&name)?,
                        "set" => self.need("variables")?,
                        "setflag" | "addflag" | "removeflag" => self.need("imap4flags")?,
                        _ => {}
                    }
                    let args = self.args(&name, false)?;
                    self.expect(';')?;
                    code.push(Statement::Action(name, args));
                }
                _ => bail!("unknown command {name}"),
            }
        }
        Ok(code)
    }
}

pub fn compile(src: &str) -> Result<Script> {
    let mut parser = Parser {
        tokens: lex(src)?,
        p: 0,
        required: BTreeSet::new(),
        depth: 0,
    };
    let code = parser.code(true)?;
    ensure!(parser.p == parser.tokens.len(), "unexpected closing brace");
    Ok(Script {
        code,
        variables: parser.required.contains("variables"),
    })
}

/// A fileinto/keep snapshot: subsequent flag changes do not alter earlier actions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filing {
    pub mailbox: Option<String>,
    pub create: bool,
    pub flags: Vec<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Vacation {
    pub mime: bool,
    pub days: u32,
    pub addresses: Vec<String>,
    pub subject: Option<String>,
    pub from: Option<String>,
    pub handle: String,
    pub reason: String,
}
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub filings: Vec<Filing>,
    pub redirects: Vec<String>,
    pub vacation: Option<Vacation>,
    pub reject: Option<String>,
    pub extended_reject: bool,
    pub discarded: bool,
}
pub struct Context<'a> {
    pub data: &'a [u8],
    pub from: &'a str,
    pub to: &'a str,
}
struct Eval<'a> {
    context: Context<'a>,
    message: Message<'a>,
    plan: Plan,
    variables: BTreeMap<String, String>,
    captures: Vec<String>,
    flags: Vec<String>,
    implicit_keep: bool,
    stopped: bool,
    steps: usize,
    variables_enabled: bool,
    text_headers: Vec<(String, String)>,
    variable_bytes: usize,
    mailboxes: BTreeSet<String>,
}
impl Script {
    pub fn evaluate(&self, context: Context<'_>) -> Result<Plan> {
        self.evaluate_with_mailboxes(context, &[])
    }

    /// Evaluate against a snapshot of this account's writable mailbox paths.
    /// The interpreter still performs no I/O.
    pub fn evaluate_with_mailboxes(&self, context: Context<'_>, mailboxes: &[String]) -> Result<Plan> {
        let message = MessageParser::default()
            .parse(context.data)
            .ok_or_else(|| anyhow::anyhow!("message parse error"))?;
        let mut text_parser = MessageParser::new().default_header_text();
        for header in message.headers() {
            text_parser = text_parser.header_text(header.name.as_str().to_string());
        }
        let text_headers = text_parser
            .parse_headers(context.data)
            .map(|m| {
                m.headers()
                    .iter()
                    .map(|h| {
                        (
                            h.name.as_str().to_string(),
                            h.value.as_text().unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut eval = Eval {
            context,
            message,
            plan: Plan::default(),
            variables: BTreeMap::new(),
            captures: Vec::new(),
            flags: Vec::new(),
            implicit_keep: true,
            stopped: false,
            steps: 0,
            variables_enabled: self.variables,
            text_headers,
            variable_bytes: 0,
            mailboxes: mailboxes.iter().map(|name| mailbox_key(name)).collect(),
        };
        eval.code(&self.code)?;
        if eval.implicit_keep {
            eval.plan.filings.push(Filing {
                flags: eval.flags,
                ..Filing::default()
            });
        }
        ensure!(eval.plan.redirects.len() <= MAX_REDIRECTS, "redirect limit");
        ensure!(
            eval.plan.filings.len() + eval.plan.redirects.len() <= MAX_ACTIONS,
            "action limit"
        );
        ensure!(
            eval.plan.reject.is_none()
                || (eval.plan.filings.is_empty()
                    && eval.plan.redirects.is_empty()
                    && eval.plan.vacation.is_none()
                    && !eval.plan.discarded),
            "reject conflicts with another action"
        );
        Ok(eval.plan)
    }
}
impl Eval<'_> {
    fn set_variable(&mut self, key: String, value: String) -> Result<()> {
        self.steps += value.len()/1024+1;
        ensure!(self.steps <= MAX_STEPS,"variable allocation work limit");
        let bytes = self.variable_bytes - self.variables.get(&key).map(String::len).unwrap_or(0)
            + value.len();
        ensure!(bytes <= 1024 * 1024, "variable storage limit");
        self.variables.insert(key, value);
        self.variable_bytes = bytes;
        Ok(())
    }
    fn step(&mut self) -> Result<()> {
        self.steps += 1;
        ensure!(self.steps <= MAX_STEPS, "instruction limit");
        Ok(())
    }
    fn expand(&self, value: &str) -> Result<String> {
        if !self.variables_enabled {
            return Ok(value.to_string());
        }
        let mut out = String::new();
        let mut rest = value;
        while let Some(pos) = rest.find("${") {
            out.push_str(&rest[..pos]);
            let next = &rest[pos + 2..];
            if let Some(end) = next.find('}') {
                let key = &next[..end];
                let identifier = |s: &str| {
                    !s.is_empty()
                        && s.bytes().enumerate().all(|(i, b)| {
                            b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit())
                        })
                };
                let number = !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit());
                if key.contains('.') && key.split('.').all(identifier) {
                    bail!("unsupported variable namespace");
                }
                if !number && !identifier(key) {
                    // Invalid references stay literal; resume inside them so a
                    // nested valid reference still receives its one expansion.
                    out.push_str("${");
                    rest = next;
                    continue;
                }
                if let Ok(n) = key.parse::<usize>() {
                    out.push_str(self.captures.get(n).map(String::as_str).unwrap_or(""));
                } else {
                    out.push_str(
                        self.variables
                            .get(&key.to_ascii_lowercase())
                            .map(String::as_str)
                            .unwrap_or(""),
                    );
                }
                rest = &next[end + 1..];
            } else {
                out.push_str(&rest[pos..]);
                rest = "";
            }
            ensure!(out.len() <= MAX_SCRIPT_BYTES, "expanded string limit");
        }
        out.push_str(rest);
        ensure!(out.len() <= MAX_SCRIPT_BYTES, "expanded string limit");
        Ok(out)
    }
    fn expanded(&mut self, values: &[String]) -> Result<Vec<String>> {
        let mut result = Vec::new();
        for value in values {
            let value = self.expand(value)?;
            self.steps += value.len()/1024+1;
            ensure!(self.steps <= MAX_STEPS,"expansion allocation limit");
            result.push(value);
        }
        Ok(result)
    }
    fn code(&mut self, code: &[Statement]) -> Result<()> {
        for statement in code {
            if self.stopped {
                break;
            }
            self.step()?;
            match statement {
                Statement::If(branches, otherwise) => {
                    let mut selected = false;
                    for (test, body) in branches {
                        if self.test(test)? {
                            self.code(body)?;
                            selected = true;
                            break;
                        }
                    }
                    if !selected {
                        self.code(otherwise)?;
                    }
                }
                Statement::Action(name, args) => self.action(name, args)?,
            }
        }
        Ok(())
    }
    fn action(&mut self, name: &str, args: &Args) -> Result<()> {
        ensure!(
            self.plan.filings.len() + self.plan.redirects.len() < MAX_ACTIONS,
            "action limit"
        );
        match name {
            "stop" => self.stopped = true,
            "discard" => {
                self.implicit_keep = false;
                self.plan.discarded = true;
            }
            "keep" | "fileinto" => {
                let flags = if let Some(flags) = args.tags.get("flags") {
                    self.expanded(flags)?
                } else {
                    self.flags.clone()
                };
                self.plan.filings.push(Filing {
                    mailbox: if name == "fileinto" {
                        Some(self.expand(args.value(0))?)
                    } else {
                        None
                    },
                    create: args.has("create"),
                    flags,
                });
                if !args.has("copy") {
                    self.implicit_keep = false;
                }
            }
            "redirect" => {
                let address = self.expand(args.value(0))?;
                if !self.plan.redirects.contains(&address) {
                    self.plan.redirects.push(address);
                }
                ensure!(self.plan.redirects.len() <= MAX_REDIRECTS, "redirect limit");
                if !args.has("copy") {
                    self.implicit_keep = false;
                }
            }
            "set" => {
                let key = args.value(0).to_ascii_lowercase();
                ensure!(
                    key.bytes().enumerate().all(|(i, b)| b.is_ascii_alphabetic()
                        || b == b'_'
                        || (i > 0 && b.is_ascii_digit()))
                        && !key.is_empty(),
                    "invalid variable name"
                );
                let mut value = self.expand(args.value(1))?;
                // RFC 5229 modifier precedence, independent of tag order.
                if args.has("lower") {
                    value = value.to_ascii_lowercase();
                } else if args.has("upper") {
                    value = value.to_ascii_uppercase();
                }
                if let Some(first) = value.chars().next() {
                    if args.has("lowerfirst") {
                        value = first.to_ascii_lowercase().to_string() + &value[first.len_utf8()..];
                    } else if args.has("upperfirst") {
                        value = first.to_ascii_uppercase().to_string() + &value[first.len_utf8()..];
                    }
                }
                if args.has("quotewildcard") {
                    value = value
                        .replace('\\', "\\\\")
                        .replace('*', "\\*")
                        .replace('?', "\\?");
                }
                if args.has("length") {
                    value = value.chars().count().to_string();
                }
                self.set_variable(key, value)?;
            }
            "setflag" | "addflag" | "removeflag" => {
                let flags = self.expanded(args.values.last().unwrap())?;
                let variable = if args.values.len() == 2 {
                    Some(args.value(0).to_ascii_lowercase())
                } else {
                    None
                };
                let mut target = if let Some(variable) = &variable {
                    let raw = self.variables.get(variable).cloned().unwrap_or_default();
                    self.flag_values(&[raw])?
                } else {
                    self.flags.clone()
                };
                if name == "setflag" {
                    target.clear();
                }
                for flag in flags
                    .into_iter()
                    .flat_map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
                {
                    self.steps += target.len() + 1;
                    ensure!(self.steps <= MAX_STEPS, "flag work limit");
                    if !valid_flag(&flag) { continue; }
                    target.retain(|f: &String| !f.eq_ignore_ascii_case(&flag));
                    if name != "removeflag" {
                        target.push(flag);
                    }
                    ensure!(target.len() <= crate::keyword::MAX_KEYWORDS_PER_MESSAGE + 5, "flag count limit");
                }
                if let Some(variable) = variable {
                    self.set_variable(variable, target.join(" "))?;
                } else {
                    self.flags = target;
                }
            }
            "vacation" => {
                ensure!(self.plan.vacation.is_none(), "multiple vacation actions");
                let reason = self.expand(args.value(0))?;
                let subject = args.tag("subject").map(|s| self.expand(s)).transpose()?;
                let from = args.tag("from").map(|s| self.expand(s)).transpose()?;
                let handle = if let Some(handle) = args.tag("handle") {
                    handle.to_string()
                } else {
                    // RFC 5230 response identity uses the unexpanded arguments.
                    blake3::hash(
                        format!(
                            "{:?}{:?}{}{}",
                            args.tag("subject"),
                            args.tag("from"),
                            args.has("mime"),
                            args.value(0)
                        )
                        .as_bytes(),
                    )
                    .to_hex()
                    .to_string()
                };
                self.plan.vacation = Some(Vacation {
                    mime: args.has("mime"),
                    days: args.tag("days").unwrap_or("7").parse::<u64>()?.clamp(1, 31) as u32,
                    addresses: args
                        .tags
                        .get("addresses")
                        .map(|v| self.expanded(v))
                        .transpose()?
                        .unwrap_or_default(),
                    subject,
                    from,
                    handle,
                    reason,
                });
            }
            "reject" | "ereject" => {
                self.plan.reject = Some(self.expand(args.value(0))?);
                self.plan.extended_reject = name == "ereject";
                self.implicit_keep = false;
            }
            _ => unreachable!(),
        }
        Ok(())
    }
    fn test(&mut self, test: &Test) -> Result<bool> {
        self.step()?;
        match test {
            Test::Bool(b) => Ok(*b),
            Test::Not(t) => Ok(!self.test(t)?),
            Test::Group(all, tests) => {
                for test in tests {
                    let value = self.test(test)?;
                    if value != *all {
                        return Ok(!*all);
                    }
                }
                Ok(*all)
            }
            Test::Match(name, args) => {
                if ["header", "address", "exists", "body"].contains(&name.as_str()) {
                    // Account for source scans and transforms even if the
                    // resulting text is empty (for example, HTML comments).
                    self.steps += self.context.data.len() / 1024;
                    ensure!(self.steps <= MAX_STEPS, "message scan work limit");
                }
                if name == "size" {
                    let n: usize = args.value(0).parse()?;
                    return Ok(if args.has("over") {
                        self.context.data.len() > n
                    } else {
                        self.context.data.len() < n
                    });
                }
                let names = self.expanded(&args.values[0])?;
                if name == "exists" {
                    for name in names {
                        self.steps += self.context.data.len()/1024+1;
                        ensure!(self.steps <= MAX_STEPS,"header scan limit");
                        if self.message.header(name.as_str()).is_none() { return Ok(false); }
                    }
                    return Ok(true);
                }
                if name == "mailboxexists" {
                    for name in names {
                        self.step()?;
                        if !self.mailboxes.contains(&mailbox_key(&name)) {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
                let mut values = Vec::new();
                let mut octets = Vec::new();
                match name.as_str() {
                    "header" => {
                        for n in &names {
                            self.steps += self.context.data.len()/1024+1;
                            ensure!(self.steps <= MAX_STEPS,"header scan limit");
                            let mut headers: Vec<&str> = self
                                .text_headers
                                .iter()
                                .filter(|(name, _)| name.eq_ignore_ascii_case(n))
                                .map(|(_, value)| value.as_str())
                                .collect();
                            if let Some(index) = args.tag("index") {
                                let index = index.parse::<usize>()?;
                                if args.has("last") {
                                    headers.reverse();
                                }
                                headers = headers.into_iter().nth(index-1).into_iter().collect();
                            }
                            for header in headers {
                                self.steps += header.len()/1024+1;
                                ensure!(self.steps <= MAX_STEPS,"header allocation limit");
                                values.push(header.to_string());
                            }
                        }
                    }
                    "address" => {
                        for n in &names {
                            self.steps += self.context.data.len()/1024+1;
                            ensure!(self.steps <= MAX_STEPS,"address scan limit");
                            // Parse arbitrary address headers with the same MIME parser.
                            let mut headers: Vec<_> = self
                                .message
                                .headers()
                                .iter()
                                .filter(|h| h.name.as_str().eq_ignore_ascii_case(n))
                                .collect();
                            if args.has("last") {
                                headers.reverse();
                            }
                            if let Some(index) = args.tag("index") {
                                headers = headers
                                    .into_iter()
                                    .nth(index.parse::<usize>()? - 1)
                                    .into_iter()
                                    .collect();
                            }
                            for header in headers {
                                let value = &header.value;
                                if let Some(addresses) = value.as_address() {
                                    values.extend(
                                        addresses
                                            .iter()
                                            .filter_map(|a| a.address())
                                            .map(str::to_string),
                                    );
                                }
                            }
                        }
                    }
                    "envelope" => {
                        for n in &names {
                            values.push(
                                if n.eq_ignore_ascii_case("from") {
                                    self.context.from
                                } else {
                                    self.context.to
                                }
                                .to_string(),
                            );
                        }
                    }
                    "body" => {
                        let content_types = args.tags.get("content").map(|types| self.expanded(types)).transpose()?;
                        if args.has("raw") {
                            let start = self.message.root_part().offset_body as usize;
                            octets.push(self.context.data[start..].to_vec());
                        } else {
                            for part in &self.message.parts {
                                self.steps += content_types.as_ref().map_or(1,Vec::len);
                                ensure!(self.steps <= MAX_STEPS,"MIME selector work limit");
                                let content_type = part
                                    .content_type()
                                    .map(|ct| {
                                        format!(
                                            "{}/{}",
                                            ct.ctype(),
                                            ct.subtype().unwrap_or("plain")
                                        )
                                    })
                                    .unwrap_or_else(|| "text/plain".into());
                                let selected = content_types
                                    .as_ref()
                                    .map(|types| {
                                        types.iter().any(|t| {
                                            t.is_empty()
                                                || content_type.eq_ignore_ascii_case(t)
                                                || content_type
                                                    .split('/')
                                                    .next()
                                                    .is_some_and(|p| p.eq_ignore_ascii_case(t))
                                        })
                                    })
                                    .unwrap_or_else(|| content_type.starts_with("text/"));
                                if selected && let PartType::Binary(bytes) | PartType::InlineBinary(bytes) = &part.body {
                                    octets.push(bytes.to_vec());
                                    continue;
                                }
                                if selected && let Some(text) = part.text_contents() {
                                    values.push(
                                        if !args.has("content")
                                            && matches!(&part.body, PartType::Html(_))
                                        {
                                            mail_parser::decoders::html::html_to_text(text)
                                        } else {
                                            text.to_string()
                                        },
                                    );
                                }
                            }
                        }
                    }
                    "string" => values = names.clone(),
                    "hasflag" => {
                        values = if args.values.len() == 2 {
                            let mut flags = Vec::new();
                            for name in &names {
                                let raw = self.variables.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default();
                                flags.extend(self.flag_values(&[raw])?);
                            }
                            flags
                        } else {
                            self.flags.clone()
                        };
                    }
                    _ => unreachable!(),
                }
                if name == "address" || name == "envelope" {
                    if args.has("localpart") {
                        values = values
                            .into_iter()
                            .map(|s| s.split_once('@').map(|(l, _)| l).unwrap_or(&s).to_string())
                            .collect();
                    } else if args.has("domain") {
                        values = values
                            .into_iter()
                            .map(|s| s.rsplit_once('@').map(|(_, d)| d).unwrap_or("").to_string())
                            .collect();
                    }
                }
                if args.has("count") {
                    if name == "string" {
                        values.retain(|value| !value.is_empty());
                    }
                    values = vec![(values.len()+octets.len()).to_string()];
                    octets.clear();
                }
                let mut keys = self.expanded(if name == "body" {
                    &args.values[0]
                } else if name == "hasflag" {
                    args.values.last().unwrap()
                } else {
                    &args.values[1]
                })?;
                if name == "hasflag" {
                    keys = keys.iter().flat_map(|s| s.split_whitespace().map(str::to_string)).collect();
                }
                for value in octets {
                    for key in &keys {
                        self.step()?;
                        if self.matches_octets(&value,key,args)? { return Ok(true); }
                    }
                }
                for value in values {
                    for key in &keys {
                        self.step()?;
                        // RFC 5173 section 6: body tests do not set match
                        // variables, even when using the :matches operator.
                        if self.matches(&value, key, args, name != "body")? {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            }
        }
    }
    fn flag_values(&mut self, raw: &[String]) -> Result<Vec<String>> {
        let mut values = BTreeMap::new();
        self.steps += raw.iter().map(String::len).sum::<usize>()/1024;
        ensure!(self.steps <= MAX_STEPS,"flag scan work limit");
        for flag in raw.iter().flat_map(|s| s.split_whitespace()) {
            self.step()?;
            if valid_flag(flag) {
                values.insert(flag.to_ascii_lowercase(),flag.to_string());
            }
        }
        Ok(values.into_values().collect())
    }
    fn matches_octets(&mut self, value: &[u8], key: &str, args: &Args) -> Result<bool> {
        self.steps += (value.len()+key.len())/1024;
        ensure!(self.steps <= MAX_STEPS,"string work limit");
        let mut value = value.to_vec();
        let mut key = key.as_bytes().to_vec();
        let numeric = args.tag("comparator") == Some("i;ascii-numeric");
        if !numeric && args.tag("comparator") != Some("i;octet") {
            value.make_ascii_lowercase(); key.make_ascii_lowercase();
        }
        if let Some(op) = args.tag("count").or(args.tag("value")) {
            let order = if numeric {
                // Only the ASCII digit prefix participates in numeric comparison.
                numeric_key(&String::from_utf8_lossy(&value)).cmp(&numeric_key(&String::from_utf8_lossy(&key)))
            } else { value.cmp(&key) };
            return Ok(match op { "gt"=>order.is_gt(),"ge"=>order.is_ge(),"lt"=>order.is_lt(),"le"=>order.is_le(),"eq"=>order.is_eq(),"ne"=>order.is_ne(),_=>unreachable!() });
        }
        if args.has("matches") { glob_octets(&value,&key,&mut self.steps) }
        else if args.has("contains") { Ok(contains_octets(&value,&key)) }
        else if numeric { Ok(numeric_key(&String::from_utf8_lossy(&value)) == numeric_key(&String::from_utf8_lossy(&key))) }
        else { Ok(value==key) }
    }
    fn matches(&mut self, value: &str, key: &str, args: &Args, capture: bool) -> Result<bool> {
        // Charge for scanned bytes as well as VM instructions. Repeated body
        // comparisons cannot turn a short script into unbounded CPU work.
        self.steps += (value.len() + key.len()) / 1024;
        ensure!(self.steps <= MAX_STEPS, "string work limit");
        let numeric = args.tag("comparator") == Some("i;ascii-numeric");
        let octet = args.tag("comparator") == Some("i;octet");
        let original = value;
        let value = if octet || numeric {
            value.to_string()
        } else {
            value.to_ascii_lowercase()
        };
        let key = if octet || numeric {
            key.to_string()
        } else {
            key.to_ascii_lowercase()
        };
        if let Some(op) = args.tag("count").or(args.tag("value")) {
            let order = if numeric {
                numeric_key(&value).cmp(&numeric_key(&key))
            } else {
                value.cmp(&key)
            };
            return Ok(match op {
                "gt" => order.is_gt(),
                "ge" => order.is_ge(),
                "lt" => order.is_lt(),
                "le" => order.is_le(),
                "eq" => order.is_eq(),
                "ne" => order.is_ne(),
                _ => unreachable!(),
            });
        }
        if args.has("matches") {
            if let Some(captures) = glob(original, &key, !octet, &mut self.steps)? {
                if capture { self.captures = captures; }
                return Ok(true);
            }
            Ok(false)
        } else if args.has("contains") {
            Ok(value.contains(&key))
        } else if numeric {
            Ok(numeric_key(&value) == numeric_key(&key))
        } else {
            Ok(value == key)
        }
    }
}

fn mailbox_key(path: &str) -> String {
    let (first, rest) = path.split_once('/').unwrap_or((path, ""));
    if first.eq_ignore_ascii_case("INBOX") {
        if rest.is_empty() { "INBOX".into() } else { format!("INBOX/{rest}") }
    } else {
        path.into()
    }
}

fn valid_flag(flag: &str) -> bool {
    if flag.starts_with('\\') {
        ["\\seen","\\answered","\\flagged","\\deleted","\\draft"].contains(&flag.to_ascii_lowercase().as_str())
    } else {
        flag.is_ascii() && crate::keyword::normalise_user_keyword(flag).is_ok()
    }
}

// Numeric comparator's non-digit prefix is positive infinity; digit sequences
// are compared by significant length so arbitrarily long numbers do not overflow.
fn numeric_key(s: &str) -> (bool, usize, String) {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let significant = digits.trim_start_matches('0').to_string();
    (digits.is_empty(), significant.len(), significant)
}

/// Linear-time substring matching; repeated near-matches in a binary body must
/// not multiply the byte-work budget by the key length.
fn contains_octets(value: &[u8], key: &[u8]) -> bool {
    if key.is_empty() { return true; }
    let mut prefix = vec![0;key.len()];
    let mut matched = 0;
    for i in 1..key.len() {
        while matched > 0 && key[i] != key[matched] { matched = prefix[matched-1]; }
        if key[i] == key[matched] { matched += 1; }
        prefix[i] = matched;
    }
    matched = 0;
    for byte in value {
        while matched > 0 && *byte != key[matched] { matched = prefix[matched-1]; }
        if *byte == key[matched] { matched += 1; }
        if matched == key.len() { return true; }
    }
    false
}

/// Body octets have no capture side effects. Match byte units without a UTF-8
/// conversion so invalid bytes and ASCII signatures retain their exact values.
fn glob_octets(value: &[u8], pattern: &[u8], steps: &mut usize) -> Result<bool> {
    ensure!(value.len() <= MAX_SCRIPT_BYTES,"wildcard input limit");
    let mut tokens = Vec::new();
    let mut bytes = pattern.iter().copied();
    while let Some(byte) = bytes.next() {
        tokens.push(if byte == b'\\' { (bytes.next().unwrap_or(b'\\'),false) } else { (byte,byte==b'*' || byte==b'?') });
    }
    let mut stack = vec![(0,0)];
    while let Some((mut p,mut v)) = stack.pop() {
        loop {
            *steps += 1;
            ensure!(*steps <= MAX_STEPS,"wildcard instruction limit");
            if p == tokens.len() { if v == value.len() { return Ok(true); } break; }
            let (byte,wildcard) = tokens[p];
            if wildcard && byte==b'*' {
                for end in (v..=value.len()).rev() {
                    *steps += 1;
                    ensure!(*steps <= MAX_STEPS,"wildcard allocation limit");
                    stack.push((p+1,end));
                }
                break;
            }
            if v == value.len() || (!wildcard && byte != value[v]) { break; }
            p += 1; v += 1;
        }
    }
    Ok(false)
}

fn glob(
    value: &str,
    pattern: &str,
    casemap: bool,
    steps: &mut usize,
) -> Result<Option<Vec<String>>> {
    // Iterative bounded wildcard matcher. Backtracking consumes the same budget
    // as instructions; no recursive stack or exponential unbounded work.
    ensure!(value.len() <= MAX_SCRIPT_BYTES, "wildcard input limit");
    let value: Vec<char> = value.chars().collect();
    let mut pattern_tokens = Vec::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        pattern_tokens.push(if c == '\\' {
            (chars.next().unwrap_or('\\'), false)
        } else {
            (c, c == '*' || c == '?')
        });
    }
    let mut stack = vec![(0, 0, Vec::<(usize, usize)>::new())];
    while let Some((mut p, mut v, mut captures)) = stack.pop() {
        loop {
            *steps += 1;
            ensure!(*steps <= MAX_STEPS, "wildcard instruction limit");
            if p == pattern_tokens.len() {
                if v == value.len() {
                    let mut strings = vec![value.iter().collect()];
                    strings.extend(
                        captures
                            .into_iter()
                            .map(|(start, end)| value[start..end].iter().collect()),
                    );
                    return Ok(Some(strings));
                }
                break;
            }
            let (c, wildcard) = pattern_tokens[p];
            if wildcard && c == '*' {
                // Bounded pending alternatives as well as bounded work.
                ensure!(
                    stack.len() + value.len() - v < MAX_STEPS,
                    "wildcard state limit"
                );
                for end in (v..=value.len()).rev() {
                    *steps += captures.len() + 1;
                    ensure!(*steps <= MAX_STEPS, "wildcard allocation limit");
                    let mut next = captures.clone();
                    next.push((v, end));
                    stack.push((p + 1, end, next));
                }
                break;
            }
            if v == value.len() {
                break;
            }
            if wildcard {
                captures.push((v, v + 1));
            } else if c
                != (if casemap {
                    value[v].to_ascii_lowercase()
                } else {
                    value[v]
                })
            {
                break;
            }
            p += 1;
            v += 1;
        }
    }
    Ok(None)
}
