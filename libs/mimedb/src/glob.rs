// SPDX-License-Identifier: MIT OR Apache-2.0
//! `globs2` patterns and their matching, in the order the desktop uses:
//! a literal name first, then the longest `*.suffix`, then full globs.

/// How a pattern is matched, decided once when the file is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// No wildcard: the whole name.
    Literal,
    /// A leading `*` and no other wildcard: the rest is a suffix.
    Suffix(String),
    /// Anything else: matched as an `fnmatch` pattern.
    Full,
}

#[derive(Clone, Debug)]
pub(crate) struct Glob {
    pub weight: u32,
    pub mime: String,
    pub pattern: String,
    pub case_sensitive: bool,
    pub kind: Kind,
}

impl Glob {
    pub fn new(weight: u32, mime: &str, pattern: &str, case_sensitive: bool) -> Self {
        Self {
            weight,
            mime: mime.to_owned(),
            pattern: pattern.to_owned(),
            case_sensitive,
            kind: kind_of(pattern),
        }
    }
}

fn kind_of(pattern: &str) -> Kind {
    let mut chars = pattern.chars();
    let leading_star = pattern.starts_with('*');
    if leading_star {
        chars.next();
    }
    if chars.any(|c| matches!(c, '*' | '?' | '[' | '\\')) {
        Kind::Full
    } else if leading_star {
        Kind::Suffix(pattern[1..].to_owned())
    } else {
        Kind::Literal
    }
}

/// Parse a `globs2` (`weight:mime:pattern[:flags[:…]]`, later fields
/// ignored) or legacy `globs` (`mime:pattern`, weight 50) file. `__NOGLOBS__` lines are returned as
/// their own list: they clear the type's globs from lower directories.
pub(crate) fn parse(text: &str, version2: bool) -> (Vec<Glob>, Vec<String>) {
    let mut globs = Vec::new();
    let mut cleared = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(':').collect();
        let (weight, mime, pattern, flags) = if version2 {
            let (w, m, p, f) = match fields.as_slice() {
                [w, m, p] => (*w, *m, *p, ""),
                [w, m, p, f, ..] => (*w, *m, *p, *f),
                _ => continue,
            };
            let Ok(weight) = w.parse() else {
                continue;
            };
            (weight, m, p, f)
        } else {
            match line.split_once(':') {
                Some((m, p)) => (50, m, p, ""),
                None => continue,
            }
        };
        if pattern == "__NOGLOBS__" {
            cleared.push(mime.to_owned());
            continue;
        }
        let case_sensitive = flags.split(',').any(|f| f == "cs");
        globs.push(Glob::new(weight, mime, pattern, case_sensitive));
    }
    (globs, cleared)
}

/// Drop a case-insensitive glob that also exists case-sensitively for the
/// same type: `globs2` can list `*.C` both ways, and the desktop's cache
/// keeps only the case-sensitive one (so `*.C` never claims `x.c`).
pub(crate) fn merge_case(globs: &mut Vec<Glob>) {
    let sensitive: std::collections::HashSet<(String, String)> = globs
        .iter()
        .filter(|g| g.case_sensitive)
        .map(|g| (g.mime.clone(), g.pattern.clone()))
        .collect();
    globs.retain(|g| g.case_sensitive || !sensitive.contains(&(g.mime.clone(), g.pattern.clone())));
}

/// GLib asks for at most this many glob candidates, and stops collecting
/// once it has them.
pub(crate) const MAX_CANDIDATES: usize = 10;

/// Add `name`'s longest-suffix matches in `globs` to `found`, up to the cap.
/// Case-sensitive globs count only when `all` is set. Patterns are compared
/// as stored. Two scans, so nothing is collected that the length rule drops.
fn suffix_hits(globs: &[Glob], name: &str, all: bool, found: &mut Vec<(String, u32)>) {
    let hit = |g: &Glob| match &g.kind {
        Kind::Suffix(suffix) if (all || !g.case_sensitive) && name.ends_with(suffix.as_str()) => {
            Some(suffix.len())
        }
        _ => None,
    };
    let Some(longest) = globs.iter().filter_map(hit).max() else {
        return;
    };
    for glob in globs {
        if found.len() >= MAX_CANDIDATES {
            return;
        }
        if hit(glob) == Some(longest) {
            found.push((glob.mime.clone(), glob.weight));
        }
    }
}

/// The first literal for `name` in `layers`: in each directory in order,
/// a literal with exactly that pattern ends the search, accepted when
/// `all` is set or it is case-insensitive, else rejected (the lower
/// directories are not searched).
fn literal<'a>(layers: &'a [Vec<Glob>], name: &str, all: bool) -> Option<&'a Glob> {
    for layer in layers {
        if let Some(glob) = layer
            .iter()
            .find(|g| g.kind == Kind::Literal && g.pattern == name)
        {
            return (all || !glob.case_sensitive).then_some(glob);
        }
    }
    None
}

/// The types `name` matches, highest weight first (stable within a weight),
/// each once with its highest weight. `layers` are the directories' globs,
/// highest precedence first; this is the desktop's cache lookup, which keeps
/// each directory's cache separate and collects at most
/// [`MAX_CANDIDATES`] matches before merging them:
///
/// - literals: the lowercased name against case-insensitive patterns, then
///   the name against all; the first hit is the whole answer;
/// - suffixes: the lowercased name against case-insensitive patterns, then,
///   while fewer than two matched, the name against all; in each pass the
///   longest suffix wins within a directory, and directories add up;
/// - full globs: the lowercased name against case-insensitive patterns only
///   when nothing has matched yet, then, while fewer than two matched, the
///   name against all.
///
/// Patterns are compared as stored: `update-mime-database` writes
/// case-insensitive ones in lower case.
pub(crate) fn candidates(layers: &[Vec<Glob>], name: &str) -> Vec<(String, u32)> {
    let lower = name.to_ascii_lowercase();
    if let Some(glob) = literal(layers, &lower, false).or_else(|| literal(layers, name, true)) {
        return vec![(glob.mime.clone(), glob.weight)];
    }

    let mut found: Vec<(String, u32)> = Vec::new();
    for layer in layers {
        suffix_hits(layer, &lower, false, &mut found);
    }
    if found.len() < 2 {
        for layer in layers {
            suffix_hits(layer, name, true, &mut found);
        }
    }

    let full = || layers.iter().flatten().filter(|g| g.kind == Kind::Full);
    if found.is_empty() {
        for glob in full().filter(|g| !g.case_sensitive) {
            if found.len() >= MAX_CANDIDATES {
                break;
            }
            if fnmatch(&glob.pattern, &lower) {
                found.push((glob.mime.clone(), glob.weight));
            }
        }
    }
    if found.len() < 2 {
        for glob in full() {
            if found.len() >= MAX_CANDIDATES {
                break;
            }
            if fnmatch(&glob.pattern, name) {
                found.push((glob.mime.clone(), glob.weight));
            }
        }
    }

    // Each type once, at its highest weight, removed the way GLib removes
    // duplicates (the last candidate takes the duplicate's place, which
    // decides the order of equal weights); then a stable sort by weight.
    // At most MAX_CANDIDATES entries, so this is cheap.
    let mut merged = found;
    let mut i = 0;
    while i < merged.len() {
        let mut j = i + 1;
        while j < merged.len() {
            if merged[j].0 == merged[i].0 {
                merged[i].1 = merged[i].1.max(merged[j].1);
                merged.swap_remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
    merged.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    merged
}

/// `fnmatch(pattern, name, 0)`: `*`, `?`, bracket classes (with `!` or `^`
/// negation and ranges) and backslash escapes.
pub(crate) fn fnmatch(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    matches_from(&p, &n)
}

fn matches_from(p: &[char], n: &[char]) -> bool {
    let (mut pi, mut ni) = (0, 0);
    // The most recent `*`: where it was, and how much of the name it holds.
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi, ni));
                    pi += 1;
                    continue;
                }
                '?' => {
                    pi += 1;
                    ni += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next)) = class(p, pi, n[ni]) {
                        if matched {
                            pi = next;
                            ni += 1;
                            continue;
                        }
                    } else if n[ni] == '[' {
                        // An unclosed bracket is a literal `[`.
                        pi += 1;
                        ni += 1;
                        continue;
                    }
                }
                '\\' if pi + 1 < p.len() => {
                    if p[pi + 1] == n[ni] {
                        pi += 2;
                        ni += 1;
                        continue;
                    }
                }
                c => {
                    if c == n[ni] {
                        pi += 1;
                        ni += 1;
                        continue;
                    }
                }
            }
        }
        match star {
            Some((star_pi, star_ni)) => {
                pi = star_pi + 1;
                ni = star_ni + 1;
                star = Some((star_pi, star_ni + 1));
            }
            None => return false,
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Match `c` against the bracket class starting at `p[start] == '['`:
/// `Some((matched, index after ']'))`, or `None` when it never closes.
fn class(p: &[char], start: usize, c: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = i < p.len() && (p[i] == '!' || p[i] == '^');
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == ']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        // [:class:], [=c=] (equivalence) and [.c.] (collating symbol). In a
        // UTF-8 locale without special collation the last two name one
        // character each.
        if p[i] == '['
            && let Some(&kind) = p.get(i + 1)
            && matches!(kind, ':' | '=' | '.')
            && let Some(close) =
                (i + 2..p.len().saturating_sub(1)).find(|&k| p[k] == kind && p[k + 1] == ']')
        {
            let inner: String = p[i + 2..close].iter().collect();
            let hit = match kind {
                ':' => named_class(&inner, c),
                _ => inner.chars().eq(std::iter::once(c)),
            };
            if hit {
                matched = true;
            }
            i = close + 2;
            continue;
        }
        let (lo, next) = endpoint(p, i);
        if next + 1 < p.len() && p[next] == '-' && p[next + 1] != ']' {
            let (hi, after) = endpoint(p, next + 1);
            if lo <= c && c <= hi {
                matched = true;
            }
            i = after;
        } else {
            if lo == c {
                matched = true;
            }
            i = next;
        }
    }
    None
}

/// The character at `p[i]` in a bracket expression, honouring a backslash
/// escape, and the index after it.
fn endpoint(p: &[char], i: usize) -> (char, usize) {
    if p[i] == '\\' && i + 1 < p.len() {
        (p[i + 1], i + 2)
    } else {
        (p[i], i + 1)
    }
}

/// The POSIX character classes. Letters, case and space use Unicode's
/// predicates and digits the ASCII ten: close to glibc in a UTF-8 locale,
/// but not its locale tables, so rare characters (no-break and em spaces,
/// line separators) and collating-symbol range ends can differ. The stock
/// freedesktop database checked on 2026-10-11 uses only ASCII ranges in its
/// brackets (`*.so.[0-9]*`, `*.[1-9]`), which match exactly; a database an
/// application or user installs may use more of `fnmatch`.
fn named_class(name: &str, c: char) -> bool {
    match name {
        "alpha" => c.is_alphabetic(),
        "digit" => c.is_ascii_digit(),
        "alnum" => c.is_alphabetic() || c.is_ascii_digit(),
        "upper" => c.is_uppercase(),
        "lower" => c.is_lowercase(),
        "space" => c.is_whitespace(),
        "blank" => c == ' ' || c == '\t',
        "punct" => {
            c.is_ascii_punctuation()
                || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control())
        }
        "xdigit" => c.is_ascii_hexdigit(),
        "cntrl" => c.is_control(),
        "print" => !c.is_control(),
        "graph" => !c.is_control() && !c.is_whitespace(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of("README"), Kind::Literal);
        assert_eq!(kind_of("*.tar.gz"), Kind::Suffix(".tar.gz".into()));
        assert_eq!(kind_of("readme*"), Kind::Full);
        assert_eq!(kind_of("*.[ch]"), Kind::Full);
        assert_eq!(kind_of("Makefile.*"), Kind::Full);
    }

    #[test]
    fn fnmatch_basics() {
        assert!(fnmatch("readme*", "readme.txt"));
        assert!(fnmatch("*.[ch]", "x.h"));
        assert!(!fnmatch("*.[ch]", "x.o"));
        assert!(fnmatch("*.[!o]", "x.c"));
        assert!(fnmatch("a?c", "abc"));
        assert!(!fnmatch("a?c", "ac"));
        assert!(fnmatch("*", ""));
        assert!(fnmatch("a*b*c", "axxbyyc"));
        assert!(!fnmatch("a*b*c", "axxbyy"));
        assert!(fnmatch("\\*x", "*x"));
        assert!(fnmatch("[a-c]z", "bz"));
    }

    fn db(lines: &str) -> Vec<Glob> {
        parse(lines, true).0
    }

    /// One directory's globs.
    fn find(g: &[Glob], name: &str) -> Vec<(String, u32)> {
        candidates(&[g.to_vec()], name)
    }

    #[test]
    fn literal_beats_everything_and_folds_case() {
        let g = db("50:text/x-a:*.txt\n50:text/x-lit:core\n");
        assert_eq!(find(&g, "core"), vec![("text/x-lit".into(), 50)]);
        assert_eq!(find(&g, "CORE"), vec![("text/x-lit".into(), 50)]);
    }

    #[test]
    fn longest_suffix_wins() {
        let g = db("50:application/gzip:*.gz\n50:application/x-tar-gz:*.tar.gz\n");
        assert_eq!(
            find(&g, "a.tar.gz"),
            vec![("application/x-tar-gz".into(), 50)]
        );
        assert_eq!(find(&g, "a.gz"), vec![("application/gzip".into(), 50)]);
    }

    #[test]
    fn case_sensitive_suffix_and_folded_full_glob() {
        let g = db("50:text/x-c:*.c:cs\n50:text/x-cpp:*.C:cs\n10:text/x-readme:readme*\n");
        assert_eq!(find(&g, "x.c"), vec![("text/x-c".into(), 50)]);
        assert_eq!(find(&g, "x.C"), vec![("text/x-cpp".into(), 50)]);
        assert_eq!(find(&g, "README"), vec![("text/x-readme".into(), 10)]);
    }

    #[test]
    fn ties_keep_weight_order_and_merge_duplicates() {
        let g = db(
            "40:application/typescript:*.ts\n50:video/mp2t:*.ts\n50:text/x-ts:*.ts\n45:video/mp2t:*.ts\n",
        );
        assert_eq!(
            find(&g, "a.ts"),
            vec![
                ("video/mp2t".into(), 50),
                ("text/x-ts".into(), 50),
                ("application/typescript".into(), 40)
            ]
        );
    }

    #[test]
    fn a_case_sensitive_twin_wins_and_a_suffix_hides_full_globs() {
        let mut g = db(
            "50:text/x-csrc:*.c:cs\n50:text/x-csrc:*.c\n50:text/x-c++src:*.C:cs\n50:text/x-c++src:*.C\n",
        );
        merge_case(&mut g);
        assert_eq!(g.len(), 2);
        assert_eq!(find(&g, "x.c"), vec![("text/x-csrc".into(), 50)]);
        assert_eq!(find(&g, "x.C"), vec![("text/x-c++src".into(), 50)]);
        let g = db("50:image/x-sun:*.sun\n10:text/x-readme:readme*\n");
        assert_eq!(find(&g, "README.Sun"), vec![("image/x-sun".into(), 50)]);
        assert_eq!(find(&g, "README"), vec![("text/x-readme".into(), 10)]);
    }

    #[test]
    fn noglobs_and_v1() {
        let (globs, cleared) = parse("50:text/x-a:*.a\n0:text/x-b:__NOGLOBS__\n", true);
        assert_eq!(globs.len(), 1);
        assert_eq!(cleared, vec!["text/x-b".to_string()]);
        let (globs, _) = parse("text/x-a:*.a\n", false);
        assert_eq!(globs[0].weight, 50);
    }

    #[test]
    fn each_directory_keeps_its_own_longest_suffix() {
        let high = db("80:application/x-high:*.gz\n");
        let low = db("50:application/x-low:*.tar.gz\n");
        assert_eq!(
            candidates(&[high, low], "x.tar.gz"),
            vec![
                ("application/x-high".into(), 80),
                ("application/x-low".into(), 50)
            ]
        );
    }

    #[test]
    fn later_globs2_fields_leave_the_flags_alone() {
        let g = db("50:text/x-a:*.c:cs:future\n");
        assert_eq!(find(&g, "x.c"), vec![("text/x-a".into(), 50)]);
        assert!(find(&g, "x.C").is_empty());
    }

    #[test]
    fn posix_classes() {
        assert!(fnmatch("file[[:digit:]].abc", "file1.abc"));
        assert!(!fnmatch("file[[:digit:]].abc", "filex.abc"));
        assert!(fnmatch("[[:upper:][:digit:]]x", "Qx"));
        assert!(fnmatch("[![:alpha:]]x", "7x"));
    }

    #[test]
    fn at_most_ten_candidates() {
        let mut lines = String::new();
        for i in 0..12 {
            lines.push_str(&format!("50:text/x-t{i}:*.x\n"));
        }
        assert_eq!(find(&db(&lines), "a.x").len(), MAX_CANDIDATES);
    }

    #[test]
    fn a_rejected_case_sensitive_literal_ends_the_folded_pass() {
        let high = db("50:text/x-high:readme:cs\n");
        let low = db("50:text/x-low:readme\n");
        let layers = [high, low];
        assert!(candidates(&layers, "README").is_empty());
        assert_eq!(
            candidates(&layers, "readme"),
            vec![("text/x-high".into(), 50)]
        );
    }

    #[test]
    fn equivalence_collating_escaped_ranges_and_unicode_classes() {
        assert!(fnmatch("[[=a=]].txt", "a.txt"));
        assert!(fnmatch("[[.a.]].txt", "a.txt"));
        assert!(!fnmatch("[[=a=]].txt", "b.txt"));
        assert!(fnmatch("[a-\\c].txt", "b.txt"));
        assert!(fnmatch("[[:alpha:]].txt", "é.txt"));
        assert!(!fnmatch("[[:digit:]].txt", "٣.txt"));
    }

    #[test]
    fn duplicates_are_removed_the_glib_way() {
        let layers = [
            db("50:application/x-a:*.q\n"),
            db("50:application/x-a:*.q\n"),
            db("50:text/x-b:*.q\n"),
            db("50:text/x-c:*.q\n"),
        ];
        let order: Vec<String> = candidates(&layers, "z.q")
            .into_iter()
            .map(|(m, _)| m)
            .collect();
        assert_eq!(order, vec!["application/x-a", "text/x-c", "text/x-b"]);
    }
}
