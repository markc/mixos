// SPDX-License-Identifier: MIT OR Apache-2.0
//! `archive` feature — `tar_list` / `tar_unpack` / `tar_pack` builtins.
//!
//! Archives are read and written in-language so that a signed manifest can
//! pin a digest, `hash_file` can check it, and the SAME verified file can be
//! unpacked without an external `tar` or `zstd` binary. That matters inside
//! MixOS machines, where no such tools exist.
//!
//! Codecs: zstd (structured-zstd, pure Rust, RFC 8878 — both directions),
//! gzip (flate2's gzip framing over its default Rust backend), none. Pure
//! Rust throughout: no C codec in the shipped binary. Encoder profile
//! contract: single frame, no dictionaries, preset levels only (their
//! windows stay far under the decoder's ceiling) — see `mix man tar`.
//!
//! Safety posture (why this is not a thin `Archive::unpack` wrapper):
//! extraction is a classic escape hatch. Every member name is validated on
//! the crate-RESOLVED path (GNU longname/PAX smuggling lands here, not on
//! raw headers): no absolute/`..`/`.` components, UTF-8 only. Device, fifo
//! and unknown entry types are refused. Symlink targets are contained and
//! nothing is ever extracted THROUGH a created symlink; hardlinks may only
//! target an earlier regular file (hardlink-to-hardlink is refused). All
//! entry metadata — including directories' — is applied in a children-first
//! post-pass using lchown / utimensat(AT_SYMLINK_NOFOLLOW) so nothing
//! follows a link out of the staging tree. `max_stream_bytes` bounds the
//! DECODED stream including trailing padding. Metadata payloads have a
//! separate small ceiling checked before tar can buffer them.
//! Xattrs are parsed and applied BY HAND: the tar crate's blanket
//! `set_unpack_xattrs` would restore `security.capability` even when the
//! caller asked for suid/sgid stripping, so `keep_special_bits:false` (the
//! default) skips `security.*` entirely — a capability xattr is a privilege
//! grant exactly like a suid bit. Entries land in a private 0700 staging
//! directory (dest must NOT exist) and are renamed into place only after
//! the compressed stream is drained to true EOF: the zstd frame checksum /
//! gzip CRC is actually verified, the decoded tail must be all zero (GNU
//! padding), AND the raw input must be fully consumed (a concatenated
//! second frame never decodes, so only the source position can see it).

use crate::builtins::{opt_invalid, sanitize_for_diag};
use crate::error::{MixError, MixResult};
use crate::value::Value;
use indexmap::IndexMap;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, Builder, EntryType, Header};

fn runtime(msg: String) -> MixError {
    MixError::RuntimeError { span: None, msg }
}

// ---------------------------------------------------------------------------
// Codec + options
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Codec {
    Zstd,
    Gzip,
    None,
}

impl Codec {
    fn as_str(self) -> &'static str {
        match self {
            Codec::Zstd => "zstd",
            Codec::Gzip => "gzip",
            Codec::None => "none",
        }
    }
}

fn parse_codec(caller: &str, v: Option<&Value>) -> Result<Codec, MixError> {
    match v {
        None | Some(Value::Nil) => Ok(Codec::Zstd),
        Some(Value::String(s)) if s == "zstd" => Ok(Codec::Zstd),
        Some(Value::String(s)) if s == "gzip" => Ok(Codec::Gzip),
        Some(Value::String(s)) if s == "none" => Ok(Codec::None),
        other => Err(opt_invalid(
            caller,
            format!(
                "codec must be \"zstd\" (default), \"gzip\" or \"none\", got {}",
                other.map(Value::type_name).unwrap_or("nil")
            ),
        )),
    }
}

fn bool_opt(caller: &str, name: &str, v: Option<&Value>) -> Result<Option<bool>, MixError> {
    match v {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        other => Err(opt_invalid(
            caller,
            format!(
                "{name} must be a bool, got {}",
                other.map(Value::type_name).unwrap_or("nil")
            ),
        )),
    }
}

fn count_opt(caller: &str, name: &str, v: Option<&Value>) -> Result<Option<u64>, MixError> {
    match v {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::Number(n)) if *n >= 1.0 && n.fract() == 0.0 => Ok(Some(*n as u64)),
        other => Err(opt_invalid(
            caller,
            format!(
                "{name} must be a whole number >= 1, got {}",
                other.map(Value::type_name).unwrap_or("nil")
            ),
        )),
    }
}

/// zstd levels 1..=22 (C numbering, `CompressionLevel::Level`). 22 is the
/// cap because its window (2^27) is exactly the pinned builtin decoder's
/// ceiling — anything the builtin PACKS, the builtin can unpack.
const ZSTD_MIN_LEVEL: i32 = 1;
const ZSTD_MAX_LEVEL: i32 = 22;

struct UnpackOpts {
    codec: Codec,
    numeric_owner: bool,
    xattrs: bool,
    keep_special_bits: bool,
    max_entries: u64,
    max_bytes: u64,
    max_name: usize,
    max_stream_bytes: u64,
}

impl Default for UnpackOpts {
    fn default() -> Self {
        UnpackOpts {
            codec: Codec::Zstd,
            numeric_owner: true,
            xattrs: true,
            keep_special_bits: false,
            max_entries: 200_000,
            max_bytes: 64 << 30,
            max_name: 4096,
            // Includes headers, file data, metadata and trailing padding.
            max_stream_bytes: 16 << 30,
        }
    }
}

fn known_keys_check(
    caller: &str,
    m: &IndexMap<String, Value>,
    known: &[&str],
) -> Result<(), MixError> {
    for k in m.keys() {
        if !known.contains(&k.as_str()) {
            return Err(opt_invalid(caller, format!("unknown option '{k}'")));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Verified decode layer
// ---------------------------------------------------------------------------

/// One budget shared by tar parsing and final stream verification.
struct LimitedRead<'a> {
    inner: &'a mut VerifiedDecode,
    remaining: u64,
    caller: String,
}

impl Read for LimitedRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // At the boundary, probe EOF without returning any over-budget byte.
        if self.remaining == 0 {
            let mut probe = [0];
            return match self.inner.read_some(&mut probe)? {
                0 => Ok(0),
                _ => Err(std::io::Error::other(format!(
                    "{}: exceeded max_stream_bytes",
                    self.caller
                ))),
            };
        }
        let len = self.remaining.min(buf.len() as u64) as usize;
        let n = self.inner.read_some(&mut buf[..len])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// Fixed ceiling independent of the decoded-stream allowance. GNU names also
/// obey max_name (plus their terminating NUL). PAX may contain xattr values.
const MAX_METADATA_BYTES: u64 = 64 << 10;

/// Inspect physical tar headers before the tar crate can buffer extensions.
/// Only bounded extension bodies are buffered here; ordinary data streams
/// through. PAX size overrides must also govern physical record boundaries.
struct TarRead<R> {
    inner: R,
    pending: std::io::Cursor<Vec<u8>>,
    payload_remaining: u64,
    pax_size: Option<u64>,
    entries: u64,
    max_entries: u64,
    max_name: usize,
    ended: bool,
}

impl<R: Read> TarRead<R> {
    fn new(inner: R, opts: &UnpackOpts) -> Self {
        Self {
            inner,
            pending: std::io::Cursor::new(Vec::new()),
            payload_remaining: 0,
            pax_size: None,
            entries: 0,
            max_entries: opts.max_entries,
            max_name: opts.max_name,
            ended: false,
        }
    }
}

impl<R: Read> Read for TarRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = self.pending.read(buf)?;
        if n != 0 {
            return Ok(n);
        }
        if self.ended {
            return self.inner.read(buf);
        }
        if self.payload_remaining != 0 {
            let len = self.payload_remaining.min(buf.len() as u64) as usize;
            let n = self.inner.read(&mut buf[..len])?;
            self.payload_remaining -= n as u64;
            return Ok(n);
        }
        let mut header = Header::new_old();
        // A completely empty tar is accepted by tar-rs; partial headers fail.
        if self.inner.read(&mut header.as_mut_bytes()[..1])? == 0 {
            return Ok(0);
        }
        self.inner.read_exact(&mut header.as_mut_bytes()[1..])?;
        if header.as_bytes().iter().all(|b| *b == 0) {
            self.ended = true;
            self.pending = std::io::Cursor::new(header.as_bytes().to_vec());
            return self.pending.read(buf);
        }
        if self.entries >= self.max_entries {
            return Err(std::io::Error::other("exceeded max_entries"));
        }
        let kind = header.entry_type();
        let recognized = header.as_gnu().is_some() || header.as_ustar().is_some();
        let local_pax = recognized && kind.is_pax_local_extensions();
        let gnu_name_kind = kind.is_gnu_longname() || kind.is_gnu_longlink();
        let gnu_name = recognized && gnu_name_kind;
        let extension =
            gnu_name_kind || kind.is_pax_local_extensions() || kind.is_pax_global_extensions();
        if kind.is_gnu_sparse() {
            return Err(std::io::Error::other("refusing GNU sparse entry"));
        }
        let mut size = header.entry_size()?;
        if !extension {
            size = self.pax_size.unwrap_or(size);
        }
        // tar-rs returns global PAX (and unrecognised extension headers) as
        // entries, ending the current local-extension chain. They do not use
        // its size override, and it must not leak into a later record.
        if !gnu_name && !local_pax {
            self.pax_size = None;
        }
        let padded = size
            .checked_add(511)
            .ok_or_else(|| std::io::Error::other("tar size overflow"))?
            & !511;
        let mut pending = header.as_bytes().to_vec();
        if extension {
            let ceiling = if gnu_name_kind {
                MAX_METADATA_BYTES.min(self.max_name.saturating_add(1) as u64)
            } else {
                MAX_METADATA_BYTES
            };
            if size > ceiling {
                return Err(std::io::Error::other(format!(
                    "metadata payload exceeds limit ({size} > {ceiling}; GNU names obey max_name)"
                )));
            }
            pending.resize(512 + size as usize, 0);
            self.inner.read_exact(&mut pending[512..])?;
            if local_pax {
                // Match tar-rs: first size record wins, malformed records or
                // values stop numeric lookup without overriding the header.
                self.pax_size = None;
                for record in tar::PaxExtensions::new(&pending[512..]) {
                    let Ok(record) = record else { break };
                    if record.key_bytes() == b"size" {
                        self.pax_size = record.value().ok().and_then(|v| v.parse().ok());
                        break;
                    }
                }
            }
            self.payload_remaining = padded - size;
        } else {
            self.payload_remaining = padded;
        }
        if !gnu_name && !local_pax {
            self.entries += 1;
        }
        self.pending = std::io::Cursor::new(pending);
        self.pending.read(buf)
    }
}

enum VerifiedDecode {
    Zstd {
        dec: Box<
            structured_zstd::decoding::StreamingDecoder<
                BufReader<File>,
                structured_zstd::decoding::FrameDecoder,
            >,
        >,
    },
    Gzip {
        dec: flate2::bufread::GzDecoder<BufReader<File>>,
    },
    None {
        src: BufReader<File>,
    },
}

/// A failure raised by the zstd or gzip decoder itself (corrupt, truncated or
/// checksum-failed stream). It is tagged by type, not by message, so the
/// classification below never reads backend prose.
#[derive(Debug)]
struct CodecFailure(String);

impl std::fmt::Display for CodecFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecFailure {}

/// Stable error code for a corrupt or truncated archive stream.
const STREAM_CORRUPT: &str = "ARCHIVE_STREAM_CORRUPT";

/// Build the error for a failed read or write: a decoder failure becomes the
/// structured `ARCHIVE_STREAM_CORRUPT`, anything else stays a runtime error.
/// The message text is the same either way.
fn io_failure(msg: String, e: &std::io::Error) -> MixError {
    if e.get_ref().is_some_and(|inner| inner.is::<CodecFailure>()) {
        MixError::structured(STREAM_CORRUPT, msg)
    } else {
        runtime(msg)
    }
}

impl VerifiedDecode {
    fn open(path: &Path, codec: Codec, caller: &str) -> Result<Self, MixError> {
        let file = File::open(path)
            .map_err(|e| runtime(format!("{caller}: open '{}': {e}", path.display())))?;
        let br = BufReader::with_capacity(1 << 20, file);
        Ok(match codec {
            Codec::Zstd => VerifiedDecode::Zstd {
                dec: Box::new(
                    structured_zstd::decoding::StreamingDecoder::new(br).map_err(|e| {
                        MixError::structured(
                            STREAM_CORRUPT,
                            format!("{caller}: zstd init '{}': {e}", path.display()),
                        )
                    })?,
                ),
            },
            Codec::Gzip => VerifiedDecode::Gzip {
                dec: flate2::bufread::GzDecoder::new(br),
            },
            Codec::None => VerifiedDecode::None { src: br },
        })
    }

    fn read_some(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        fn codec(e: std::io::Error) -> std::io::Error {
            std::io::Error::new(e.kind(), CodecFailure(e.to_string()))
        }
        match self {
            VerifiedDecode::Zstd { dec } => dec.read(buf).map_err(codec),
            VerifiedDecode::Gzip { dec } => dec.read(buf).map_err(codec),
            VerifiedDecode::None { src } => src.read(buf),
        }
    }

    /// After decoded EOF, ensure the single-member decoder left no raw input.
    fn verify_raw_end(&mut self, caller: &str) -> MixResult<()> {
        let reader = match self {
            VerifiedDecode::Zstd { dec } => dec.get_mut(),
            VerifiedDecode::Gzip { dec } => dec.get_mut(),
            VerifiedDecode::None { src } => src,
        };
        let leftover = reader
            .fill_buf()
            .map_err(|e| io_failure(format!("{caller}: tail read: {e}"), &e))?;
        if !leftover.is_empty() {
            return Err(runtime(format!(
                "{caller}: {} raw bytes remain after the decoded stream ended — \
                 concatenated frames/members are refused",
                leftover.len()
            )));
        }
        Ok(())
    }
}

impl LimitedRead<'_> {
    /// Drain within the same budget as tar parsing, checking padding, codec
    /// checksum/CRC and finally the raw input for additional frames/members.
    fn drain(&mut self, caller: &str) -> MixResult<u64> {
        let mut drained: u64 = 0;
        let mut buf = vec![0u8; 64 << 10];
        loop {
            let n = self.read(&mut buf).map_err(|e| {
                io_failure(format!("{caller}: stream verification failed: {e}"), &e)
            })?;
            if n == 0 {
                break;
            }
            drained += n as u64;
            if buf[..n].iter().any(|&b| b != 0) {
                return Err(runtime(format!(
                    "{caller}: non-zero data after the tar end-of-archive (bytes {}..{}) — \
                     single-frame artifacts only",
                    drained - n as u64,
                    drained
                )));
            }
        }
        self.inner.verify_raw_end(caller)?;
        Ok(drained)
    }
}

// ---------------------------------------------------------------------------
// Member validation (crate-RESOLVED paths)
// ---------------------------------------------------------------------------

fn clean_member(caller: &str, raw: &Path, max_name: usize) -> Result<PathBuf, MixError> {
    let bytes = raw.as_os_str().as_bytes();
    if bytes.len() > max_name {
        return Err(runtime(format!(
            "{caller}: member name exceeds max_name ({} > {})",
            bytes.len(),
            max_name
        )));
    }
    if bytes.is_empty() {
        return Err(runtime(format!("{caller}: empty member name")));
    }
    // Names are UTF-8 only in this builtin: silent mangling of invalid
    // bytes (to_string_lossy) would lie about what got extracted.
    if raw.to_str().is_none() {
        return Err(runtime(format!(
            "{caller}: member name is not valid UTF-8: '{}'",
            sanitize_for_diag(&String::from_utf8_lossy(bytes))
        )));
    }
    let mut out = PathBuf::new();
    for c in raw.components() {
        match c {
            Component::Normal(part) => {
                let pb = part.as_bytes();
                if pb == b"." || pb == b".." || pb.is_empty() {
                    return Err(runtime(format!(
                        "{caller}: refusing member name '{}': '.' or '..' component",
                        raw.display()
                    )));
                }
                out.push(part);
            }
            _ => {
                return Err(runtime(format!(
                    "{caller}: refusing member name '{}': absolute or parent component",
                    raw.display()
                )));
            }
        }
    }
    Ok(out)
}

fn clean_link_target(
    caller: &str,
    name: &str,
    target: &std::ffi::OsStr,
    max_name: usize,
) -> Result<PathBuf, MixError> {
    clean_member(caller, Path::new(target), max_name).map_err(|_| {
        runtime(format!(
            "{caller}: refusing symlink '{}' with absolute or escaping target '{}'",
            name,
            target.to_string_lossy()
        ))
    })
}

// ---------------------------------------------------------------------------
// tar_list
// ---------------------------------------------------------------------------

pub fn builtin_tar_list(args: Vec<Value>) -> MixResult<Option<Value>> {
    let caller = "tar_list()";
    if args.is_empty() || args.len() > 2 {
        return Err(runtime(format!(
            "{caller} expects 1 or 2 args (path[, opts]), got {}",
            args.len()
        )));
    }
    let path = args[0].to_mix_string();
    let mut opts = UnpackOpts::default();
    match args.get(1) {
        None | Some(Value::Nil) => {}
        Some(Value::Map(m)) => {
            if let Some(v) = m.get("codec") {
                opts.codec = parse_codec(caller, Some(v))?;
            }
            opts.max_stream_bytes =
                count_opt(caller, "max_stream_bytes", m.get("max_stream_bytes"))?
                    .unwrap_or(opts.max_stream_bytes);
            opts.max_name = count_opt(caller, "max_name", m.get("max_name"))?
                .unwrap_or(opts.max_name as u64) as usize;
            opts.max_entries =
                count_opt(caller, "max_entries", m.get("max_entries"))?.unwrap_or(opts.max_entries);
            known_keys_check(
                caller,
                m,
                &["codec", "max_stream_bytes", "max_name", "max_entries"],
            )?;
        }
        Some(other) => {
            return Err(opt_invalid(
                caller,
                format!("options must be a map or nil, got {}", other.type_name()),
            ));
        }
    }

    let mut dec = VerifiedDecode::open(Path::new(&path), opts.codec, caller)?;
    let mut out: Vec<Value> = Vec::new();
    let mut count: u64 = 0;
    let mut limited = LimitedRead {
        inner: &mut dec,
        remaining: opts.max_stream_bytes,
        caller: caller.to_string(),
    };
    {
        let mut archive = Archive::new(TarRead::new(&mut limited, &opts));
        let iter = archive
            .entries()
            .map_err(|e| io_failure(format!("{caller}: {e}"), &e))?;
        for entry in iter {
            let entry = entry.map_err(|e| io_failure(format!("{caller}: {e}"), &e))?;
            count += 1;
            if count > opts.max_entries {
                return Err(runtime(format!(
                    "{caller}: exceeded max_entries ({})",
                    opts.max_entries
                )));
            }
            let header = entry.header();
            let raw = entry
                .path()
                .map_err(|e| runtime(format!("{caller}: member name: {e}")))?
                .to_path_buf();
            let name = clean_member(caller, &raw, opts.max_name)?;
            let mut map = IndexMap::new();
            map.insert(
                "name".to_string(),
                Value::String(name.to_string_lossy().to_string()),
            );
            map.insert("size".to_string(), Value::Number(entry.size() as f64));
            map.insert(
                "mode".to_string(),
                Value::Number(header.mode().unwrap_or(0) as f64),
            );
            map.insert(
                "uid".to_string(),
                Value::Number(header.uid().unwrap_or(0) as f64),
            );
            map.insert(
                "gid".to_string(),
                Value::Number(header.gid().unwrap_or(0) as f64),
            );
            map.insert(
                "mtime".to_string(),
                Value::Number(header.mtime().unwrap_or(0) as f64),
            );
            map.insert(
                "kind".to_string(),
                Value::String(kind_str(header.entry_type()).to_string()),
            );
            out.push(Value::Map(std::rc::Rc::new(map)));
        }
    }
    limited.drain(caller)?;
    Ok(Some(Value::List(std::rc::Rc::new(out))))
}

fn kind_str(t: EntryType) -> &'static str {
    match t {
        EntryType::Regular | EntryType::Continuous => "file",
        EntryType::Directory => "dir",
        EntryType::Symlink => "symlink",
        EntryType::Link => "hardlink",
        _ => "other",
    }
}

// ---------------------------------------------------------------------------
// tar_unpack
// ---------------------------------------------------------------------------

pub fn builtin_tar_unpack(args: Vec<Value>) -> MixResult<Option<Value>> {
    let caller = "tar_unpack()";
    if args.len() < 2 || args.len() > 3 {
        return Err(runtime(format!(
            "{caller} expects 2 or 3 args (path, dest[, opts]), got {}",
            args.len()
        )));
    }
    let path = args[0].to_mix_string();
    let dest = args[1].to_mix_string();
    let mut opts = UnpackOpts::default();
    match args.get(2) {
        None | Some(Value::Nil) => {}
        Some(Value::Map(m)) => {
            if let Some(v) = m.get("codec") {
                opts.codec = parse_codec(caller, Some(v))?;
            }
            if let Some(v) = bool_opt(caller, "numeric_owner", m.get("numeric_owner"))? {
                opts.numeric_owner = v;
            }
            if let Some(v) = bool_opt(caller, "xattrs", m.get("xattrs"))? {
                opts.xattrs = v;
            }
            if let Some(v) = bool_opt(caller, "keep_special_bits", m.get("keep_special_bits"))? {
                opts.keep_special_bits = v;
            }
            opts.max_entries =
                count_opt(caller, "max_entries", m.get("max_entries"))?.unwrap_or(opts.max_entries);
            opts.max_bytes =
                count_opt(caller, "max_bytes", m.get("max_bytes"))?.unwrap_or(opts.max_bytes);
            opts.max_name = count_opt(caller, "max_name", m.get("max_name"))?
                .unwrap_or(opts.max_name as u64) as usize;
            opts.max_stream_bytes =
                count_opt(caller, "max_stream_bytes", m.get("max_stream_bytes"))?
                    .unwrap_or(opts.max_stream_bytes);
            known_keys_check(
                caller,
                m,
                &[
                    "codec",
                    "numeric_owner",
                    "xattrs",
                    "keep_special_bits",
                    "max_entries",
                    "max_bytes",
                    "max_name",
                    "max_stream_bytes",
                ],
            )?;
        }
        Some(other) => {
            return Err(opt_invalid(
                caller,
                format!("options must be a map or nil, got {}", other.type_name()),
            ));
        }
    }

    let dest_path = Path::new(&dest);
    if dest_path.exists() {
        return Err(runtime(format!(
            "{caller}: dest '{dest}' exists; refusing (extraction is staged, never merged)"
        )));
    }
    let parent = dest_path
        .parent()
        .ok_or_else(|| runtime(format!("{caller}: dest has no parent directory")))?;
    if !parent.is_dir() {
        return Err(runtime(format!(
            "{caller}: dest parent '{}' does not exist",
            parent.display()
        )));
    }

    let staging = parent.join(format!(
        ".mixtar-stage-{}-{}",
        std::process::id(),
        STAGE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&staging).map_err(|e| runtime(format!("{caller}: staging: {e}")))?;
    // Private staging regardless of umask: content exists briefly before
    // the header mode is applied.
    let _ = fs::set_permissions(&staging, fs::Permissions::from_mode(0o700));

    match unpack_staged(&path, &staging, &opts, caller) {
        Ok(receipt) => {
            if let Err(e) = fs::rename(&staging, dest_path) {
                return Err(rollback_staging(
                    &staging,
                    runtime(format!("{caller}: commit '{dest}': {e}")),
                ));
            }
            Ok(Some(Value::Map(std::rc::Rc::new(receipt))))
        }
        Err(e) => Err(rollback_staging(&staging, e)),
    }
}

static STAGE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn rollback_staging(staging: &Path, original: MixError) -> MixError {
    // Restore parents before inspecting children: the metadata post-pass may
    // already have made a directory 0000 or read-only. Never chmod symlinks.
    let restore = (|| -> std::io::Result<()> {
        let mut queue = vec![staging.to_path_buf()];
        while let Some(dir) = queue.pop() {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
            for child in fs::read_dir(&dir)? {
                let child = child?;
                if child.file_type()?.is_dir() {
                    queue.push(child.path());
                }
            }
        }
        Ok(())
    })();
    let remove = fs::remove_dir_all(staging);
    match (restore, remove) {
        (Ok(()), Ok(())) => original,
        (restore, remove) => runtime(format!(
            "{original}; rollback cleanup '{}': restore modes: {:?}; remove: {:?}",
            staging.display(),
            restore.err(),
            remove.err()
        )),
    }
}

/// Never write a byte beyond the remaining file-data budget, even if a reader
/// returns more data than its declared size. The final probe detects excess.
fn copy_file_bounded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    mut remaining: u64,
) -> std::io::Result<u64> {
    let mut buf = [0u8; 64 << 10];
    let mut written = 0;
    while remaining != 0 {
        let len = remaining.min(buf.len() as u64) as usize;
        let n = reader.read(&mut buf[..len])?;
        if n == 0 {
            return Ok(written);
        }
        writer.write_all(&buf[..n])?;
        remaining -= n as u64;
        written += n as u64;
    }
    if reader.read(&mut buf[..1])? != 0 {
        return Err(std::io::Error::other("exceeded max_bytes while copying"));
    }
    Ok(written)
}

/// Metadata deferred to a post-pass: directory mtime/owner must be applied
/// children-first, and symlink metadata must never follow the link.
struct DeferredMeta {
    rel: PathBuf,
    mode: u32,
    uid: u32,
    gid: u32,
    mtime: i64,
    is_symlink: bool,
    /// `SCHILY.xattr.*` pairs collected from the entry's PAX header before its
    /// data stream was consumed. Applied last, after owner and mode.
    xattrs: Vec<(Vec<u8>, Vec<u8>)>,
}

fn unpack_staged(
    path: &str,
    staging: &Path,
    opts: &UnpackOpts,
    caller: &str,
) -> MixResult<IndexMap<String, Value>> {
    let mut dec = VerifiedDecode::open(Path::new(path), opts.codec, caller)?;
    let mut files: u64 = 0;
    let mut dirs: u64 = 0;
    let mut symlinks: u64 = 0;
    let mut hardlinks: u64 = 0;
    let mut bytes: u64 = 0;
    let mut xattrs_restored: u64 = 0;
    let mut entry_count: u64 = 0;
    let mut symlinks_created: HashSet<PathBuf> = HashSet::new();
    let mut extracted_files: HashSet<PathBuf> = HashSet::new();
    let mut deferred: Vec<DeferredMeta> = Vec::new();
    let mut limited = LimitedRead {
        inner: &mut dec,
        remaining: opts.max_stream_bytes,
        caller: caller.to_string(),
    };
    {
        let mut archive = Archive::new(TarRead::new(&mut limited, opts));
        let iter = archive
            .entries()
            .map_err(|e| io_failure(format!("{caller}: {e}"), &e))?;

        for entry in iter {
            let mut entry = entry.map_err(|e| io_failure(format!("{caller}: {e}"), &e))?;
            entry_count += 1;
            if entry_count > opts.max_entries {
                return Err(runtime(format!(
                    "{caller}: exceeded max_entries ({})",
                    opts.max_entries
                )));
            }
            let header = entry.header().clone();
            let raw_path = entry
                .path()
                .map_err(|e| runtime(format!("{caller}: member name: {e}")))?
                .to_path_buf();
            let rel = clean_member(caller, &raw_path, opts.max_name)?;
            let target_abs = staging.join(&rel);

            // Never extract THROUGH a symlink this archive created.
            for ancestor in rel.ancestors().skip(1) {
                if !ancestor.as_os_str().is_empty() && symlinks_created.contains(ancestor) {
                    return Err(runtime(format!(
                        "{caller}: refusing to extract '{}' through symlink '{}'",
                        rel.display(),
                        ancestor.display()
                    )));
                }
            }

            let mode = header.mode().unwrap_or(0o644);
            let masked = if opts.keep_special_bits {
                mode & 0o7777
            } else {
                mode & !0o6000 & 0o7777
            };
            let mut meta = DeferredMeta {
                rel: rel.clone(),
                mode: masked,
                uid: header.uid().unwrap_or(0) as u32,
                gid: header.gid().unwrap_or(0) as u32,
                mtime: header.mtime().unwrap_or(0) as i64,
                is_symlink: false,
                xattrs: Vec::new(),
            };

            match header.entry_type() {
                EntryType::Directory => {
                    // 0700 & umask during extraction; the post-pass applies the
                    // real (masked) mode so nothing is ever wider than asked.
                    fs::create_dir(&target_abs).map_err(|e| {
                        runtime(format!("{caller}: mkdir '{}': {e}", rel.display()))
                    })?;
                    dirs += 1;
                    deferred.push(meta);
                }
                EntryType::Regular | EntryType::Continuous => {
                    let remaining = opts.max_bytes - bytes;
                    if entry.size() > remaining {
                        return Err(runtime(format!(
                            "{caller}: declared size of '{}' exceeds remaining max_bytes ({remaining})",
                            rel.display()
                        )));
                    }
                    if let Some(parent) = target_abs.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| runtime(format!("{caller}: mkdir parent: {e}")))?;
                    }
                    if opts.xattrs {
                        meta.xattrs = collect_pax_xattrs(&mut entry, opts);
                    }
                    entry.set_preserve_mtime(true);
                    let mut out = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&target_abs)
                        .map_err(|e| {
                            runtime(format!("{caller}: create '{}': {e}", rel.display()))
                        })?;
                    let written =
                        copy_file_bounded(&mut entry, &mut out, remaining).map_err(|e| {
                            io_failure(format!("{caller}: write '{}': {e}", rel.display()), &e)
                        })?;
                    bytes += written;
                    files += 1;
                    extracted_files.insert(rel.clone());
                    deferred.push(meta);
                }
                EntryType::Symlink => {
                    let target = entry
                        .link_name()
                        .map_err(|e| runtime(format!("{caller}: link name: {e}")))?
                        .ok_or_else(|| {
                            runtime(format!(
                                "{caller}: symlink '{}' has no target",
                                rel.display()
                            ))
                        })?
                        .clone();
                    let clean = clean_link_target(
                        caller,
                        &rel.to_string_lossy(),
                        target.as_os_str(),
                        opts.max_name,
                    )?;
                    std::os::unix::fs::symlink(&clean, &target_abs).map_err(|e| {
                        runtime(format!("{caller}: symlink '{}': {e}", rel.display()))
                    })?;
                    symlinks += 1;
                    symlinks_created.insert(rel.clone());
                    let mut m = meta;
                    m.is_symlink = true;
                    deferred.push(m);
                }
                EntryType::Link => {
                    let target = entry
                        .link_name()
                        .map_err(|e| runtime(format!("{caller}: link name: {e}")))?
                        .ok_or_else(|| {
                            runtime(format!(
                                "{caller}: hardlink '{}' has no target",
                                rel.display()
                            ))
                        })?
                        .clone();
                    let clean_target = clean_member(caller, &target, opts.max_name)?;
                    if !extracted_files.contains(&clean_target) {
                        return Err(runtime(format!(
                            "{caller}: hardlink '{}' targets '{}' which is not an earlier \
                         regular file in this archive",
                            rel.display(),
                            clean_target.display()
                        )));
                    }
                    fs::hard_link(staging.join(&clean_target), &target_abs).map_err(|e| {
                        runtime(format!("{caller}: hardlink '{}': {e}", rel.display()))
                    })?;
                    hardlinks += 1;
                }
                other => {
                    return Err(runtime(format!(
                        "{caller}: refusing member '{}' of type {other:?} \
                     (device/fifo/special entries are not extracted)",
                        rel.display()
                    )));
                }
            }
        }
    }
    let trailing = limited.drain(caller)?;

    // Deferred metadata, children-first so directory mtimes stick, never
    // following symlinks (lchown / utimensat AT_SYMLINK_NOFOLLOW). The order
    // per file is owner, then mode, then xattrs: chown clears setuid/setgid
    // and security.capability, so chmod must follow it for a kept special bit
    // to survive, and xattrs must come last so nothing strips them again.
    for m in deferred.iter().rev() {
        let abs = staging.join(&m.rel);
        if opts.numeric_owner {
            let _ = lchown(&abs, m.uid, m.gid);
        }
        if !m.is_symlink {
            let _ = fs::set_permissions(&abs, fs::Permissions::from_mode(m.mode));
        }
        let _ = set_mtime_nofollow(&abs, m.mtime);
        xattrs_restored += apply_xattrs(&abs, &m.rel, &m.xattrs, caller)?;
    }

    let mut map = IndexMap::new();
    map.insert("files".to_string(), Value::Number(files as f64));
    map.insert("dirs".to_string(), Value::Number(dirs as f64));
    map.insert("symlinks".to_string(), Value::Number(symlinks as f64));
    map.insert("hardlinks".to_string(), Value::Number(hardlinks as f64));
    map.insert("bytes".to_string(), Value::Number(bytes as f64));
    map.insert(
        "xattrs_restored".to_string(),
        Value::Number(xattrs_restored as f64),
    );
    map.insert("entries".to_string(), Value::Number(entry_count as f64));
    map.insert(
        "trailing_padding".to_string(),
        Value::Number(trailing as f64),
    );
    map.insert(
        "codec".to_string(),
        Value::String(opts.codec.as_str().to_string()),
    );
    Ok(map)
}

/// Collect the entry's `SCHILY.xattr.*` PAX records before its data stream is
/// consumed. `security.*` is dropped here unless `keep_special_bits`: a
/// capability xattr is a privilege grant exactly like a suid bit, and the
/// tar crate's blanket `set_unpack_xattrs` would restore it regardless of the
/// caller's choice. The pairs are applied later by `apply_xattrs`, once the
/// owner and mode are settled.
fn collect_pax_xattrs<E: Read>(
    entry: &mut tar::Entry<E>,
    opts: &UnpackOpts,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let pax = match entry.pax_extensions() {
        Ok(Some(p)) => p,
        _ => return Vec::new(),
    };
    let mut pairs = Vec::new();
    for kv in pax.flatten() {
        let key = kv.key_bytes();
        if !key.starts_with(b"SCHILY.xattr.") {
            continue;
        }
        let xname = &key[b"SCHILY.xattr.".len()..];
        if !opts.keep_special_bits && xname.starts_with(b"security.") {
            continue;
        }
        pairs.push((xname.to_vec(), kv.value_bytes().to_vec()));
    }
    pairs
}

/// Apply collected xattr pairs to `abs` with `lsetxattr`. EPERM is tolerated
/// (non-root extraction is a documented capability limit); every other
/// failure raises.
fn apply_xattrs(
    abs: &Path,
    rel: &Path,
    pairs: &[(Vec<u8>, Vec<u8>)],
    caller: &str,
) -> MixResult<u64> {
    if pairs.is_empty() {
        return Ok(0);
    }
    let cpath = std::ffi::CString::new(abs.as_os_str().as_bytes())
        .map_err(|_| runtime(format!("{caller}: member path contains NUL")))?;
    let mut restored = 0u64;
    for (xname, value) in pairs {
        let Ok(cname) = std::ffi::CString::new(xname.as_slice()) else {
            continue; // xattr names cannot contain NUL by definition
        };
        let rc = unsafe {
            libc::lsetxattr(
                cpath.as_ptr(),
                cname.as_ptr(),
                value.as_ptr() as *const libc::c_void,
                value.len(),
                0,
            )
        };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EPERM) {
                continue; // non-root: documented limit, not an error
            }
            return Err(runtime(format!(
                "{caller}: xattr '{}' on '{}': {err}",
                String::from_utf8_lossy(xname),
                rel.display()
            )));
        }
        restored += 1;
    }
    Ok(restored)
}

fn lchown(path: &Path, uid: u32, gid: u32) -> bool {
    match std::ffi::CString::new(path.as_os_str().as_bytes()) {
        Ok(c) => unsafe { libc::lchown(c.as_ptr(), uid, gid) == 0 },
        Err(_) => false,
    }
}

fn set_mtime_nofollow(path: &Path, mtime: i64) -> bool {
    let ts = libc::timespec {
        tv_sec: mtime,
        tv_nsec: 0,
    };
    let times = [ts, ts];
    match std::ffi::CString::new(path.as_os_str().as_bytes()) {
        Ok(c) => unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                c.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            ) == 0
        },
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// tar_pack
// ---------------------------------------------------------------------------

struct PackOpts {
    codec: Codec,
    level: i32,
    keep_special_bits: bool,
}

pub fn builtin_tar_pack(args: Vec<Value>) -> MixResult<Option<Value>> {
    let caller = "tar_pack()";
    if args.len() < 2 || args.len() > 3 {
        return Err(runtime(format!(
            "{caller} expects 2 or 3 args (source, path[, opts]), got {}",
            args.len()
        )));
    }
    let source = args[0].to_mix_string();
    let path = args[1].to_mix_string();
    let mut opts = PackOpts {
        codec: Codec::Zstd,
        level: 7,
        keep_special_bits: false,
    };
    match args.get(2) {
        None | Some(Value::Nil) => {}
        Some(Value::Map(m)) => {
            if let Some(v) = m.get("codec") {
                opts.codec = parse_codec(caller, Some(v))?;
            }
            if let Some(v) = m.get("level") {
                match v {
                    Value::Number(n)
                        if *n >= ZSTD_MIN_LEVEL as f64
                            && *n <= ZSTD_MAX_LEVEL as f64
                            && n.fract() == 0.0 =>
                    {
                        opts.level = *n as i32;
                    }
                    _ => {
                        return Err(opt_invalid(
                            caller,
                            format!(
                                "level must be a whole number 1..=22 (C zstd numbering; \
                                 gzip clamps above 9), got {}",
                                v.type_name()
                            ),
                        ));
                    }
                }
            }
            if let Some(v) = bool_opt(caller, "keep_special_bits", m.get("keep_special_bits"))? {
                opts.keep_special_bits = v;
            }
            known_keys_check(caller, m, &["codec", "level", "keep_special_bits"])?;
        }
        Some(other) => {
            return Err(opt_invalid(
                caller,
                format!("options must be a map or nil, got {}", other.type_name()),
            ));
        }
    }
    // Remove trailing separators and '.' so O_NOFOLLOW still checks the
    // source directory itself, even when the caller writes "link/".
    let source_path: PathBuf = Path::new(&source).components().collect();
    let source_dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&source_path)
        .map_err(|e| runtime(format!("{caller}: open source directory '{source}': {e}")))?;
    let out_path = Path::new(&path);
    if let Some(parent) = out_path.parent()
        && !parent.is_dir()
    {
        return Err(runtime(format!(
            "{caller}: output parent '{}' does not exist",
            parent.display()
        )));
    }

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(out_path)
        .map_err(|e| runtime(format!("{caller}: create '{path}': {e}")))?;
    let buf = BufWriter::with_capacity(1 << 20, file);
    match pack_stream(source_dir, buf, &opts, caller) {
        Ok(receipt) => Ok(Some(Value::Map(std::rc::Rc::new(receipt)))),
        Err(e) => {
            let _ = fs::remove_file(out_path);
            Err(e)
        }
    }
}

enum PackSink {
    Zstd(Box<structured_zstd::encoding::StreamingEncoder<BufWriter<File>>>),
    Gzip(Box<flate2::write::GzEncoder<BufWriter<File>>>),
    None(BufWriter<File>),
}

impl Write for PackSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            PackSink::Zstd(w) => w.write(buf),
            PackSink::Gzip(w) => w.write(buf),
            PackSink::None(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            PackSink::Zstd(w) => w.flush(),
            PackSink::Gzip(w) => w.flush(),
            PackSink::None(w) => w.flush(),
        }
    }
}

impl PackSink {
    /// Finalise the CODEC before the buffered writer flushes: gzip without
    /// `finish` omits the CRC32/ISIZE trailer (our own unpack would reject
    /// the artifact), and ruzstd-lineage encoders finalise the frame on
    /// `finish`, not on flush.
    fn finish(self) -> std::io::Result<()> {
        match self {
            PackSink::Zstd(w) => w.finish().and_then(|mut b| b.flush()),
            PackSink::Gzip(w) => w.finish().and_then(|mut b| b.flush()),
            PackSink::None(mut w) => w.flush(),
        }
    }
}

fn pack_stream(
    source: File,
    buf: BufWriter<File>,
    opts: &PackOpts,
    caller: &str,
) -> MixResult<IndexMap<String, Value>> {
    let sink = match opts.codec {
        Codec::Zstd => {
            let level = structured_zstd::encoding::CompressionLevel::Level(opts.level);
            let enc = structured_zstd::encoding::StreamingEncoder::new(buf, level);
            PackSink::Zstd(Box::new(enc))
        }
        Codec::Gzip => PackSink::Gzip(Box::new(flate2::write::GzEncoder::new(
            buf,
            flate2::Compression::new(opts.level.min(9) as u32),
        ))),
        Codec::None => PackSink::None(buf),
    };
    walk_and_append(source, sink, opts, caller)
}

fn walk_and_append(
    source: File,
    sink: PackSink,
    opts: &PackOpts,
    caller: &str,
) -> MixResult<IndexMap<String, Value>> {
    let mut builder = Builder::new(sink);
    let mut files: u64 = 0;
    let mut dirs: u64 = 0;
    let mut symlinks: u64 = 0;
    let mut bytes: u64 = 0;
    let mut caps: u64 = 0;

    let mut queue = vec![(PathBuf::new(), source)];
    while let Some((dir_rel, dir)) = queue.pop() {
        let mut children = directory_names(&dir)
            .map_err(|e| runtime(format!("{caller}: read_dir '{}': {e}", dir_rel.display())))?;
        children.sort();
        for name in children {
            let rel = dir_rel.join(&name);
            // O_PATH pins even a symlink without following it. Classification,
            // symlink target and subsequent identity checks use this fd.
            let child = open_child(&dir, &name, libc::O_PATH)
                .map_err(|e| runtime(format!("{caller}: open '{}': {e}", rel.display())))?;
            let meta = child
                .metadata()
                .map_err(|e| runtime(format!("{caller}: fstat '{}': {e}", rel.display())))?;
            if meta.is_dir() {
                let opened =
                    open_child_checked(&dir, &name, &meta, libc::O_RDONLY | libc::O_DIRECTORY)
                        .map_err(|e| {
                            runtime(format!("{caller}: open directory '{}': {e}", rel.display()))
                        })?;
                let meta = opened
                    .metadata()
                    .map_err(|e| runtime(format!("{caller}: fstat '{}': {e}", rel.display())))?;
                let mut header = Header::new_gnu();
                header.set_size(0);
                set_header_meta(&mut header, &meta, opts);
                builder
                    .append_data(&mut header, &rel, std::io::empty())
                    .map_err(|e| runtime(format!("{caller}: append '{}': {e}", rel.display())))?;
                dirs += 1;
                queue.push((rel, opened));
            } else if meta.file_type().is_symlink() {
                let target = readlink_fd(&child)
                    .map_err(|e| runtime(format!("{caller}: readlink '{}': {e}", rel.display())))?;
                // Contained targets only, so packed archives always
                // round-trip through the safe unpacker.
                clean_link_target(caller, &rel.to_string_lossy(), target.as_os_str(), 4096)?;
                let mut header = Header::new_gnu();
                header.set_size(0);
                header.set_entry_type(EntryType::Symlink);
                set_header_meta(&mut header, &meta, opts);
                header
                    .set_link_name(&target)
                    .map_err(|e| runtime(format!("{caller}: link name: {e}")))?;
                builder
                    .append_data(&mut header, &rel, std::io::empty())
                    .map_err(|e| runtime(format!("{caller}: append '{}': {e}", rel.display())))?;
                symlinks += 1;
            } else if meta.is_file() {
                // O_NONBLOCK prevents a replacement fifo from blocking open;
                // the inode check then refuses every changed file type.
                let f = open_child_checked(&dir, &name, &meta, libc::O_RDONLY | libc::O_NONBLOCK)
                    .map_err(|e| {
                    runtime(format!("{caller}: open file '{}': {e}", rel.display()))
                })?;
                let meta = f
                    .metadata()
                    .map_err(|e| runtime(format!("{caller}: fstat '{}': {e}", rel.display())))?;
                if opts.keep_special_bits
                    && let Some(raw) = xattr_security_capability(&f)
                {
                    builder
                        .append_pax_extensions([(
                            "SCHILY.xattr.security.capability",
                            raw.as_slice(),
                        )])
                        .map_err(|e| {
                            runtime(format!("{caller}: pax xattr '{}': {e}", rel.display()))
                        })?;
                    caps += 1;
                }
                let mut header = Header::new_gnu();
                header.set_size(meta.len());
                set_header_meta(&mut header, &meta, opts);
                builder
                    .append_data(&mut header, &rel, f)
                    .map_err(|e| runtime(format!("{caller}: append '{}': {e}", rel.display())))?;
                files += 1;
                bytes += meta.len();
            } else {
                return Err(runtime(format!(
                    "{caller}: refusing to pack special file '{}' (device/fifo); \
                     source trees may contain only files, dirs and symlinks",
                    rel.display()
                )));
            }
        }
    }
    let sink = builder
        .into_inner()
        .map_err(|e| runtime(format!("{caller}: finish archive: {e}")))?;
    sink.finish()
        .map_err(|e| runtime(format!("{caller}: finalise codec: {e}")))?;

    let mut map = IndexMap::new();
    map.insert("files".to_string(), Value::Number(files as f64));
    map.insert("dirs".to_string(), Value::Number(dirs as f64));
    map.insert("symlinks".to_string(), Value::Number(symlinks as f64));
    map.insert("bytes".to_string(), Value::Number(bytes as f64));
    map.insert("capabilities".to_string(), Value::Number(caps as f64));
    map.insert(
        "codec".to_string(),
        Value::String(opts.codec.as_str().to_string()),
    );
    map.insert("level".to_string(), Value::Number(opts.level as f64));
    Ok(map)
}

fn set_header_meta(header: &mut Header, meta: &fs::Metadata, opts: &PackOpts) {
    let mode = meta.permissions().mode();
    let masked = if opts.keep_special_bits {
        mode
    } else {
        mode & !0o6000
    };
    header.set_mode(masked & 0o7777);
    header.set_uid(u64::from(meta.uid()));
    header.set_gid(u64::from(meta.gid()));
    header.set_mtime(meta.mtime().max(0) as u64);
    if meta.is_dir() {
        header.set_entry_type(EntryType::Directory);
    }
}

fn open_child(dir: &File, name: &std::ffi::OsStr, flags: i32) -> std::io::Result<File> {
    let name = std::ffi::CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        // Ownership of the newly opened fd transfers to File.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn open_child_checked(
    dir: &File,
    name: &std::ffi::OsStr,
    expected: &fs::Metadata,
    flags: i32,
) -> std::io::Result<File> {
    let opened = open_child(dir, name, flags)?;
    let actual = opened.metadata()?;
    if actual.dev() != expected.dev()
        || actual.ino() != expected.ino()
        || actual.mode() & libc::S_IFMT != expected.mode() & libc::S_IFMT
    {
        return Err(std::io::Error::other("source entry changed while packing"));
    }
    Ok(opened)
}

/// Enumerate the directory already opened and queued, never its pathname.
fn directory_names(dir: &File) -> std::io::Result<Vec<std::ffi::OsString>> {
    struct DirStream(*mut libc::DIR);
    impl Drop for DirStream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let fd = unsafe { libc::fcntl(dir.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        let err = std::io::Error::last_os_error();
        unsafe {
            libc::close(fd);
        }
        return Err(err);
    }
    let stream = DirStream(stream);
    let mut names = Vec::new();
    loop {
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(0) {
                return Err(err);
            }
            return Ok(names);
        }
        let bytes = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(std::ffi::OsString::from_vec(bytes.to_vec()));
        }
    }
}

fn readlink_fd(file: &File) -> std::io::Result<PathBuf> {
    let mut target = vec![0u8; 4097];
    // Linux readlinkat with an empty name operates on the pinned O_PATH fd.
    let n = unsafe {
        libc::readlinkat(
            file.as_raw_fd(),
            c"".as_ptr(),
            target.as_mut_ptr().cast(),
            target.len(),
        )
    };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if n as usize == target.len() {
        return Err(std::io::Error::other("symlink target exceeds max_name"));
    }
    target.truncate(n as usize);
    Ok(PathBuf::from(std::ffi::OsString::from_vec(target)))
}

/// Size-query first, then read: a fixed buffer would silently truncate
/// VFS_CAP_REVISION_3 values (~1 KB) into corrupt capabilities.
fn xattr_security_capability(file: &File) -> Option<Vec<u8>> {
    let name = c"security.capability".as_ptr();
    let size = unsafe { libc::fgetxattr(file.as_raw_fd(), name, std::ptr::null_mut(), 0) };
    if size <= 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let n = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            name,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
        )
    };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(buf)
}
