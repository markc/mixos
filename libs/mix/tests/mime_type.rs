// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mime_type()` against a private shared-mime-info database: globs, magic,
//! the text check, aliases and the parent chain, with no dependence on the
//! host's `/usr/share/mime`.
#![cfg(feature = "mime")]

use mix::evaluator::{Evaluator, SharedBuf};
use mix::lexer::Lexer;
use mix::parser::Parser;
use std::path::{Path, PathBuf};

fn test_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mix_mime_type_test_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn run(src: &str) -> Result<String, String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens, src);
    let stmts = parser.parse_program().map_err(|e| e.to_string())?;
    let stdout = SharedBuf::new();
    let stderr = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    eval.execute(&stmts).await.map_err(|e| e.to_string())?;
    Ok(stdout.to_string_lossy())
}

/// One magic rule: `>offset=` then the big-endian value length and value.
fn rule(offset: u32, value: &[u8]) -> Vec<u8> {
    let mut out = format!(">{offset}=").into_bytes();
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
    out.push(b'\n');
    out
}

/// A private database under `<root>/mime`.
fn write_database(root: &Path) {
    let mime = root.join("mime");
    std::fs::create_dir_all(&mime).unwrap();
    std::fs::write(
        mime.join("globs2"),
        "# test database\n\
         50:text/x-mixtest:*.mxt\n\
         50:text/x-child:*.chd\n\
         50:application/x-foo:*.foo\n\
         50:application/x-bar:*.foo\n",
    )
    .unwrap();
    let mut magic = b"MIME-Magic\0\n".to_vec();
    magic.extend_from_slice(b"[50:application/x-bar]\n");
    magic.extend(rule(0, b"BARX"));
    magic.extend_from_slice(b"[80:application/x-magiconly]\n");
    magic.extend(rule(0, b"MAGIC"));
    std::fs::write(mime.join("magic"), magic).unwrap();
    std::fs::write(
        mime.join("subclasses"),
        "text/x-mixtest text/x-parent\n\
         text/x-child text/x-oldtest\n",
    )
    .unwrap();
    std::fs::write(mime.join("aliases"), "text/x-oldtest text/x-mixtest\n").unwrap();
}

/// `mime_type(path[, opts])` as `mime by uncertain exists parents`.
fn probe(path: &Path, opts: &str) -> String {
    format!(
        "$r = mime_type(\"{}\"{opts})\nprint($r.mime .. \" \" .. $r.by .. \" \" .. to_string($r.uncertain) .. \" \" .. to_string($r.exists) .. \" \" .. join($r.parents, \",\"))\n",
        path.display()
    )
}

#[tokio::test]
async fn types_follow_the_desktop_order_against_a_private_database() {
    // One test owns the process environment: XDG_* are process-global.
    let root = test_dir("db");
    write_database(&root);
    let home = test_dir("home");
    // SAFETY: this binary has one test, and it sets the variables before
    // anything reads them.
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &home);
        std::env::set_var("XDG_DATA_DIRS", &root);
    }

    let files = test_dir("files");
    let cases: Vec<(PathBuf, &[u8], &str, &str)> = vec![
        // A unique glob names it; the parent chain ends text/plain, octet-stream.
        (
            files.join("a.mxt"),
            b"hello\n",
            "",
            "text/x-mixtest glob false true text/x-parent,text/plain,application/octet-stream",
        ),
        // A parent named by an alias is unaliased, and its own parents follow.
        (
            files.join("b.chd"),
            b"child\n",
            "",
            "text/x-child glob false true text/x-mixtest,text/x-parent,text/plain,application/octet-stream",
        ),
        // Two globs: magic settles which (the glob type, a subclass of the sniffed one).
        (
            files.join("c.foo"),
            b"BARX and more",
            "",
            "application/x-bar glob false true application/octet-stream",
        ),
        // No glob: certain magic.
        (
            files.join("noext"),
            b"MAGIC bytes",
            "",
            "application/x-magiconly magic false true application/octet-stream",
        ),
        // No glob, no magic, text.
        (
            files.join("words"),
            b"plain words\n",
            "",
            "text/plain content false true application/octet-stream",
        ),
        // No glob, no magic, binary.
        (
            files.join("blob"),
            b"\x00\xff\x00\xfe",
            "",
            "application/octet-stream fallback true true",
        ),
        // Empty is zero-size, whatever its name says (as GIO reports it).
        (
            files.join("empty.mxt"),
            b"",
            "",
            "application/x-zerosize content false true application/octet-stream",
        ),
        (
            files.join("empty"),
            b"",
            "",
            "application/x-zerosize content false true application/octet-stream",
        ),
        // content:false never reads: the magic-only file is unknown.
        (
            files.join("noext2"),
            b"MAGIC bytes",
            ", {content: false}",
            "application/octet-stream fallback true true",
        ),
    ];
    for (path, bytes, opts, expected) in &cases {
        std::fs::write(path, bytes).unwrap();
        let out = run(&probe(path, opts)).await.unwrap();
        assert_eq!(out.trim_end(), *expected, "{}", path.display());
    }

    // A directory is typed by stat.
    let out = run(&probe(&files, "")).await.unwrap();
    assert_eq!(out.trim_end(), "inode/directory stat false true");

    // A missing path is typed by its name alone.
    let out = run(&probe(&files.join("missing.mxt"), "")).await.unwrap();
    assert_eq!(
        out.trim_end(),
        "text/x-mixtest glob false false text/x-parent,text/plain,application/octet-stream"
    );

    // A symlink is followed.
    let link = files.join("link");
    std::os::unix::fs::symlink(files.join("noext"), &link).unwrap();
    let out = run(&probe(&link, "")).await.unwrap();
    assert_eq!(
        out.trim_end(),
        "application/x-magiconly magic false true application/octet-stream"
    );

    // A changed XDG_DATA_DIRS is a different database.
    let other = test_dir("other");
    std::fs::create_dir_all(other.join("mime")).unwrap();
    std::fs::write(other.join("mime/globs2"), "50:text/x-other:*.mxt\n").unwrap();
    // SAFETY: as above, this test alone owns the environment.
    unsafe {
        std::env::set_var("XDG_DATA_DIRS", &other);
    }
    let out = run(&probe(&files.join("a.mxt"), ", {content: false}"))
        .await
        .unwrap();
    assert_eq!(
        out.trim_end(),
        "text/x-other glob false true text/plain,application/octet-stream"
    );
    // SAFETY: as above.
    unsafe {
        std::env::set_var("XDG_DATA_DIRS", &root);
    }

    // Bad options are refused.
    let err = run(&probe(&files.join("a.mxt"), ", {sniff: true}"))
        .await
        .unwrap_err();
    assert!(err.contains("unknown option 'sniff'"), "{err}");
    let err = run(&probe(&files.join("a.mxt"), ", \"yes\""))
        .await
        .unwrap_err();
    assert!(
        err.contains("TYPE_MISMATCH") || err.contains("map"),
        "{err}"
    );
}
