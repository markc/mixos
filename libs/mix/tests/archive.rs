// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `tar_list` / `tar_unpack` / `tar_pack` tests — the round-trip contract
//! plus the ADVERSARIAL set: every archive here that escapes, lies or bombs
//! must be refused by name, and the refusal is the assertion. Malicious
//! archives are built with the raw `tar` crate as PLAIN tar (codec "none"):
//! the validation layer is codec-independent; the codec layers are covered
//! by the builtin's own round-trips (zstd/gzip/none) and the corruption
//! cases (truncation, trailing data).
#![cfg(feature = "archive")]

use mix::value::Value;
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tar::{Builder, Header};

/// Run `source` and return the program's result value rendered briefly.
async fn run_ok(source: &str) -> Value {
    match mix::run(source).await {
        Ok(v) => v,
        Err(e) => panic!("script failed: {e}\nsource: {source}"),
    }
}

async fn run_err(source: &str) -> String {
    match mix::run(source).await {
        Ok(v) => panic!("expected an error, got {v:?}\nsource: {source}"),
        Err(e) => e.to_string(),
    }
}

/// Like `run_err`, but returns the structured error code (empty when the
/// error carries none) so a test can assert the stable code, not the prose.
async fn run_err_code(source: &str) -> (String, String) {
    match mix::run(source).await {
        Ok(v) => panic!("expected an error, got {v:?}\nsource: {source}"),
        Err(e) => (
            e.info().map(|i| i.code.clone()).unwrap_or_default(),
            e.to_string(),
        ),
    }
}

/// `setxattr(name)` on `path` with `value`.
fn set_xattr(path: &std::path::Path, name: &std::ffi::CStr, value: &[u8]) -> std::io::Result<()> {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    let r = unsafe {
        libc::setxattr(
            c.as_ptr(),
            name.as_ptr(),
            value.as_ptr() as *const libc::c_void,
            value.len(),
            0,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Probe whether this filesystem accepts `name` on a scratch file in `d`.
/// Returns false after printing the skip line when it refuses the xattr with
/// an errno meaning "not supported here"; any other failure panics.
fn xattr_supported(d: &std::path::Path, name: &std::ffi::CStr, value: &[u8]) -> bool {
    fs::create_dir_all(d).unwrap();
    let label = name.to_string_lossy().into_owned();
    let probe = d.join(format!("probe-{label}"));
    fs::write(&probe, b"probe").unwrap();
    match set_xattr(&probe, name, value) {
        Ok(()) => true,
        Err(e)
            if matches!(
                e.raw_os_error(),
                // ENOTSUP and EOPNOTSUPP are the same errno on Linux.
                Some(libc::EINVAL | libc::EPERM | libc::EOPNOTSUPP)
            ) =>
        {
            eprintln!("skipped: {label} xattrs unsupported here ({e})");
            false
        }
        Err(e) => panic!("setxattr probe for {label} failed: {e}"),
    }
}

/// A plain tar at `<d>/<tag>.tar` holding one `prog` (mode 0755, root-owned)
/// that carries the given `SCHILY.xattr.*` PAX records. Written with the tar
/// crate's PAX writer, the same call the packer uses for security.capability.
fn xattr_archive(d: &std::path::Path, tag: &str, records: &[(&str, &[u8])]) -> PathBuf {
    fs::create_dir_all(d).unwrap();
    let arc = d.join(format!("{tag}.tar"));
    let mut b = Builder::new(fs::File::create(&arc).unwrap());
    b.append_pax_extensions(records.iter().copied()).unwrap();
    let body = b"#!/bin/sh\n";
    let mut h = Header::new_gnu();
    h.set_size(body.len() as u64);
    h.set_mode(0o755);
    h.set_uid(0);
    h.set_gid(0);
    h.set_mtime(0);
    b.append_data(&mut h, "prog", &body[..]).unwrap();
    b.into_inner().unwrap().flush().unwrap();
    arc
}

/// Unique temp dir per test.
fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "mix-archive-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&d).unwrap();
    // tar_unpack refuses a group/other-writable dest parent; pin the mode so
    // a permissive umask cannot fail every test.
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    d
}

/// Append ONE raw 512-byte tar header block (hand-packed so hostile names
/// the Builder refuses — absolute, `..` — can still be crafted) plus its
/// data and the two zero end-blocks.
fn append_raw_member(mut w: impl Write, name: &str, data: &[u8]) {
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..108].copy_from_slice(b"0000644\0"); // mode
    h[108..116].copy_from_slice(b"0000000\0"); // uid
    h[116..124].copy_from_slice(b"0000000\0"); // gid
    let size = format!("{:011o}\0", data.len());
    h[124..136].copy_from_slice(size.as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0"); // mtime
    h[148..156].copy_from_slice(b"        "); // chksum placeholder: spaces
    h[156] = b'0'; // typeflag: regular
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    let chk = format!("{:06o}\0 ", sum);
    h[148..156].copy_from_slice(chk.as_bytes());
    w.write_all(&h).unwrap();
    w.write_all(data).unwrap();
    let pad = (512 - data.len() % 512) % 512;
    w.write_all(&vec![0u8; pad]).unwrap();
    w.write_all(&[0u8; 1024]).unwrap(); // two end blocks
}

/// Build a plain-tar archive from (name, bytes, mode) triples.
fn build_plain(path: &std::path::Path, members: &[(&str, &[u8], u32)]) {
    let file = fs::File::create(path).unwrap();
    let mut b = Builder::new(file);
    for (name, data, mode) in members {
        let mut h = Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(*mode);
        h.set_uid(1000);
        h.set_gid(1000);
        h.set_mtime(0);
        h.set_cksum();
        b.append_data(&mut h, name, std::io::Cursor::new(data.to_vec()))
            .unwrap();
    }
    b.into_inner().unwrap().flush().unwrap();
}

// ---------------------------------------------------------------------------
// Round-trips
// ---------------------------------------------------------------------------

#[tokio::test]
async fn roundtrip_all_codecs() {
    let d = tmpdir("rt");
    let src = d.join("src");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("hello.txt"), b"hello archive\n").unwrap();
    fs::write(src.join("sub/deep.bin"), vec![0u8; 100_000]).unwrap();
    fs::set_permissions(src.join("hello.txt"), fs::Permissions::from_mode(0o750)).unwrap();
    std::os::unix::fs::symlink("hello.txt", src.join("link")).unwrap();

    for codec in ["zstd", "gzip", "none"] {
        let arc = d.join(format!("out-{codec}.tar"));
        let pack = run_ok(&format!(
            "tar_pack(\"{}\", \"{}\", {{codec:\"{}\"}})",
            src.display(),
            arc.display(),
            codec
        ))
        .await;
        assert!(matches!(pack, Value::Map(_)), "{codec} pack receipt");
        let list = run_ok(&format!(
            "tar_list(\"{}\", {{codec:\"{}\"}})",
            arc.display(),
            codec
        ))
        .await;
        let names = match list {
            Value::List(ref items) => items
                .iter()
                .map(|v| match v {
                    Value::Map(m) => m.get("name").unwrap().to_mix_string(),
                    other => panic!("entry not a map: {other:?}"),
                })
                .collect::<Vec<_>>(),
            other => panic!("list not a list: {other:?}"),
        };
        assert!(
            names.contains(&"hello.txt".to_string()),
            "{codec}: {names:?}"
        );
        assert!(
            names.contains(&"sub/deep.bin".to_string()),
            "{codec}: {names:?}"
        );
        assert!(names.contains(&"link".to_string()), "{codec}: {names:?}");

        let dest = d.join(format!("dest-{codec}"));
        run_ok(&format!(
            "tar_unpack(\"{}\", \"{}\", {{codec:\"{}\"}})",
            arc.display(),
            dest.display(),
            codec
        ))
        .await;
        assert_eq!(
            fs::read(dest.join("hello.txt")).unwrap(),
            b"hello archive\n"
        );
        assert_eq!(fs::read(dest.join("sub/deep.bin")).unwrap().len(), 100_000);
        assert_eq!(
            fs::symlink_metadata(dest.join("hello.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750
        );
        assert_eq!(
            fs::read_link(dest.join("link")).unwrap().to_string_lossy(),
            "hello.txt"
        );
    }
}

#[tokio::test]
async fn pack_is_deterministic() {
    let d = tmpdir("det");
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a"), b"same").unwrap();
    fs::write(src.join("b"), b"bytes").unwrap();
    let a = d.join("a.tar");
    let b = d.join("b.tar");
    run_ok(&format!(
        "tar_pack(\"{}\", \"{}\")",
        src.display(),
        a.display()
    ))
    .await;
    std::thread::sleep(std::time::Duration::from_millis(1100)); // distinct mtime would break determinism if captured wrongly
    run_ok(&format!(
        "tar_pack(\"{}\", \"{}\")",
        src.display(),
        b.display()
    ))
    .await;
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
}

// ---------------------------------------------------------------------------
// Adversarial: escapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refuse_parent_traversal() {
    let d = tmpdir("trav");
    let arc = d.join("evil.tar");
    let f = fs::File::create(&arc).unwrap();
    append_raw_member(f, "../escape.txt", b"gotcha");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert!(err.contains("..") || err.contains("parent"), "{err}");
    assert!(!d.join("../escape.txt").exists());
}

#[tokio::test]
async fn refuse_absolute_member() {
    let d = tmpdir("abs");
    let arc = d.join("evil.tar");
    let f = fs::File::create(&arc).unwrap();
    append_raw_member(f, "/etc/gotcha", b"no");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert!(err.contains("absolute"), "{err}");
}

#[tokio::test]
async fn refuse_write_through_a_contained_symlink() {
    let d = tmpdir("sym-contained");
    let arc = d.join("evil.tar");
    // A real directory, a CONTAINED symlink to it (valid on its own), then a
    // member whose path traverses the symlink. Only the ancestor guard can
    // refuse this: the link target itself is clean.
    let mut b = Builder::new(fs::File::create(&arc).unwrap());
    let mut dir = Header::new_gnu();
    dir.set_entry_type(tar::EntryType::Directory);
    dir.set_size(0);
    dir.set_mode(0o755);
    dir.set_mtime(0);
    b.append_data(&mut dir, "sub", std::io::empty()).unwrap();
    let mut link = Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    link.set_mode(0o777);
    link.set_mtime(0);
    link.set_link_name("sub").unwrap();
    b.append_data(&mut link, "pivot", std::io::empty()).unwrap();
    let mut file = Header::new_gnu();
    file.set_size(5);
    file.set_mode(0o644);
    file.set_mtime(0);
    b.append_data(&mut file, "pivot/inside", &b"evil\n"[..])
        .unwrap();
    b.into_inner().unwrap().flush().unwrap();
    let dest = d.join("dest");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(err.contains("through symlink 'pivot'"), "{err}");
    assert!(!dest.exists());
    assert_no_staging(&d);
}

#[tokio::test]
async fn refuse_an_escaping_symlink_target_before_any_traversal() {
    let d = tmpdir("sym");
    let arc = d.join("evil.tar");
    // An escaping symlink, then a member whose path traverses it. The target
    // is refused first, so the traversal check never runs (see the
    // contained case above for that guard).
    let file = fs::File::create(&arc).unwrap();
    let mut b = Builder::new(file);
    let mut h = Header::new_gnu();
    h.set_size(0);
    h.set_mode(0o777);
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_mtime(0);
    h.set_cksum();
    h.set_link_name("../../outside").unwrap();
    b.append_data(&mut h, "pivot", std::io::empty()).unwrap();
    let mut h2 = Header::new_gnu();
    h2.set_size(5);
    h2.set_mode(0o644);
    h2.set_mtime(0);
    h2.set_cksum();
    b.append_data(&mut h2, "pivot/inside", std::io::Cursor::new(b"evil\n"))
        .unwrap();
    b.into_inner().unwrap().flush().unwrap();

    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    // The escaping TARGET is refused before the traversal check can bite.
    assert!(
        err.contains("symlink") && (err.contains("escaping") || err.contains("pivot")),
        "{err}"
    );
    assert!(!d.join("../../outside").exists() || fs::read_link(d.join("dest/pivot")).is_err());
}

#[tokio::test]
async fn refuse_hardlink_to_unextracted() {
    let d = tmpdir("hard");
    let arc = d.join("evil.tar");
    let file = fs::File::create(&arc).unwrap();
    let mut b = Builder::new(file);
    let mut h = Header::new_gnu();
    h.set_size(0);
    h.set_mode(0o644);
    h.set_entry_type(tar::EntryType::Link);
    h.set_mtime(0);
    h.set_cksum();
    h.set_link_name("never/extracted").unwrap();
    b.append_data(&mut h, "alias", std::io::empty()).unwrap();
    b.into_inner().unwrap().flush().unwrap();
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert!(err.contains("hardlink"), "{err}");
}

#[tokio::test]
async fn refuse_device_and_fifo_entries() {
    let d = tmpdir("dev");
    for (tag, et) in [
        ("chardev", tar::EntryType::Char),
        ("fifo", tar::EntryType::Fifo),
    ] {
        let arc = d.join(format!("{tag}.tar"));
        let file = fs::File::create(&arc).unwrap();
        let mut b = Builder::new(file);
        let mut h = Header::new_gnu();
        h.set_size(0);
        h.set_mode(0o644);
        h.set_entry_type(et);
        h.set_mtime(0);
        h.set_cksum();
        b.append_data(&mut h, tag, std::io::empty()).unwrap();
        b.into_inner().unwrap().flush().unwrap();
        let err = run_err(&format!(
            "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
            arc.display(),
            d.join("dest").display()
        ))
        .await;
        assert!(err.contains("refusing"), "{tag}: {err}");
    }
}

// ---------------------------------------------------------------------------
// Adversarial: stream integrity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refuse_trailing_data_after_tar_end() {
    let d = tmpdir("tail");
    let arc = d.join("plus.tar");
    build_plain(&arc, &[("a.txt", b"a", 0o644)]);
    // Append a second concatenated tar (non-zero bytes after the first
    // archive's end) — the single-frame policy must refuse.
    let mut f = fs::OpenOptions::new().append(true).open(&arc).unwrap();
    f.write_all(b"SECOND-ARCHIVE-GARBAGE").unwrap();
    drop(f);
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert!(err.contains("non-zero data after"), "{err}");
}

#[tokio::test]
async fn refuse_truncated_gzip() {
    let d = tmpdir("trunc");
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    let mut payload = Vec::with_capacity(50_000);
    for i in 0..50_000u32 {
        payload.push(((i * 31 + (i >> 3) * 7) % 251) as u8);
    }
    fs::write(src.join("data"), &payload).unwrap();
    let arc = d.join("data.tar.gz");
    run_ok(&format!(
        "tar_pack(\"{}\", \"{}\", {{codec:\"gzip\"}})",
        src.display(),
        arc.display()
    ))
    .await;
    let full = fs::read(&arc).unwrap();
    assert!(
        full.len() > 400,
        "gzip payload too small to truncate: {}",
        full.len()
    );
    fs::write(&arc, &full[..full.len() - 200]).unwrap();
    let (code, msg) = run_err_code(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"gzip\"}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert_eq!(code, "ARCHIVE_STREAM_CORRUPT", "{msg}");
}

#[tokio::test]
async fn refuse_corrupt_zstd_checksum() {
    let d = tmpdir("cksum");
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("data"), vec![9u8; 50_000]).unwrap();
    let arc = d.join("data.tar.zst");
    run_ok(&format!(
        "tar_pack(\"{}\", \"{}\")",
        src.display(),
        arc.display()
    ))
    .await;
    // Flip one payload byte well inside the compressed body.
    let mut bytes = fs::read(&arc).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    fs::write(&arc, bytes).unwrap();
    let dest = d.join("dest");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\")",
        arc.display(),
        dest.display()
    ))
    .await;
    // The flipped byte lands wherever the (mtime-dependent) frame puts it:
    // a block the decoder rejects, data that decodes to a garbled tar
    // header, or file bytes the content checksum then refuses. Each is a
    // refusal with nothing committed; the footer-only case is
    // a_zstd_frame_with_a_corrupt_checksum_footer_is_refused.
    assert!(!err.is_empty());
    assert!(!dest.exists(), "a corrupt archive must not commit: {err}");
    assert_no_staging(&d);
}

// ---------------------------------------------------------------------------
// Limits and modes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enforce_max_entries() {
    let d = tmpdir("max");
    let arc = d.join("three.tar");
    build_plain(
        &arc,
        &[("a", b"1", 0o644), ("b", b"2", 0o644), ("c", b"3", 0o644)],
    );
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_entries:2}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert!(err.contains("max_entries"), "{err}");
}

#[tokio::test]
async fn suid_stripped_by_default_kept_opt_in() {
    let d = tmpdir("suid");
    let arc = d.join("s.tar");
    build_plain(&arc, &[("rootish", b"x", 0o4755)]);
    let dest = d.join("dest");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        dest.display()
    ))
    .await;
    let mode = fs::metadata(dest.join("rootish"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o755, "suid must be stripped by default");
    let dest2 = d.join("dest2");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", keep_special_bits:true}})",
        arc.display(),
        dest2.display()
    ))
    .await;
    let mode2 = fs::metadata(dest2.join("rootish"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode2 & 0o7777, 0o4755);
}

#[tokio::test]
async fn refuse_any_existing_dest() {
    let d = tmpdir("dest");
    let arc = d.join("one.tar");
    build_plain(&arc, &[("a", b"1", 0o644)]);
    let dest = d.join("dest");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("existing"), b"keep me").unwrap();
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(err.contains("exists"), "{err}");
    // An empty dir is also refused: extraction is staged, never merged, and
    // a pre-existing dir is indistinguishable from a mount point.
    let empty = d.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let err2 = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        empty.display()
    ))
    .await;
    assert!(err2.contains("exists"), "{err2}");
    assert!(
        empty.read_dir().unwrap().next().is_none(),
        "staging must not leak into a refused dest"
    );
}

#[tokio::test]
async fn unknown_option_refused() {
    let d = tmpdir("opts");
    let arc = d.join("one.tar");
    build_plain(&arc, &[("a", b"1", 0o644)]);
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", frobnicate:true}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert!(err.contains("frobnicate"), "{err}");
}

// ---------------------------------------------------------------------------
// Capability round-trip — root only (security.* xattrs need euid 0);
// skipped in normal dev runs, exercised by the root canary flow instead.
// ---------------------------------------------------------------------------

/// A valid v2 `security.capability` blob (20 bytes): VFS_CAP_REVISION_2 =
/// 0x02000000 in the first word, little-endian, so byte 3 carries the
/// revision. A zero revision is unknown to the kernel and is refused with
/// EINVAL on any filesystem.
fn v2_capability_blob() -> Vec<u8> {
    let mut cap = vec![0u8; 20];
    cap[3] = 0x02;
    cap
}

/// Pack a scratch `src/prog` that carries a `security.capability` xattr into
/// `<d>/cap.tar`. Returns the archive and the blob it carries, or `None` after
/// printing the skip line when this filesystem refuses the xattr. The probe
/// runs on a scratch file in the same directory first.
async fn capability_fixture(d: &std::path::Path) -> Option<(PathBuf, Vec<u8>)> {
    let src = d.join("src");
    fs::create_dir_all(&src).unwrap();
    let prog = src.join("prog");
    fs::write(&prog, b"#!/bin/sh\n").unwrap();
    fs::set_permissions(&prog, fs::Permissions::from_mode(0o755)).unwrap();
    let cap = v2_capability_blob();
    if !xattr_supported(d, c"security.capability", &cap) {
        return None;
    }
    set_xattr(&prog, c"security.capability", &cap)
        .unwrap_or_else(|e| panic!("setxattr failed: {e}"));
    let arc = d.join("cap.tar");
    run_ok(&format!(
        "tar_pack(\"{}\", \"{}\", {{keep_special_bits:true}})",
        src.display(),
        arc.display()
    ))
    .await;
    Some((arc, cap))
}

/// Read xattr `name` from `path`: the value, or the OS error.
fn get_xattr(path: &std::path::Path, name: &std::ffi::CStr) -> std::io::Result<Vec<u8>> {
    let mut got = vec![0u8; 64];
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    let n = unsafe {
        libc::getxattr(
            c.as_ptr(),
            name.as_ptr(),
            got.as_mut_ptr() as *mut libc::c_void,
            got.len(),
        )
    };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    got.truncate(n as usize);
    Ok(got)
}

#[tokio::test]
async fn capability_roundtrip_as_root() {
    if unsafe { libc::geteuid() } != 0 {
        return; // documented skip
    }
    let d = tmpdir("cap");
    let Some((arc, cap)) = capability_fixture(&d).await else {
        return;
    };
    let dest = d.join("dest");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{keep_special_bits:true}})",
        arc.display(),
        dest.display()
    ))
    .await;
    // Opted in: the restored xattr must exist and round-trip byte-exact.
    let got = get_xattr(&dest.join("prog"), c"security.capability")
        .expect("capability xattr must be restored");
    assert_eq!(got, cap, "capability xattr must round-trip");
}

#[tokio::test]
async fn capability_not_restored_by_default() {
    if unsafe { libc::geteuid() } != 0 {
        return; // documented skip
    }
    let d = tmpdir("capdef");
    let cap = v2_capability_blob();
    if !xattr_supported(&d, c"security.capability", &cap) {
        return;
    }
    // The sibling: a non-security xattr in the same archive. Skipped alone
    // when this filesystem refuses user.* xattrs.
    let user_ok = xattr_supported(&d, c"user.mixtest", &b"mix"[..]);
    let mut records: Vec<(&str, &[u8])> =
        vec![("SCHILY.xattr.security.capability", cap.as_slice())];
    if user_ok {
        records.push(("SCHILY.xattr.user.mixtest", &b"mix"[..]));
    }
    let arc = xattr_archive(&d, "capdef", &records);
    let dest = d.join("dest");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        dest.display()
    ))
    .await;
    let prog = dest.join("prog");
    assert!(prog.exists(), "the file itself must still unpack");
    // Policy: security.* is stripped by default, so the capability never lands.
    get_xattr(&prog, c"security.capability")
        .expect_err("default unpack must not restore security.capability");
    // Not "nothing is ever restored": a non-security xattr in the same archive
    // IS restored by default, so the stripping above is the policy alone.
    if user_ok {
        assert_eq!(
            get_xattr(&prog, c"user.mixtest").expect("user.* xattr must be restored by default"),
            b"mix".to_vec()
        );
    }
}

fn assert_no_staging(d: &std::path::Path) {
    for entry in fs::read_dir(d).unwrap() {
        assert!(
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".mixtar-stage-"),
            "rollback leaked a staging directory"
        );
    }
}

fn encode_tar(bytes: &[u8], codec: &str) -> Vec<u8> {
    match codec {
        "none" => bytes.to_vec(),
        "gzip" => {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }
        "zstd" => {
            let mut encoder = structured_zstd::encoding::StreamingEncoder::new(
                Vec::new(),
                structured_zstd::encoding::CompressionLevel::Level(1),
            );
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn trailing_padding_uses_the_same_stream_budget() {
    let d = tmpdir("padding-budget");
    let mut plain = Vec::new();
    append_raw_member(&mut plain, "a", b"x");
    plain.extend_from_slice(&[0; 8192]);
    for codec in ["none", "gzip", "zstd"] {
        let arc = d.join(format!("{codec}.tar"));
        fs::write(&arc, encode_tar(&plain, codec)).unwrap();
        // Parsing consumes 1536 bytes, so this failure must come from draining.
        for limit in [1536, plain.len() - 1] {
            let opts = format!("{{codec:\"{codec}\", max_stream_bytes:{limit}}}");
            let err = run_err(&format!("tar_list(\"{}\", {opts})", arc.display())).await;
            assert!(err.contains("max_stream_bytes"), "{codec}: {err}");
            let dest = d.join(format!("dest-{codec}-{limit}"));
            let err = run_err(&format!(
                "tar_unpack(\"{}\", \"{}\", {opts})",
                arc.display(),
                dest.display()
            ))
            .await;
            assert!(err.contains("max_stream_bytes"), "{codec}: {err}");
            assert!(!dest.exists());
            assert_no_staging(&d);
        }
        let opts = format!("{{codec:\"{codec}\", max_stream_bytes:{}}}", plain.len());
        run_ok(&format!("tar_list(\"{}\", {opts})", arc.display())).await;
        run_ok(&format!(
            "tar_unpack(\"{}\", \"{}\", {opts})",
            arc.display(),
            d.join(format!("exact-{codec}")).display()
        ))
        .await;
    }
}

#[tokio::test]
async fn declared_file_size_is_refused_before_reading_data() {
    let d = tmpdir("file-budget");
    for prefix in [false, true] {
        let arc = d.join(format!("{prefix}.tar"));
        let mut bytes = Vec::new();
        if prefix {
            append_raw_member(&mut bytes, "first", b"123");
            bytes.truncate(1024); // omit end blocks, retain the first file and its padding
        }
        let mut header = Header::new_gnu();
        header.set_path("oversized").unwrap();
        header.set_mode(0o600);
        header.set_size(if prefix { 2 } else { 1 << 30 });
        header.set_cksum();
        bytes.extend_from_slice(header.as_bytes());
        // Deliberately no body: a late size check would report truncated data.
        fs::write(&arc, bytes).unwrap();
        let dest = d.join(format!("dest-{prefix}"));
        let err = run_err(&format!(
            "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_bytes:4}})",
            arc.display(),
            dest.display()
        ))
        .await;
        assert!(
            err.contains("declared size") && err.contains("max_bytes"),
            "{err}"
        );
        assert!(!dest.exists());
        assert_no_staging(&d);
    }
    let arc = d.join("exact.tar");
    build_plain(&arc, &[("first", b"123", 0o600), ("second", b"4", 0o600)]);
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_bytes:4}})",
        arc.display(),
        d.join("exact").display()
    ))
    .await;
    assert_eq!(fs::read(d.join("exact/second")).unwrap(), b"4");
}

#[tokio::test]
async fn extension_sizes_are_bounded_before_buffering() {
    let d = tmpdir("metadata-budget");
    for kind in *b"LKxg" {
        for size in [65_537, 1 << 30] {
            let arc = d.join(format!("{kind}-{size}.tar"));
            let mut header = Header::new_gnu();
            header.set_path("extension").unwrap();
            header.set_entry_type(tar::EntryType::new(kind));
            header.set_size(size);
            header.set_cksum();
            fs::write(&arc, header.as_bytes()).unwrap();
            // A huge stream allowance must not permit a huge metadata buffer.
            let opts = "{codec:\"none\", max_name:1000000, max_stream_bytes:4294967296}";
            let err = run_err(&format!("tar_list(\"{}\", {opts})", arc.display())).await;
            assert!(err.contains("metadata payload exceeds limit"), "{err}");
            let dest = d.join(format!("dest-{kind}-{size}"));
            let err = run_err(&format!(
                "tar_unpack(\"{}\", \"{}\", {opts})",
                arc.display(),
                dest.display()
            ))
            .await;
            assert!(err.contains("metadata payload exceeds limit"), "{err}");
            assert!(!dest.exists());
            assert_no_staging(&d);
        }
    }
    // GNU extension bodies also respect max_name plus one terminating NUL.
    for kind in *b"LK" {
        let arc = d.join(format!("name-{kind}.tar"));
        let mut header = Header::new_gnu();
        header.set_entry_type(tar::EntryType::new(kind));
        header.set_size(10);
        header.set_cksum();
        fs::write(&arc, header.as_bytes()).unwrap();
        let err = run_err(&format!(
            "tar_list(\"{}\", {{codec:\"none\", max_name:8}})",
            arc.display()
        ))
        .await;
        assert!(err.contains("metadata payload exceeds limit"), "{err}");
    }
}

#[tokio::test]
async fn entry_limit_precedes_the_next_extension_body() {
    let d = tmpdir("metadata-entry-limit");
    let arc = d.join("entries.tar");
    let mut bytes = Vec::new();
    append_raw_member(&mut bytes, "first", b"1");
    bytes.truncate(1024);
    let mut header = Header::new_gnu();
    header.set_entry_type(tar::EntryType::XHeader);
    header.set_size(64 << 10);
    header.set_cksum();
    bytes.extend_from_slice(header.as_bytes()); // no extension body
    fs::write(&arc, bytes).unwrap();
    let opts = "{codec:\"none\", max_entries:1}";
    let err = run_err(&format!("tar_list(\"{}\", {opts})", arc.display())).await;
    assert!(err.contains("max_entries"), "{err}");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {opts})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert!(err.contains("max_entries"), "{err}");
    assert_no_staging(&d);
}

#[tokio::test]
async fn bounded_extensions_still_resolve_names_links_and_pax() {
    let d = tmpdir("metadata-boundary");
    let arc = d.join("bounded.tar");
    let long_name = "a".repeat(200);
    let long_target = "b".repeat(200);
    let mut builder = Builder::new(fs::File::create(&arc).unwrap());
    for (kind, value, next_name) in [
        (b'L', long_name.as_str(), "placeholder"),
        (b'K', long_target.as_str(), "link"),
    ] {
        let mut payload = value.as_bytes().to_vec();
        payload.push(0);
        let mut header = Header::new_gnu();
        header.set_entry_type(tar::EntryType::new(kind));
        header.set_size(payload.len() as u64);
        builder
            .append_data(&mut header, "extension", payload.as_slice())
            .unwrap();
        let mut header = Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o600);
        if kind == b'K' {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("placeholder").unwrap();
        }
        builder
            .append_data(&mut header, next_name, std::io::empty())
            .unwrap();
    }
    // Five length digits plus record framing and key consume 15 bytes.
    // This valid PAX payload lands exactly on the independent 64 KiB ceiling.
    builder
        .append_pax_extensions([("comment", vec![b'x'; 65_521].as_slice())])
        .unwrap();
    let mut header = Header::new_gnu();
    header.set_size(0);
    header.set_mode(0o600);
    builder
        .append_data(&mut header, "pax-file", std::io::empty())
        .unwrap();
    builder.into_inner().unwrap().flush().unwrap();
    let list = run_ok(&format!(
        "tar_list(\"{}\", {{codec:\"none\", max_name:200}})",
        arc.display()
    ))
    .await;
    let Value::List(list) = &list else {
        panic!("expected archive list")
    };
    assert_eq!(list.len(), 3);
    let Value::Map(first) = &list[0] else {
        panic!("expected entry map")
    };
    assert_eq!(first["name"].to_mix_string(), long_name);
    let dest = d.join("dest");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_name:200}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(dest.join(&long_name).is_file());
    assert_eq!(
        fs::read_link(dest.join("link")).unwrap(),
        PathBuf::from(long_target)
    );
    assert!(dest.join("pax-file").is_file());
}

#[tokio::test]
async fn pax_size_override_keeps_extension_checks_aligned() {
    let d = tmpdir("pax-size");
    let arc = d.join("size.tar");
    let mut builder = Builder::new(Vec::new());
    builder
        .append_pax_extensions([("size", b"1024".as_slice())])
        .unwrap();
    let mut bytes = builder.into_inner().unwrap();
    bytes.truncate(1024); // PAX header, payload and padding, without end blocks
    let mut header = Header::new_gnu();
    header.set_path("file").unwrap();
    header.set_size(0); // overridden by PAX
    header.set_cksum();
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(&[7; 1024]);
    header.set_entry_type(tar::EntryType::XHeader);
    header.set_size(65_537);
    header.set_cksum();
    bytes.extend_from_slice(header.as_bytes());
    fs::write(&arc, bytes).unwrap();
    let err = run_err(&format!(
        "tar_list(\"{}\", {{codec:\"none\"}})",
        arc.display()
    ))
    .await;
    assert!(err.contains("metadata payload exceeds limit"), "{err}");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_bytes:1023}})",
        arc.display(),
        d.join("dest").display()
    ))
    .await;
    assert!(
        err.contains("declared size") && err.contains("max_bytes"),
        "{err}"
    );
    assert_no_staging(&d);
}

#[tokio::test]
async fn global_pax_ends_the_local_size_override_chain() {
    let d = tmpdir("pax-global-size");
    let arc = d.join("global.tar");
    let mut builder = Builder::new(Vec::new());
    builder
        .append_pax_extensions([("size", b"1048576".as_slice())])
        .unwrap();
    let mut bytes = builder.into_inner().unwrap();
    bytes.truncate(1024);
    for (name, kind) in [("global", b'g'), ("file", b'0')] {
        let mut header = Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_entry_type(tar::EntryType::new(kind));
        header.set_size(0);
        header.set_cksum();
        bytes.extend_from_slice(header.as_bytes());
    }
    let mut header = Header::new_gnu();
    header.set_entry_type(tar::EntryType::XHeader);
    header.set_size(65_537);
    header.set_cksum();
    bytes.extend_from_slice(header.as_bytes());
    fs::write(&arc, bytes).unwrap();
    let err = run_err(&format!(
        "tar_list(\"{}\", {{codec:\"none\"}})",
        arc.display()
    ))
    .await;
    assert!(err.contains("metadata payload exceeds limit"), "{err}");
}

#[tokio::test]
async fn gzip_rejects_unread_input_and_second_members() {
    let d = tmpdir("gzip-tail");
    let mut plain = Vec::new();
    append_raw_member(&mut plain, "a", b"x");
    let first = encode_tar(&plain, "gzip");
    for (tag, tail) in [
        ("garbage", vec![1, 2, 3]),
        ("zeros", vec![0; 32]),
        ("empty-member", encode_tar(&[], "gzip")),
        ("archive-member", first.clone()),
    ] {
        let mut bytes = first.clone();
        bytes.extend(tail);
        // Everything fits within the input buffer: read::GzDecoder used to
        // swallow these tails into its inaccessible internal buffer.
        assert!(bytes.len() < 1 << 20);
        let arc = d.join(format!("{tag}.gz"));
        fs::write(&arc, bytes).unwrap();
        let err = run_err(&format!(
            "tar_list(\"{}\", {{codec:\"gzip\"}})",
            arc.display()
        ))
        .await;
        assert!(err.contains("raw bytes remain"), "{tag}: {err}");
        let dest = d.join(format!("dest-{tag}"));
        let err = run_err(&format!(
            "tar_unpack(\"{}\", \"{}\", {{codec:\"gzip\"}})",
            arc.display(),
            dest.display()
        ))
        .await;
        assert!(err.contains("raw bytes remain"), "{tag}: {err}");
        assert!(!dest.exists());
        assert_no_staging(&d);
    }
}

#[tokio::test]
async fn packing_capability_records_requires_opt_in() {
    let d = tmpdir("pack-capability-policy");
    let cap = v2_capability_blob();
    if !xattr_supported(&d, c"security.capability", &cap) {
        return;
    }
    let src = d.join("src");
    fs::create_dir(&src).unwrap();
    let prog = src.join("prog");
    fs::write(&prog, b"program").unwrap();
    set_xattr(&prog, c"security.capability", &cap).unwrap();
    for keep in [false, true] {
        let arc = d.join(format!("{keep}.tar"));
        let opts = if keep {
            "{codec:\"none\", keep_special_bits:true}"
        } else {
            "{codec:\"none\"}"
        };
        let receipt = run_ok(&format!(
            "tar_pack(\"{}\", \"{}\", {opts})",
            src.display(),
            arc.display()
        ))
        .await;
        let Value::Map(receipt) = &receipt else {
            panic!("expected pack receipt")
        };
        assert_eq!(
            receipt["capabilities"].to_mix_string(),
            if keep { "1" } else { "0" }
        );
        let mut archive = tar::Archive::new(fs::File::open(&arc).unwrap());
        let mut records = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if let Some(pax) = entry.pax_extensions().unwrap() {
                for kv in pax {
                    let kv = kv.unwrap();
                    if kv.key_bytes() == b"SCHILY.xattr.security.capability" {
                        records.push(kv.value_bytes().to_vec());
                    }
                }
            }
        }
        assert_eq!(records, if keep { vec![cap.clone()] } else { Vec::new() });
    }
}

#[tokio::test]
async fn rollback_removes_directories_after_restrictive_metadata() {
    let d = tmpdir("rollback-modes");
    let arc = d.join("modes.tar");
    let mut builder = Builder::new(fs::File::create(&arc).unwrap());
    // An overlong xattr name reliably fails outside the tolerated EPERM case.
    // This file is visited last by the metadata post-pass, after chmod on dirs.
    let key = format!("SCHILY.xattr.user.{}", "x".repeat(300));
    builder
        .append_pax_extensions([(key.as_str(), b"value".as_slice())])
        .unwrap();
    let mut header = Header::new_gnu();
    header.set_size(1);
    header.set_mode(0o600);
    builder
        .append_data(&mut header, "bad-xattr", b"x".as_slice())
        .unwrap();
    for (name, mode) in [("locked", 0), ("locked/readonly", 0o500)] {
        let mut header = Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(mode);
        builder
            .append_data(&mut header, name, std::io::empty())
            .unwrap();
    }
    let mut header = Header::new_gnu();
    header.set_size(1);
    header.set_mode(0o600);
    builder
        .append_data(&mut header, "locked/readonly/file", b"x".as_slice())
        .unwrap();
    builder.into_inner().unwrap().flush().unwrap();
    let dest = d.join("dest");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", numeric_owner:false}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(err.contains("xattr") && err.contains("bad-xattr"), "{err}");
    assert!(
        !err.contains("rollback cleanup"),
        "cleanup should succeed: {err}"
    );
    assert!(!dest.exists());
    assert_no_staging(&d);
}

#[tokio::test]
async fn zstd_initialisation_failures_have_the_stream_corrupt_code() {
    let d = tmpdir("zstd-init");
    let empty = d.join("empty.zst");
    fs::write(&empty, []).unwrap();
    let truncated = d.join("truncated.zst");
    fs::write(&truncated, [0x28, 0xb5, 0x2f, 0xfd, 0x00]).unwrap();
    for (i, path) in [std::path::Path::new("/dev/null"), &empty, &truncated]
        .into_iter()
        .enumerate()
    {
        let (code, msg) = run_err_code(&format!("tar_list(\"{}\")", path.display())).await;
        assert_eq!(code, "ARCHIVE_STREAM_CORRUPT", "{msg}");
        assert!(msg.contains("zstd init"), "{msg}");
        let dest = d.join(format!("dest-{i}"));
        let (code, msg) = run_err_code(&format!(
            "tar_unpack(\"{}\", \"{}\")",
            path.display(),
            dest.display()
        ))
        .await;
        assert_eq!(code, "ARCHIVE_STREAM_CORRUPT", "{msg}");
        assert!(!dest.exists());
        assert_no_staging(&d);
    }
}

#[tokio::test]
async fn pack_refuses_a_symlink_source_directory() {
    let d = tmpdir("pack-root-link");
    let actual = d.join("actual");
    fs::create_dir(&actual).unwrap();
    fs::write(actual.join("file"), b"outside").unwrap();
    let source = d.join("source");
    std::os::unix::fs::symlink(&actual, &source).unwrap();
    for (i, suffix) in ["", "/", "/."].into_iter().enumerate() {
        let arc = d.join(format!("out-{i}.tar"));
        let err = run_err(&format!(
            "tar_pack(\"{}{suffix}\", \"{}\", {{codec:\"none\"}})",
            source.display(),
            arc.display()
        ))
        .await;
        assert!(err.contains("open source directory"), "{err}");
        assert!(!arc.exists());
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn pack_uses_the_queued_directory_fd_after_path_replacement() {
    use std::os::fd::{AsRawFd, FromRawFd};
    let d = tmpdir("pack-directory-race");
    let src = d.join("src");
    let outside = d.join("outside");
    fs::create_dir_all(src.join("a-queued")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(src.join("a-queued/file"), b"inside").unwrap();
    fs::write(outside.join("file"), b"outside").unwrap();
    // Sorted after the directory: compression gives the watcher time to swap
    // its pathname before the queued directory is enumerated.
    let sentinel = src.join("b-sentinel");
    fs::File::create(&sentinel)
        .unwrap()
        .set_len(64 << 20)
        .unwrap();
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    assert!(fd >= 0, "{}", std::io::Error::last_os_error());
    let events = unsafe { fs::File::from_raw_fd(fd) };
    let path = std::ffi::CString::new(sentinel.as_os_str().as_bytes()).unwrap();
    assert!(
        unsafe { libc::inotify_add_watch(events.as_raw_fd(), path.as_ptr(), libc::IN_OPEN) } >= 0
    );
    let worker_src = src.clone();
    let worker_d = d.clone();
    let worker = std::thread::spawn(move || {
        let mut poll = libc::pollfd {
            fd: events.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut poll, 1, 10_000) },
            1,
            "sentinel was never opened"
        );
        fs::rename(worker_src.join("a-queued"), worker_d.join("parked")).unwrap();
        std::os::unix::fs::symlink(&outside, worker_src.join("a-queued")).unwrap();
    });
    let arc = d.join("out.gz");
    let result = mix::run(&format!(
        "tar_pack(\"{}\", \"{}\", {{codec:\"gzip\", level:9}})",
        src.display(),
        arc.display()
    ))
    .await;
    worker.join().unwrap();
    result.expect("packing must keep using the original opened directory");
    assert!(
        fs::symlink_metadata(src.join("a-queued"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let mut archive = tar::Archive::new(flate2::bufread::GzDecoder::new(std::io::BufReader::new(
        fs::File::open(&arc).unwrap(),
    )));
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().as_ref() == std::path::Path::new("a-queued/file") {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            assert_eq!(
                bytes, b"inside",
                "queued path followed its replacement symlink"
            );
            found = true;
        }
    }
    assert!(found, "original queued directory content missing");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn packing_file_replacements_never_reads_a_symlink_target() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let d = tmpdir("pack-file-race");
    let src = d.join("src");
    fs::create_dir(&src).unwrap();
    let outside = d.join("outside");
    let inside = vec![b'I'; 4096];
    fs::write(&outside, vec![b'O'; 4096]).unwrap();
    let victim = src.join("victim");
    let parked = d.join("parked");
    fs::write(&victim, &inside).unwrap();
    std::os::unix::fs::symlink(&outside, &parked).unwrap();
    let victim_c = std::ffi::CString::new(victim.as_os_str().as_bytes()).unwrap();
    let parked_c = std::ffi::CString::new(parked.as_os_str().as_bytes()).unwrap();
    let exchange = move || unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            victim_c.as_ptr(),
            libc::AT_FDCWD,
            parked_c.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    assert_eq!(
        exchange(),
        0,
        "atomic exchange unavailable: {}",
        std::io::Error::last_os_error()
    );
    assert_eq!(exchange(), 0);
    let stop = Arc::new(AtomicBool::new(false));
    let swaps = Arc::new(AtomicU64::new(0));
    let worker_stop = stop.clone();
    let worker_swaps = swaps.clone();
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            assert_eq!(exchange(), 0);
            worker_swaps.fetch_add(1, Ordering::Release);
        }
    });
    while swaps.load(Ordering::Acquire) == 0 {
        std::thread::yield_now();
    }
    let mut packed = Vec::new();
    let mut leaked_outputs = Vec::new();
    for i in 0..96 {
        let arc = d.join(format!("{i}.tar"));
        match mix::run(&format!(
            "tar_pack(\"{}\", \"{}\", {{codec:\"none\"}})",
            src.display(),
            arc.display()
        ))
        .await
        {
            Ok(_) => packed.push(arc),
            Err(_) if arc.exists() => leaked_outputs.push(arc),
            Err(_) => {}
        }
    }
    stop.store(true, Ordering::Release);
    worker.join().unwrap();
    assert!(swaps.load(Ordering::Acquire) > 0);
    assert!(
        leaked_outputs.is_empty(),
        "failed packs leaked output: {leaked_outputs:?}"
    );
    // Successes may contain only the pinned regular inode. Refusals are also
    // valid during mutation; a success must never contain outside bytes.
    for arc in packed {
        let mut archive = tar::Archive::new(fs::File::open(&arc).unwrap());
        let mut entries = archive.entries().unwrap();
        let mut entry = entries.next().expect("victim entry missing").unwrap();
        assert!(entry.header().entry_type().is_file());
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
        assert_eq!(bytes, inside, "packer followed a replacement symlink");
        assert!(entries.next().is_none());
    }
}

/// The receipt map of a successful call.
fn receipt(v: &Value) -> &indexmap::IndexMap<String, Value> {
    match v {
        Value::Map(m) => m,
        other => panic!("expected a receipt map, got {other:?}"),
    }
}

fn no_warnings(r: &indexmap::IndexMap<String, Value>) {
    match &r["warnings"] {
        Value::List(l) => assert!(l.is_empty(), "unexpected warnings: {l:?}"),
        other => panic!("warnings is not a list: {other:?}"),
    }
}

#[tokio::test]
async fn a_pax_global_header_is_metadata_not_a_member() {
    // `git archive` opens with a global header carrying the commit id.
    let d = tmpdir("pax-global");
    let arc = d.join("git.tar");
    let mut b = Builder::new(fs::File::create(&arc).unwrap());
    let payload = b"18 comment=abcdef\n";
    let mut g = Header::new_ustar();
    g.set_entry_type(tar::EntryType::XGlobalHeader);
    g.set_size(payload.len() as u64);
    g.set_mtime(0);
    b.append_data(&mut g, "pax_global_header", &payload[..])
        .unwrap();
    let mut f = Header::new_ustar();
    f.set_size(3);
    f.set_mode(0o644);
    f.set_mtime(0);
    b.append_data(&mut f, "repo/a.txt", &b"abc"[..]).unwrap();
    b.into_inner().unwrap().flush().unwrap();

    let list = run_ok(&format!(
        "tar_list(\"{}\", {{codec:\"none\"}})",
        arc.display()
    ))
    .await;
    let Value::List(items) = &list else {
        panic!("list not a list: {list:?}")
    };
    let kinds: Vec<String> = items
        .iter()
        .map(|v| receipt(v)["kind"].to_mix_string())
        .collect();
    assert_eq!(kinds, ["metadata", "file"]);

    let dest = d.join("dest");
    let out = run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        dest.display()
    ))
    .await;
    let r = receipt(&out);
    assert_eq!(r["metadata"].to_mix_string(), "1");
    assert_eq!(r["files"].to_mix_string(), "1");
    no_warnings(r);
    assert_eq!(fs::read(dest.join("repo/a.txt")).unwrap(), b"abc");
    assert!(!dest.join("pax_global_header").exists());
}

#[tokio::test]
async fn privileged_xattrs_need_their_opt_in() {
    if unsafe { libc::geteuid() } != 0 {
        return; // documented skip: trusted.* needs CAP_SYS_ADMIN
    }
    let d = tmpdir("privileged-xattrs");
    if !xattr_supported(&d, c"trusted.mixtest", b"t") || !xattr_supported(&d, c"user.mixtest", b"u")
    {
        return;
    }
    let arc = xattr_archive(
        &d,
        "privileged",
        &[
            ("SCHILY.xattr.user.mixtest", &b"u"[..]),
            ("SCHILY.xattr.trusted.mixtest", &b"t"[..]),
        ],
    );
    let plain = d.join("plain");
    let out = run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        plain.display()
    ))
    .await;
    assert_eq!(receipt(&out)["xattrs_restored"].to_mix_string(), "1");
    assert_eq!(
        get_xattr(&plain.join("prog"), c"user.mixtest").unwrap(),
        b"u"
    );
    get_xattr(&plain.join("prog"), c"trusted.mixtest")
        .expect_err("trusted.* must not be restored by default");
    // keep_special_bits admits security.capability only, not trusted.*.
    let special = d.join("special");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", keep_special_bits:true}})",
        arc.display(),
        special.display()
    ))
    .await;
    get_xattr(&special.join("prog"), c"trusted.mixtest")
        .expect_err("keep_special_bits must not admit trusted.*");
    let opted = d.join("opted");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", privileged_xattrs:true}})",
        arc.display(),
        opted.display()
    ))
    .await;
    assert_eq!(
        get_xattr(&opted.join("prog"), c"trusted.mixtest").unwrap(),
        b"t"
    );
}

#[tokio::test]
async fn retained_xattr_bytes_are_bounded_across_the_archive() {
    let d = tmpdir("xattr-budget");
    let arc = d.join("many.tar");
    let mut b = Builder::new(fs::File::create(&arc).unwrap());
    // Each record is under the 64 KiB per-payload ceiling; together they are
    // over the budget asked for below.
    let value = vec![b'v'; 40_000];
    for i in 0..3 {
        b.append_pax_extensions([("SCHILY.xattr.user.big", value.as_slice())])
            .unwrap();
        let mut h = Header::new_gnu();
        h.set_size(0);
        h.set_mode(0o644);
        h.set_mtime(0);
        b.append_data(&mut h, format!("f{i}"), std::io::empty())
            .unwrap();
    }
    b.into_inner().unwrap().flush().unwrap();
    let dest = d.join("dest");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_xattr_bytes:100000}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(err.contains("max_xattr_bytes"), "{err}");
    assert!(!dest.exists());
    assert_no_staging(&d);
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_xattr_bytes:2000000000}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(err.contains("max_xattr_bytes must be at most"), "{err}");
}

#[tokio::test]
async fn pack_refuses_an_output_inside_its_source() {
    let d = tmpdir("pack-inside");
    let src = d.join("src");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a"), b"a").unwrap();
    for out in [src.join("out.tar"), src.join("sub/out.tar")] {
        let err = run_err(&format!(
            "tar_pack(\"{}\", \"{}\", {{codec:\"none\"}})",
            src.display(),
            out.display()
        ))
        .await;
        assert!(err.contains("inside the source"), "{err}");
        assert!(!out.exists(), "the refused output must not be created");
    }
}

#[tokio::test]
async fn a_dangling_symlink_at_dest_counts_as_existing() {
    let d = tmpdir("dest-dangling");
    let arc = d.join("one.tar");
    build_plain(&arc, &[("a", b"1", 0o644)]);
    let dest = d.join("dest");
    std::os::unix::fs::symlink(d.join("nowhere"), &dest).unwrap();
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(err.contains("exists"), "{err}");
    assert!(!d.join("nowhere").exists());
    assert_no_staging(&d);
}

#[tokio::test]
async fn bare_relative_paths_use_the_current_directory() {
    let d = tmpdir("bare-relative");
    fs::create_dir_all(d.join("src")).unwrap();
    fs::write(d.join("src/a"), b"a").unwrap();
    // The process cwd is shared by every test thread; only this test uses
    // relative paths, and the previous cwd is restored before asserting.
    let before = std::env::current_dir().unwrap();
    std::env::set_current_dir(&d).unwrap();
    let packed = mix::run("tar_pack(\"src\", \"rel.tar\", {codec:\"none\"})").await;
    let unpacked = mix::run("tar_unpack(\"rel.tar\", \"reldest\", {codec:\"none\"})").await;
    std::env::set_current_dir(before).unwrap();
    packed.expect("tar_pack with a bare output name");
    let r = unpacked.expect("tar_unpack with a bare dest name");
    no_warnings(receipt(&r));
    assert_eq!(fs::read(d.join("reldest/a")).unwrap(), b"a");
}

#[tokio::test]
async fn dest_parent_with_an_untrusted_writer_is_refused() {
    let d = tmpdir("untrusted-parent");
    let arc = d.join("one.tar");
    build_plain(&arc, &[("a", b"1", 0o644)]);
    let parent = d.join("shared");
    fs::create_dir(&parent).unwrap();
    // World-writable without the sticky bit: anyone could swap staging.
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        parent.join("dest").display()
    ))
    .await;
    assert!(err.contains("without the sticky bit"), "{err}");
    assert!(
        parent.read_dir().unwrap().next().is_none(),
        "a refused parent must be left untouched"
    );
    // Group-writable alone is refused too.
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o775)).unwrap();
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        parent.join("dest").display()
    ))
    .await;
    assert!(err.contains("without the sticky bit"), "{err}");
    // With the sticky bit (as /tmp) only an entry's owner can rename it.
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o1777)).unwrap();
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        parent.join("dest").display()
    ))
    .await;
    assert_eq!(fs::read(parent.join("dest/a")).unwrap(), b"1");

    // A parent owned by another user (root can make one) is refused.
    if unsafe { libc::geteuid() } == 0 {
        let foreign = d.join("foreign");
        fs::create_dir(&foreign).unwrap();
        fs::set_permissions(&foreign, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::chown(&foreign, Some(12345), Some(12345)).unwrap();
        let err = run_err(&format!(
            "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
            arc.display(),
            foreign.join("dest").display()
        ))
        .await;
        assert!(err.contains("owned by uid 12345"), "{err}");
        assert!(foreign.read_dir().unwrap().next().is_none());
    }
}

/// A plain tar of empty files at the given paths (no directory entries, so
/// every parent is implicit).
fn implicit_parent_archive(path: &std::path::Path, names: &[String]) {
    let mut b = Builder::new(fs::File::create(path).unwrap());
    for name in names {
        let mut h = Header::new_gnu();
        h.set_size(0);
        h.set_mode(0o644);
        h.set_mtime(0);
        b.append_data(&mut h, name, std::io::empty()).unwrap();
    }
    b.into_inner().unwrap().flush().unwrap();
}

#[tokio::test]
async fn implicit_parent_directories_count_against_max_entries() {
    let d = tmpdir("implicit-dirs");
    let arc = d.join("deep.tar");
    // Three members, each under five directories no entry declares.
    let names: Vec<String> = (0..3).map(|i| format!("a{i}/b/c/d/e/f")).collect();
    implicit_parent_archive(&arc, &names);
    let dest = d.join("dest");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_entries:10}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(err.contains("implicit parent directories"), "{err}");
    assert!(!dest.exists());
    assert_no_staging(&d);
    // Within the limit: 3 members and 15 implicit directories.
    let ok = d.join("ok");
    let out = run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_entries:18}})",
        arc.display(),
        ok.display()
    ))
    .await;
    assert_eq!(receipt(&out)["implicit_dirs"].to_mix_string(), "15");
    assert!(ok.join("a2/b/c/d/e/f").exists());
}

#[tokio::test]
async fn implicit_directories_also_spend_the_member_allowance() {
    // Two implicit parents first, then top-level files: 5 members + 2
    // implicit directories is 7 entries, over max_entries 6.
    let d = tmpdir("implicit-then-flat");
    let arc = d.join("mixed.tar");
    let names: Vec<String> = ["d0/f", "d1/f", "t0", "t1", "t2"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    implicit_parent_archive(&arc, &names);
    let dest = d.join("dest");
    let err = run_err(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\", max_entries:6}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(err.contains("max_entries"), "{err}");
    assert!(!dest.exists());
    assert_no_staging(&d);
}

#[tokio::test]
async fn an_explicit_directory_after_its_implicit_creation_is_accepted() {
    let d = tmpdir("implicit-then-explicit");
    let arc = d.join("order.tar");
    let mut b = Builder::new(fs::File::create(&arc).unwrap());
    let mut file = Header::new_gnu();
    file.set_size(0);
    file.set_mode(0o644);
    file.set_mtime(0);
    b.append_data(&mut file, "x/y", std::io::empty()).unwrap();
    let mut dir = Header::new_gnu();
    dir.set_entry_type(tar::EntryType::Directory);
    dir.set_size(0);
    dir.set_mode(0o755);
    dir.set_mtime(0);
    b.append_data(&mut dir, "x/", std::io::empty()).unwrap();
    b.into_inner().unwrap().flush().unwrap();
    let dest = d.join("dest");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\", {{codec:\"none\"}})",
        arc.display(),
        dest.display()
    ))
    .await;
    assert!(dest.join("x/y").exists());
    assert!(dest.join("x").is_dir());
}

#[tokio::test]
async fn a_zstd_frame_with_a_corrupt_checksum_footer_is_refused() {
    let d = tmpdir("zstd-footer");
    let src = d.join("src");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("a"), b"checksummed content\n").unwrap();
    let arc = d.join("a.tar.zst");
    run_ok(&format!(
        "tar_pack(\"{}\", \"{}\", {{codec:\"zstd\"}})",
        src.display(),
        arc.display()
    ))
    .await;
    // The frame carries a content checksum (header flag bit 2), so its last
    // four bytes are the XXH64 footer and nothing else.
    let mut bytes = fs::read(&arc).unwrap();
    assert_eq!(&bytes[..4], &[0x28, 0xb5, 0x2f, 0xfd], "zstd magic");
    assert_ne!(
        bytes[4] & 0x04,
        0,
        "the packer must emit a content checksum"
    );
    let good = d.join("good");
    run_ok(&format!(
        "tar_unpack(\"{}\", \"{}\")",
        arc.display(),
        good.display()
    ))
    .await;
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    let bad = d.join("bad.tar.zst");
    fs::write(&bad, &bytes).unwrap();
    let (code, msg) = run_err_code(&format!("tar_list(\"{}\")", bad.display())).await;
    assert_eq!(code, "ARCHIVE_STREAM_CORRUPT", "{msg}");
    let dest = d.join("dest");
    let (code, msg) = run_err_code(&format!(
        "tar_unpack(\"{}\", \"{}\")",
        bad.display(),
        dest.display()
    ))
    .await;
    assert_eq!(code, "ARCHIVE_STREAM_CORRUPT", "{msg}");
    assert!(!dest.exists());
    assert_no_staging(&d);
}
