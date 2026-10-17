// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `mix man` pages are held to the binary, not to each other.
//!
//! The manual's index, keyword table, and version stamps are prose — and
//! prose rots silently (the pages once cited `docs/_man/`, a directory
//! that has never existed in the monorepo, and a keyword table missing
//! `elif`). This suite is the build-time gate for the three rot modes:
//!
//! 1. **Index ⇄ pages** — every topic the index links must exist, and
//!    every page on disk must be linked from the index. A dropped page
//!    that the index still names, or a new page nobody can discover, fails
//!    the build.
//! 2. **Keyword table** — `docs/mix/keywords.md`'s "full set" table must
//!    match the binary's `mix keywords` output exactly, in both
//!    directions. The binary is the oracle; the doc table is only allowed
//!    to agree with it.
//! 3. **Version stamps** — any "Verified against mix X.Y.Z" stamp older
//!    than this binary fails the build. The stamp is the weakest
//!    guarantee a page can carry (a number nobody re-checks), and a
//!    stale one is worse than none; pages either stamp the current
//!    version or point at this suite instead.
//!
//! Banned citations are checked too: `docs/_man/`, the archived
//! `markc/mix` repo, and the retired `markc.github.io` site must not
//! appear anywhere in the manual.
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

fn man_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/mix")
        .canonicalize()
        .expect("docs/mix must exist — it is the manual mix man reads")
}

fn read_page(name: &str) -> String {
    std::fs::read_to_string(man_dir().join(name))
        .unwrap_or_else(|e| panic!("cannot read docs/mix/{name}: {e}"))
}

fn pages() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(man_dir())
        .expect("read docs/mix")
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.ends_with(".md").then_some(name)
        })
        .collect();
    out.sort();
    out
}

/// Extract `topic` from every `[label](topic.md)` link in the index.
fn index_topics(index: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in index.lines() {
        let mut rest = line;
        while let Some(open) = rest.find('[') {
            let Some(close) = rest[open..].find(']') else {
                break;
            };
            let Some(paren) = rest[open + close..].find('(') else {
                break;
            };
            let target_start = open + close + paren + 1;
            let target_full = &rest[target_start..];
            if let Some(end) = target_full.find(')') {
                let target = &target_full[..end];
                // Only relative `topic.md` links name a manual page; skip
                // external URLs (e.g. the AGENTS.md GitHub link).
                if !target.contains("://")
                    && let Some(name) = target.strip_suffix(".md")
                {
                    out.insert(name.to_string());
                }
                // Advance past this link in the ORIGINAL line, not the
                // shadowed slice — slicing `target` again would always
                // yield "" and silently drop every later link on the line.
                rest = &target_full[end..];
            } else {
                break;
            }
        }
    }
    out
}

/// Backticked lexemes from the "full set" table in keywords.md.
fn keyword_table_lexemes(page: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_table = false;
    for line in page.lines() {
        let line = line.trim();
        if line.starts_with("## The full set") {
            in_table = true;
            continue;
        }
        if in_table && line.starts_with("## ") {
            break;
        }
        if !in_table || !line.starts_with('|') {
            continue;
        }
        let mut rest = line;
        while let Some(open) = rest.find('`') {
            let after = &rest[open + 1..];
            if let Some(end) = after.find('`') {
                out.insert(after[..end].to_string());
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
    out
}

/// The binary's reserved words, as `mix keywords` prints them.
fn binary_keywords() -> BTreeSet<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_mix"))
        .arg("keywords")
        .env("MIX_STATS", "off")
        .env_remove("MIXRC")
        .output()
        .expect("run mix keywords");
    assert!(
        out.status.success(),
        "mix keywords failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Mix reserved words:"))
        .map(str::to_string)
        .collect()
}

#[test]
fn index_links_every_page_and_only_real_pages() {
    let index = read_page("README.md");
    let linked = index_topics(&index);
    let on_disk: BTreeSet<String> = pages()
        .into_iter()
        .map(|p| p.trim_end_matches(".md").to_string())
        .collect();

    let mut missing = Vec::new();
    for topic in &linked {
        if !on_disk.contains(topic) {
            missing.push(topic.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "index links topics with no page: {missing:?}"
    );

    let mut unlisted = Vec::new();
    for topic in &on_disk {
        if topic != "README" && !linked.contains(topic) {
            unlisted.push(topic.clone());
        }
    }
    assert!(
        unlisted.is_empty(),
        "pages on disk not linked from the index (undiscoverable): {unlisted:?}"
    );

    // The gotchas pointer is the single most valuable affordance in the
    // manual — the index must name it (entry 2026-09-29).
    assert!(
        index.contains("[gotchas](gotchas.md)"),
        "the index must link gotchas.md — it is the errata every cold agent needs"
    );
}

#[test]
fn keyword_table_matches_the_binary() {
    let table = keyword_table_lexemes(&read_page("keywords.md"));
    let binary = binary_keywords();

    let mut doc_only: Vec<&String> = table.difference(&binary).collect();
    let mut bin_only: Vec<&String> = binary.difference(&table).collect();
    doc_only.sort();
    bin_only.sort();
    assert!(
        doc_only.is_empty(),
        "keywords.md names lexemes the binary does not reserve: {doc_only:?}"
    );
    assert!(
        bin_only.is_empty(),
        "mix keywords reserves words keywords.md does not list: {bin_only:?}"
    );
}

#[test]
fn no_stale_version_stamps_or_banned_citations() {
    let this_version = mix_shell::COMPONENT_VERSION;
    // Precise needles: "github.com/markc/mix" (the archived repo, in link
    // form) rather than the bare "markc/mix" substring, which also occurs
    // in api.github.com/repos/... example URLs.
    let banned = ["docs/_man/", "github.com/markc/mix", "markc.github.io"];

    for page in pages() {
        let content = read_page(&page);
        for needle in banned {
            assert!(
                !content.match_indices(needle).any(|(at, _)| {
                    needle != "github.com/markc/mix"
                        || !content[at + needle.len()..]
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                }),
                "docs/mix/{page} cites banned/retired location '{needle}'"
            );
        }
        for line in content.lines() {
            if let Some(pos) = line.find("Verified against") {
                // A numeric stamp is "mix X.Y.Z" (bold or not) — it must
                // not trail the binary. (Pages may instead name this
                // suite, which is the preferred form.) Scan for any
                // digit-led "mix N" token on the line so an un-bolded
                // stamp cannot slip past the check.
                let rest = &line[pos..];
                let mut idx = 0;
                while let Some(m) = rest[idx..].find("mix ") {
                    let after = &rest[idx + m + 4..];
                    if after.starts_with(|c: char| c.is_ascii_digit()) {
                        let stamped: String = after
                            .chars()
                            .take_while(|c| c.is_ascii_digit() || *c == '.')
                            .collect();
                        let ver = |v: &str| -> Vec<u64> {
                            v.split('.')
                                .map(|p| p.parse::<u64>().unwrap_or(0))
                                .collect()
                        };
                        assert!(
                            ver(&stamped) >= ver(this_version),
                            "docs/mix/{page} stamps 'mix {stamped}' which trails the binary (mix {this_version}) — re-verify the page and bump the stamp, or drop it for the man_pages pointer"
                        );
                    }
                    idx += m + 4;
                }
            }
        }
    }
}

/// docs/mix/builtins.md's tables are generated from `mix builtins --json`.
/// This binary runs the generator in check mode against itself, so a builtin
/// added, removed or redescribed without regenerating the page fails here.
#[test]
fn builtins_page_matches_this_binary() {
    let mix = env!("CARGO_BIN_EXE_mix");
    let generator =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/_build/gen-builtins-md.mix");
    let out = Command::new(mix)
        .arg(&generator)
        .args(["--mix", mix, "--check"])
        .env("MIX_STATS", "off")
        .env_remove("MIXRC")
        .output()
        .expect("run gen-builtins-md.mix");
    assert!(
        out.status.success(),
        "{}{}\nregenerate: mix docs/_build/gen-builtins-md.mix --mix target/debug/mix",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A category the page has no section for is refused, never dropped.
#[test]
fn builtins_generator_refuses_an_unplaced_category() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("temp dir");
    let fake = dir.path().join("fake-mix");
    // A Mix script run by this binary stands in for one whose table has a
    // category the page does not know.
    std::fs::write(
        &fake,
        format!(
            "#!{}\nprint('[{{\"name\":\"x\",\"category\":\"nope\",\"description\":\"d\",\"kind\":\"builtin\"}}]')\n",
            env!("CARGO_BIN_EXE_mix")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let generator =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/_build/gen-builtins-md.mix");
    let out = Command::new(env!("CARGO_BIN_EXE_mix"))
        .arg(&generator)
        .arg("--mix")
        .arg(&fake)
        .arg("--check")
        .env("MIX_STATS", "off")
        .env_remove("MIXRC")
        .output()
        .expect("run gen-builtins-md.mix");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("category 'nope'"),
        "{out:?}"
    );
}

/// `mix ARGS` in a home with no checkout, no installed share tree and no
/// cache. `local` rules the network out (MIXOS_MAN_SOURCE=local); otherwise
/// the default order applies, which reaches the built-in manual first.
fn man_alone(args: &[&str], local: bool) -> std::process::Output {
    man_with_checkout_page(args, local, None)
}

/// As [`man_alone`], with one `(filename, body)` page in an otherwise empty
/// checkout `docs/mix`.
fn man_with_checkout_page(
    args: &[&str],
    local: bool,
    page: Option<(&str, &str)>,
) -> std::process::Output {
    let home = tempfile::tempdir().expect("temp home");
    let root = home.path().join("no-checkout");
    std::fs::create_dir(&root).unwrap();
    if let Some((name, body)) = page {
        std::fs::create_dir_all(root.join("docs/mix")).unwrap();
        std::fs::write(root.join("docs/mix").join(name), body).unwrap();
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_mix"));
    command
        .args(args)
        .env("HOME", home.path())
        .env("MIXOS", &root)
        .env("MIXOS_SHARE", home.path().join("no-share"))
        .env("XDG_CACHE_HOME", home.path().join("no-cache"))
        .env("MIX_STATS", "off")
        .env_remove("MIXRC")
        .env_remove("MIXOS_MAN_URL")
        .env_remove("MIXOS_MAN_SOURCE");
    if local {
        command.env("MIXOS_MAN_SOURCE", "local");
    }
    command.output().expect("run mix")
}

#[test]
fn a_binary_alone_reads_its_built_in_manual() {
    for local in [true, false] {
        for (args, page) in [
            (&["man", "overview"][..], "overview.md"),
            (&["man"][..], "README.md"),
            (&["syntax"][..], "variables.md"),
        ] {
            let out = man_alone(args, local);
            assert!(out.status.success(), "{args:?}: {out:?}");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                read_page(page),
                "{args:?} (local: {local}) must print the built-in {page}"
            );
        }
    }
}

#[test]
fn a_missing_topic_lists_the_built_in_topics() {
    let out = man_alone(&["man", "no-such-topic"], true);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no manual page for 'no-such-topic'"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Available topics:") && stderr.contains("gotchas"),
        "{stderr}"
    );
}

#[test]
fn apropos_and_doctor_see_the_built_in_manual() {
    let out = man_alone(&["apropos", "heredoc"], true);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("MANUAL SECTIONS"),
        "{out:?}"
    );
    let out = man_alone(&["doctor"], true);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let built_in = pages().len() - 1;
    assert!(
        stdout.contains(&format!("{built_in} built in (")),
        "{stdout}"
    );
}

#[test]
fn local_mode_reads_and_searches_the_checkout_copy_first() {
    let page = Some(("overview.md", "# overview\n\n## Zebrafish checkout heading\n"));
    let out = man_with_checkout_page(&["apropos", "zebrafish"], true, page);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("mix man overview  § Zebrafish checkout heading"),
        "{out:?}"
    );
    let out = man_with_checkout_page(&["man", "overview"], true, page);
    assert!(String::from_utf8_lossy(&out.stdout).contains("Zebrafish"), "{out:?}");
    // By default the built-in page wins for both.
    let out = man_with_checkout_page(&["apropos", "zebrafish"], false, page);
    assert!(!String::from_utf8_lossy(&out.stdout).contains("Zebrafish"), "{out:?}");
    let out = man_with_checkout_page(&["man", "overview"], false, page);
    assert_eq!(String::from_utf8_lossy(&out.stdout), read_page("overview.md"));
}
