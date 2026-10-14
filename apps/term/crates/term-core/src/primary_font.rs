// SPDX-License-Identifier: MIT OR Apache-2.0
//! Validated terminal face discovery. Fallback routing stays in unicode_raster.
#[cfg(test)]
use design::TypographyGeneric;
use design::{TypographyRole, default_typography};
use fontdb::{Database, Family, Query, Source, Style, Weight};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use swash::{
    FontRef,
    scale::{Render, ScaleContext, Source as GlyphSource},
    tag_from_bytes,
    zeno::Format,
};

#[cfg(any(test, feature = "test-support"))]
const LEGACY_PATHS: &[&str] = &[
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/liberation2/LiberationMono-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf",
];

#[derive(Clone)]
pub(super) struct Primary {
    pub path: PathBuf,
    pub index: u32,
    pub data: Arc<[u8]>,
}

// Primary and lazy Unicode coverage share one pinned selection. Installing a
// newer set during this process cannot combine fonts from two asset releases.
fn installed_assets() -> Result<Option<&'static assets::AssetSet>, &'static str> {
    static INSTALLED: OnceLock<Result<Option<assets::AssetSet>, String>> = OnceLock::new();
    match INSTALLED.get_or_init(|| assets::mixos::discover().map_err(|error| error.to_string())) {
        Ok(set) => Ok(set.as_ref()),
        Err(error) => {
            static REPORTED: OnceLock<()> = OnceLock::new();
            REPORTED.get_or_init(|| eprintln!("terminal static assets: {error}"));
            Err(error)
        }
    }
}

#[cfg(test)]
pub(super) fn discover(override_path: Option<&Path>) -> Result<Primary, String> {
    // An explicit path is authoritative, including an error for a bad file.
    if let Some(path) = override_path {
        return from_path(path);
    }
    static SHARED: OnceLock<Result<Primary, String>> = OnceLock::new();
    SHARED
        .get_or_init(|| discover_face(None, None, DEFAULT_WEIGHT))
        .clone()
}

/// The CSS weight the terminal's settings call regular.
pub const DEFAULT_WEIGHT: u16 = 400;

/// A chosen face: `family` nearest `weight` when it is installed and
/// monospace, else the bundled monospace font nearest `weight`. Each
/// fallback is logged; it runs once per change of family or weight. An
/// explicit override path stays authoritative.
pub(super) fn discover_face(
    override_path: Option<&Path>,
    family: Option<&str>,
    weight: u16,
) -> Result<Primary, String> {
    if let Some(path) = override_path {
        return from_path(path);
    }
    let weight = Weight(weight);
    if let Some(name) = family {
        let mut db = Database::new();
        db.load_system_fonts();
        match named(&db, name, weight) {
            Ok(primary) => return Ok(primary),
            Err(reason) => {
                eprintln!("term: font family {name:?} {reason}; using the bundled monospace font")
            }
        }
    }
    // All default weights use the same bundled family. Installed mono roles
    // and system families can be single static faces, so cannot implement
    // a weight choice reliably. Named installed families above stay explicit.
    bundled(weight.0)
}

fn bundled(weight: u16) -> Result<Primary, String> {
    let (name, bytes) = assets::mixos::mono_face(weight);
    let font = FontRef::from_index(bytes, 0)
        .filter(|font| usable(*font))
        .ok_or_else(|| format!("bundled font {name} cannot be drawn"))?;
    debug_assert_eq!(font.attributes().style(), swash::Style::Normal);
    Ok(Primary {
        path: PathBuf::from(format!("bundled/{name}.ttf")),
        index: 0,
        data: Arc::from(bytes),
    })
}

/// Installed family `name`, the face nearest `weight`, when it is monospace
/// and drawable.
fn named(db: &Database, name: &str, weight: Weight) -> Result<Primary, &'static str> {
    let family = [Family::Name(name)];
    let query = Query {
        families: &family,
        weight,
        style: Style::Normal,
        ..Query::default()
    };
    let id = db.query(&query).ok_or("is not installed")?;
    if !db.face(id).is_some_and(|face| face.monospaced) {
        return Err("is not monospace");
    }
    load_face(db, id).ok_or("cannot be drawn")
}

/// Called only when the shared Unicode fallback set is first needed.
pub(super) fn installed_role(role: &str) -> Result<Option<Primary>, String> {
    let Some(set) = installed_assets().map_err(str::to_owned)? else {
        return Ok(None);
    };
    let Some(path) = set.font_path(role) else {
        return Ok(None);
    };
    let mut db = Database::new();
    db.load_font_file(&path)
        .map_err(|error| format!("installed {role} font {}: {error}", path.display()))?;
    let index = db
        .faces()
        .next()
        .ok_or_else(|| format!("installed {role} font has no face"))?
        .index;
    let data: Arc<[u8]> = std::fs::read(&path)
        .map_err(|error| format!("installed {role} font {}: {error}", path.display()))?
        .into();
    let font = FontRef::from_index(&data, index as usize)
        .ok_or_else(|| format!("installed {role} font cannot be read by Swash"))?;
    if !metrics_readable(font) {
        return Err(format!("installed {role} font has unreadable metrics"));
    }
    Ok(Some(Primary { path, index, data }))
}

/// Called only when the shared Unicode fallback set is first needed.
pub(super) fn coverage(weight: u16) -> Vec<Primary> {
    let mut db = Database::new();
    db.load_system_fonts();
    // Emoji has a dedicated lane in UnicodeRaster. Every general coverage
    // face uses the same validated loader, including the Swash metrics guard.
    let installed = ["sans", "serif"]
        .into_iter()
        .filter_map(|role| match installed_role(role) {
            Ok(face) => face.map(|face| fallback_weight(&mut db, face, weight)),
            Err(error) => {
                eprintln!("terminal static coverage: {error}");
                None
            }
        });
    let mut faces: Vec<_> = installed.collect();
    faces.extend(default_typography(TypographyRole::Terminal).fallbacks.iter()
        .filter_map(|name| fallback_family(&db, name, weight)));
    faces
}

/// Symbol/emoji faces may lack ASCII. Validate their metrics, without the
/// primary font's monospace/ASCII requirements, and query their own family.
pub(super) fn fallback_paths(paths: &[&str], weight: u16) -> Vec<Primary> {
    let mut db = Database::new();
    db.load_system_fonts();
    paths.iter().filter_map(|path| {
        db.load_font_file(path).ok()?;
        let face = db.faces().find(|face| matches!(&face.source,
            Source::File(file) | Source::SharedFile(file, _) if file == Path::new(path)))?;
        let name = face.families.first()?.0.clone();
        fallback_family(&db, &name, weight)
    }).collect()
}

pub(super) fn fallback_weight(db: &mut Database, face: Primary, weight: u16) -> Primary {
    if db.load_font_file(&face.path).is_ok() {
        let name = db.faces().find(|f| f.index == face.index && matches!(&f.source,
            Source::File(path) | Source::SharedFile(path, _) if *path == face.path))
            .and_then(|f| f.families.first()).map(|(name, _)| name.clone());
        if let Some(selected) = name.and_then(|name| fallback_family(db, &name, weight)) {
            return selected;
        }
    }
    face
}

fn fallback_family(db: &Database, name: &str, weight: u16) -> Option<Primary> {
    let mut faces: Vec<_> = db.faces().filter(|face| face.style == Style::Normal
        && face.families.iter().any(|(family, _)| family == name)).collect();
    faces.sort_by_key(|face| (face.weight.0.abs_diff(weight), face.weight.0));
    for face in faces {
        let (source, index) = db.face_source(face.id)?;
        let path = match source {
            Source::File(path) | Source::SharedFile(path, _) => path,
            Source::Binary(_) => continue,
        };
        let Ok(data) = std::fs::read(&path) else { continue };
        let data: Arc<[u8]> = data.into();
        if FontRef::from_index(&data, index as usize).is_some_and(metrics_readable) {
            if face.weight.0 != weight {
                eprintln!("term: fallback {name:?}: requested weight {weight}, nearest available {}", face.weight.0);
            }
            return Some(Primary { path, index, data });
        }
    }
    None
}

#[cfg(test)]
pub(super) fn from_database(db: &mut Database) -> Option<Primary> {
    select_weight(db, &role_families(), Weight(DEFAULT_WEIGHT))
}

/// The terminal role's family, its fallbacks, then its generic family: the
/// order every primary face is looked up in.
#[cfg(test)]
fn role_families() -> Vec<Family<'static>> {
    let role = default_typography(TypographyRole::Terminal);
    std::iter::once(&role.family)
        .chain(&role.fallbacks)
        .map(|name| Family::Name(name.as_str()))
        .chain([match role.generic {
            TypographyGeneric::Monospace => Family::Monospace,
            TypographyGeneric::SansSerif => Family::SansSerif,
        }])
        .collect()
}

pub(super) fn from_path(path: &Path) -> Result<Primary, String> {
    let mut db = Database::new();
    db.load_font_file(path)
        .map_err(|e| format!("font {}: {e}; set TERM_SPIKE_FONT", path.display()))?;
    // Even overrides can be collections. Match Light/normal within this file,
    // rather than losing its chosen index when constructing either renderer.
    let names: Vec<_> = db
        .faces()
        .flat_map(|face| face.families.iter())
        .map(|(name, _)| name.clone())
        .collect();
    let families: Vec<_> = names.iter().map(|name| Family::Name(name)).collect();
    select(&mut db, &families).ok_or_else(|| {
        format!(
            "Invalid font {}; set TERM_SPIKE_FONT to a TTF/OTF font",
            path.display()
        )
    })
}

fn select(db: &mut Database, families: &[Family<'_>]) -> Option<Primary> {
    select_weight(
        db,
        families,
        Weight(default_typography(TypographyRole::Terminal).weight),
    )
}

fn select_weight(db: &mut Database, families: &[Family<'_>], weight: Weight) -> Option<Primary> {
    for family in families {
        while let Some(id) = query_family(db, family, weight) {
            let loaded = load_face(db, id);
            if loaded.is_some() {
                return loaded;
            }
            // A stale path or a face Swash cannot load must not hide the next
            // usable face/family in the ordered query.
            db.remove_face(id);
        }
    }
    None
}

/// Face `id`'s file, when Swash can draw it.
fn load_face(db: &Database, id: fontdb::ID) -> Option<Primary> {
    db.face_source(id).and_then(|(source, index)| {
        let path = match source {
            Source::File(path) | Source::SharedFile(path, _) => path,
            Source::Binary(_) => return None,
        };
        let data: Arc<[u8]> = std::fs::read(&path).ok()?.into();
        let font = FontRef::from_index(&data, index as usize)?;
        if !usable(font) {
            return None;
        }
        Some(Primary { path, index, data })
    })
}

fn query_family(db: &mut Database, family: &Family<'_>, weight: Weight) -> Option<fontdb::ID> {
    let mut query = Query {
        families: std::slice::from_ref(family),
        weight,
        style: Style::Normal,
        ..Query::default()
    };
    loop {
        let id = db.query(&query)?;
        if db.face(id)?.weight >= weight {
            return Some(id);
        }
        // CSS searches below 300 before considering 400. Retry this family
        // (including a fontconfig generic alias) at Regular, never Thin.
        if query.weight < Weight::NORMAL {
            query.weight = Weight::NORMAL;
        } else {
            db.remove_face(id);
        }
    }
}

fn usable(font: FontRef<'_>) -> bool {
    if !metrics_readable(font) {
        return false;
    }
    let metrics = font.glyph_metrics(&[]);
    let mut context = ScaleContext::new();
    let mut scaler = context.builder(font).size(24.0).hint(true).build();
    ['M', '0'].into_iter().all(|ch| {
        let glyph = font.charmap().map(ch);
        let advance = metrics.advance_width(glyph);
        glyph != 0
            && advance.is_finite()
            && advance > 0.0
            && Render::new(&[GlyphSource::Outline])
                .format(Format::Alpha)
                .render(&mut scaler, glyph)
                .is_some_and(|image| {
                    image.placement.width > 0
                        && image.placement.height > 0
                        && image.data.iter().any(|sample| *sample != 0)
                })
    })
}

/// Whether swash can answer horizontal advances for `font` without panicking.
///
/// Derived from swash 0.2.10 internals, but decided only from public values:
/// `MetricsProxy::fill` returns early when head or maxp is unreadable, leaving
/// the long-metric count at its Default 0, and a readable hhea shorter than 36
/// bytes (or a literal 0) also yields 0; `xmtx::advance` then computes
/// `count - 1` unchecked (a panic with overflow checks, a wrong-but-safe 0
/// without). `glyph_count` is set only after head and maxp both parsed, and
/// `table` is None for a table missing or lying past EOF. The hmtx length check
/// guards no panic (every hmtx read is bounds-checked); it is belt and braces.
/// Vertical metrics share the flaw via vhea, but term never asks for them.
pub(super) fn metrics_readable(font: FontRef<'_>) -> bool {
    let metrics = font.metrics(&[]);
    let count = font
        .table(tag_from_bytes(b"hhea"))
        .and_then(|hhea| hhea.get(34..36))
        .map_or(0, |b| u16::from_be_bytes([b[0], b[1]]));
    metrics.units_per_em > 0
        && metrics.glyph_count > 0
        && count > 0
        && font
            .table(tag_from_bytes(b"hmtx"))
            .is_some_and(|hmtx| hmtx.len() >= count as usize * 4)
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn fixture() -> Result<Primary, String> {
    static SHARED: OnceLock<Result<Primary, String>> = OnceLock::new();
    SHARED
        .get_or_init(|| fixture_weight(Weight::NORMAL))
        .clone()
}

#[cfg(any(test, feature = "test-support"))]
pub(super) fn fixture_weight(weight: Weight) -> Result<Primary, String> {
    let mut db = Database::new();
    db.load_system_fonts();
    select_weight(&mut db, &[Family::Name("DejaVu Sans Mono")], weight)
        .or_else(|| {
            LEGACY_PATHS[..2].iter().find_map(|path| {
                let path = if weight == Weight::BOLD {
                    Path::new(path).with_file_name("DejaVuSansMono-Bold.ttf")
                } else {
                    PathBuf::from(path)
                };
                from_path(&path).ok()
            })
        })
        .ok_or_else(|| "raster fixtures require DejaVu Sans Mono".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a bootstrapped static asset set"]
    fn bundled_regular_is_primary_and_installed_emoji_is_coverage() {
        let set = assets::mixos::discover().unwrap().expect("installed set");
        assert_eq!(
            discover(None).unwrap().path,
            Path::new("bundled/JetBrainsMono-Regular.ttf")
        );
        assert_eq!(
            installed_role("emoji").unwrap().unwrap().path,
            set.font_path("emoji").unwrap()
        );
        assert!(
            coverage(DEFAULT_WEIGHT)
                .iter()
                .any(|face| face.path == set.font_path("sans").unwrap())
        );
        let explicit = fixture().unwrap();
        assert_eq!(discover(Some(&explicit.path)).unwrap().path, explicit.path);
        assert!(discover(Some(Path::new("/no/such/explicit-font.ttf"))).is_err());
    }

    #[test]
    fn regular_queries_do_not_inherit_the_terminal_roles_light_weight() {
        let fixture = fixture().unwrap();
        let mut source = Database::new();
        source.load_font_file(&fixture.path).unwrap();
        let template = source.faces().next().unwrap().clone();
        for weights in [
            &[100, 400][..],
            &[200, 400],
            &[100, 200, 400],
            &[100, 200, 300, 400],
        ] {
            for family in [Family::Name("Test Mono"), Family::Monospace] {
                let mut db = Database::new();
                db.set_monospace_family("Test Mono");
                for weight in weights {
                    let mut face = template.clone();
                    face.families =
                        vec![("Test Mono".into(), fontdb::Language::English_UnitedStates)];
                    face.style = Style::Normal;
                    face.weight = Weight(*weight);
                    db.push_face_info(face);
                }
                let id = query_family(&mut db, &family, Weight(DEFAULT_WEIGHT)).unwrap();
                assert_eq!(db.face(id).unwrap().weight, Weight::NORMAL);
            }
        }
    }
}
