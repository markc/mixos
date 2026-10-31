// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The mirror's sealed cache (F1, design §5.4): what a desk's settingsd
//! serves when it mirrors the machine's authority. It is not a profile: no
//! writer lock, no receipts, never migrated, and never a source of writes.
//! It carries the same seal and is checked the same way as the authority's
//! record, including the re-derived interpretation. A cache that fails its
//! check is set aside and rebuilt from the authority, never served.
//!
//! S6 is the storage only. The wire that fills it (`settings.get` from
//! `settingsd.<node>.bus`, then `settings.mirror.attach` and the pushed
//! `settings.mirror.revision`) is S7, which calls [`open`] with its fetch
//! and [`write`] on each push.
use crate::store::{self, Accepted, FORMAT, PRIMARY, canonical, encode};
use serde::{Deserialize, Serialize};
use settings::registry::Values;
use settings::*;
use std::path::Path;

/// The cache, in the mirror's root.
pub const CACHE: &str = "mirror.conf.mix";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cache {
    pub schema: u32,
    pub format: u32,
    /// The binding the mirror answers for: the authority's.
    pub binding: Binding,
    /// The node whose settingsd is the authority.
    pub authority: String,
    pub incarnation: String,
    pub revision: Revision,
    pub design_revision: Revision,
    pub values: Values,
    pub embedded_source: String,
    pub effective_digest: String,
    pub content_digest: String,
}

/// How [`open`] came by the cache it returns.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// The stored cache verified.
    Cached,
    /// The authority's, because there was none, or the stored one was
    /// refused and set aside under `refused`.
    Rebuilt { refused: Option<String> },
}

impl Cache {
    /// The cache of the authority's record `accepted`, sealed. The receipts
    /// stay with the authority.
    pub fn of(accepted: &Accepted, authority: &str) -> anyhow::Result<Self> {
        let mut cache = Self {
            schema: SCHEMA,
            format: FORMAT,
            binding: accepted.binding.clone(),
            authority: authority.into(),
            incarnation: accepted.incarnation.clone(),
            revision: accepted.revision,
            design_revision: accepted.design_revision,
            values: canonical(accepted.values.clone()),
            embedded_source: accepted.embedded_source.clone(),
            effective_digest: accepted.effective_digest.clone(),
            content_digest: String::new(),
        };
        cache.seal()?;
        Ok(cache)
    }
    fn digest(&self) -> anyhow::Result<String> {
        Ok(settings::digest(&(
            (
                &self.schema,
                &self.binding,
                &self.authority,
                &self.incarnation,
                &self.revision,
                &self.design_revision,
                &self.values,
                &self.embedded_source,
                &self.effective_digest,
            ),
            self.format,
        ))?)
    }
    pub fn seal(&mut self) -> anyhow::Result<()> {
        self.content_digest = self.digest()?;
        Ok(())
    }
    /// The record the mirror serves reads from: the authority's identity
    /// and values, with no receipts.
    pub fn accepted(&self) -> Accepted {
        let mut accepted = Accepted {
            schema: self.schema,
            format: self.format,
            binding: self.binding.clone(),
            incarnation: self.incarnation.clone(),
            revision: self.revision,
            design_revision: self.design_revision,
            values: self.values.clone(),
            embedded_source: self.embedded_source.clone(),
            effective_digest: self.effective_digest.clone(),
            receipts: Vec::new(),
            content_digest: String::new(),
        };
        // Sealed only so the shared checks hold; a mirror never stores it.
        let _ = accepted.seal();
        accepted
    }
    /// The seal check the authority's record has, for this binding and
    /// authority, including the interpretation.
    pub fn check(&self, binding: &Binding, authority: &str) -> anyhow::Result<()> {
        anyhow::ensure!(self.schema == SCHEMA, "unsupported cache schema");
        anyhow::ensure!(
            self.format == FORMAT,
            "cache format {} is not {FORMAT}",
            self.format
        );
        anyhow::ensure!(&self.binding == binding, "cache holds another binding");
        anyhow::ensure!(
            self.authority == authority,
            "cache mirrors another authority"
        );
        anyhow::ensure!(
            self.embedded_source.len() <= MAX_SOURCE_BYTES,
            "pinned source limit exceeded"
        );
        anyhow::ensure!(
            self.content_digest == self.digest()?,
            "cache content digest mismatch"
        );
        store::check_lineage(
            binding,
            &self.incarnation,
            self.revision,
            self.design_revision,
            &[],
        )?;
        self.accepted().effective()?;
        Ok(())
    }
}

/// The mirror's root: created owner-only when missing, and never a
/// profile's (a profile left there is retired first, `settingsd retire`).
fn directory(root: &Path) -> anyhow::Result<std::fs::File> {
    if !root.try_exists()? {
        store::create_directory(root)?;
    }
    let directory = store::open_directory(root)?;
    match store::open_in(&directory, PRIMARY, libc::O_RDONLY) {
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(directory)
        }
        Err(e) => Err(e),
        Ok(_) => anyhow::bail!(
            "{} holds a profile ({PRIMARY}); retire it before mirroring",
            root.display()
        ),
    }
}

/// The cache to serve for `binding` from `authority`. A stored cache that
/// verifies is served. One that does not (unreadable, tampered, another
/// binding or authority, or an interpretation this build does not derive)
/// is renamed aside and replaced by `fetch`'s, which must pass the same
/// check before it is written or served. With no cache, `fetch` supplies
/// the first.
pub fn open(
    root: &Path,
    binding: &Binding,
    authority: &str,
    fetch: impl FnOnce() -> anyhow::Result<Cache>,
) -> anyhow::Result<(Cache, Origin)> {
    let dir = directory(root)?;
    let refused = match store::open_in(&dir, CACHE, libc::O_RDONLY) {
        Ok(file) => {
            let stored = store::read_bytes(file).and_then(|bytes| {
                let mut cache: Cache = strict::from_str(std::str::from_utf8(&bytes)?)?;
                cache.values = canonical(std::mem::take(&mut cache.values));
                cache.check(binding, authority)?;
                Ok(cache)
            });
            match stored {
                Ok(cache) => return Ok((cache, Origin::Cached)),
                Err(error) => {
                    let aside =
                        format!("mirror.refused-{}.conf.mix", uuid::Uuid::now_v7().simple());
                    eprintln!(
                        "settingsd: mirror cache refused ({error}); kept as {aside}, rebuilding from the authority"
                    );
                    std::fs::rename(root.join(CACHE), root.join(&aside))?;
                    dir.sync_all()?;
                    Some(aside)
                }
            }
        }
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(e) => return Err(e),
    };
    let cache = fetch().map_err(|e| anyhow::anyhow!("mirror cache unavailable: {e}"))?;
    cache.check(binding, authority)?;
    write_in(&dir, &cache)?;
    Ok((cache, Origin::Rebuilt { refused }))
}

/// Replace the cache with `cache`, which must verify first for the
/// mirror's own `binding` and `authority`, as [`open`] checks it.
pub fn write(root: &Path, binding: &Binding, authority: &str, cache: &Cache) -> anyhow::Result<()> {
    cache.check(binding, authority)?;
    write_in(&directory(root)?, cache)
}

fn write_in(dir: &std::fs::File, cache: &Cache) -> anyhow::Result<()> {
    config::atomic::replace_in(dir, CACHE.as_ref(), &encode(cache)?)?;
    Ok(())
}
