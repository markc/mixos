// SPDX-License-Identifier: MIT OR Apache-2.0
//! The session-local appearance cache. It is a convenience for cold start
//! only: it is read once, validated, and never written back to settingsd.
use settings::Binding;
use settings::appearance::AppearanceProjection;
use std::io::Read;
use std::path::{Path, PathBuf};

const FILE: &str = "appearance.json";
const MAX_BYTES: u64 = 64 * 1024;

/// Binding fields are validated path components, with no separators or dots.
pub fn path(dir: &Path, binding: &Binding) -> anyhow::Result<PathBuf> {
    binding
        .validate()
        .map_err(|fault| anyhow::anyhow!(fault.message))?;
    Ok(dir
        .join(&binding.instance)
        .join(&binding.profile)
        .join(FILE))
}

/// A validated cached projection, or `None` when absent or unusable. An
/// unusable file is logged and ignored, never fatal.
pub fn load(dir: &Path, binding: &Binding) -> Option<AppearanceProjection> {
    let path = path(dir, binding).ok()?;
    let file = std::fs::File::open(&path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        tracing::warn!(path = %path.display(), "appearance cache too large or not regular; ignored");
        return None;
    }
    let mut text = String::new();
    // Bound the read too: a file can grow after metadata was sampled.
    match file.take(MAX_BYTES + 1).read_to_string(&mut text) {
        Ok(_) if text.len() as u64 <= MAX_BYTES => {}
        result => {
            tracing::warn!(path = %path.display(), ?result, "appearance cache unreadable or too large; ignored");
            return None;
        }
    }
    let parsed: Result<AppearanceProjection, String> = serde_json::from_str(&text)
        .map_err(|error| error.to_string())
        .and_then(|projection: AppearanceProjection| {
            projection
                .validate()
                .map(|()| projection)
                .map_err(|diagnostic| diagnostic.message)
        });
    match parsed {
        Ok(projection) if &projection.binding == binding => Some(projection),
        Ok(_) => {
            tracing::warn!(path = %path.display(), "appearance cache binding differs; ignored");
            None
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "appearance cache invalid; ignored");
            None
        }
    }
}

/// Replaces the cache atomically: write a sibling temporary file, then rename.
pub fn save(dir: &Path, projection: &AppearanceProjection) -> anyhow::Result<()> {
    projection
        .validate()
        .map_err(|fault| anyhow::anyhow!(fault.message))?;
    let path = path(dir, &projection.binding)?;
    std::fs::create_dir_all(path.parent().expect("cache has a parent"))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(projection)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}
