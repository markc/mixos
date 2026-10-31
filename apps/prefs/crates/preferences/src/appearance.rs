// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The Appearance panel over settingsd (docs/docs/spec/settings/): the session's
//! colour scheme, style, mode, contrast, fonts, scale, density and motion.
//!
//! The panel reads the desktop profile (`settings.get`) and applies each
//! edit at once in a fenced `settings.apply`, as GNOME's settings do. An
//! edit made while an apply is in flight or settling waits as a draft,
//! measured against the expected look (settingsd's with the apply laid
//! over it), and goes out once it settles. A refused apply returns its
//! edits to the draft and holds them for the person (Apply tries again,
//! Revert drops them) instead of retrying in a loop. Another writer's change arrives as a hint on the
//! profile's topic and is re-read over the authenticated call; a draft
//! survives it: the draft is the axes the person edited, and it follows
//! settingsd on every other axis. An edited axis another writer changed to
//! something else is a conflict, applied over only once the person keeps
//! their edits. Every apply is fenced on the revision shown and sends only
//! the edited axes, so it never lands on an unseen change, nor twice. An
//! apply whose reply is lost is settled by its receipt (`settings.status`
//! with the operation id) before anything else is applied; when settingsd
//! holds no receipt (evicted, or never accepted) the next read decides what
//! is reported, never a guess. Quitting waits for a bounded number of
//! receipt lookups; nothing needs to survive a restart, since the fence and
//! a fresh read already make any later apply safe.
use crate::email::Edit as EmailEdit;
use crate::engine::{Effect, Engine};
use crate::terminal::Edit;
use citizen::{CallError, Reply};
use design::{
    CaptionSide, Contrast, DesignContext, Mode, RequestedDecorations as Decorations, Scheme, Style,
};
use serde::Serialize;
use serde_json::{Value, json};
use settings::model::{Appearance as Stored, ApplyRequest, Binding, Revision};
use settings::terminal::Terminal as TerminalPrefs;
use std::collections::BTreeMap;

/// The Bus service behind the Appearance panel.
pub const SETTINGS: &str = "settingsd";
/// The desktop profile the panel edits.
pub const PROFILE: &str = "default";

/// The topics settingsd announces the profile's changes on (contract
/// 1.0.0): the look and the `keys` topics of the app domains the panels
/// show. Each is a hint; the panels read the `values` view.
pub fn topics() -> Vec<String> {
    settings::follow::hint_topics(PROFILE)
}

/// Whether a delivery on `topic` is one of [`topics`].
pub fn is_topic(topic: &str) -> bool {
    settings::follow::is_hint(PROFILE, topic)
}

/// The session's interface font weight and size, as settingsd stores them.
#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceFont {
    /// `None` is the scheme's own weight.
    pub weight: Option<u16>,
    /// `None` is the design's own UI size.
    pub size: Option<f64>,
}

/// One control's change to the session appearance.
#[derive(Clone, Debug, PartialEq)]
pub enum AppearanceEdit {
    FontFamily(Option<String>),
    MonoFamily(Option<String>),
    TextScale(f64),
    Density(f64),
    ReducedMotion(bool),
}

impl AppearanceEdit {
    fn args(&self) -> Value {
        match self {
            Self::FontFamily(family) => json!({"font_family":family}),
            Self::MonoFamily(family) => json!({"mono_family":family}),
            Self::TextScale(scale) => json!({"text_scale":scale}),
            Self::Density(density) => json!({"density":density}),
            Self::ReducedMotion(reduced) => json!({"reduced_motion":reduced}),
        }
    }
}

/// One appearance, as the panel shows and edits it.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub scheme: Scheme,
    /// `None` is the scheme's own style.
    pub style: Option<Style>,
    pub mode: Mode,
    pub contrast: Contrast,
    pub decorations: Decorations,
    pub captions: String,
    pub window_style: String,
    pub menu_style: String,
    pub menu_side: String,
    pub actions_place: String,
    pub actions_align: String,
    pub title_align: String,
    pub font_family: Option<String>,
    pub mono_family: Option<String>,
    pub text_scale: f64,
    pub density: f64,
    pub reduced_motion: bool,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            scheme: Scheme::default(),
            style: None,
            mode: Mode::default(),
            contrast: Contrast::default(),
            decorations: Decorations::default(),
            captions: CaptionSide::default().name().into(),
            window_style: design::WindowStyle::default().name().into(),
            menu_style: design::MenuStyle::default().name().into(),
            menu_side: design::MenuSide::default().name().into(),
            actions_place: design::ActionsPlace::default().name().into(),
            actions_align: design::ActionsAlign::default().name().into(),
            title_align: design::TitleAlign::default().name().into(),
            font_family: None,
            mono_family: None,
            text_scale: 1.0,
            density: 1.0,
            reduced_motion: false,
        }
    }
}

fn named<T>(what: &str, name: &str, from_name: fn(&str) -> Option<T>) -> Result<T, String> {
    from_name(name).ok_or_else(|| format!("unknown {what} {name:?}"))
}

impl Look {
    pub fn decoration_resolution(&self) -> design::decorations::Resolution {
        design::decorations::resolve_with_actions(
            None,
            self.decorations,
            design::ActionsPlace::resolve(&self.actions_place),
        )
    }

    /// The appearance summary distinguishes the stored request from what
    /// this session's window backend can actually draw.
    pub fn effective(&self) -> Value {
        let resolved = self.decoration_resolution();
        json!({"scheme":self.scheme.name(),"style":self.style.map_or("own", Style::name),
            "mode":self.mode.name(),"contrast":self.contrast.name(),
            "decorations":resolved.mode.name(),"decorations_reason":resolved.reason,
            "caption_side":CaptionSide::resolve(&self.captions).name(),
            "window_style":design::WindowStyle::resolve(&self.window_style).name(),
            "title_align":design::TitleAlign::resolve(&self.title_align).name(),
            "actions_align":design::ActionsAlign::resolve(&self.actions_align).name(),
            "actions_place":design::ActionsPlace::resolve(&self.actions_place).name(),
            "menu_style":design::MenuStyle::resolve(&self.menu_style).name(),
            "menu_side":design::MenuSide::resolve(&self.menu_side).name()})
    }

    /// The profile's stored appearance.
    pub fn from_stored(stored: &Stored) -> Result<Self, String> {
        Ok(Self {
            scheme: named("scheme", &stored.scheme, Scheme::from_name)?,
            style: match &stored.style {
                None => None,
                Some(name) => Some(named("style", name, Style::from_name)?),
            },
            mode: named("mode", &stored.mode, Mode::from_name)?,
            contrast: named("contrast", &stored.contrast, Contrast::from_name)?,
            decorations: named("decorations", &stored.decorations, Decorations::from_name)?,
            captions: stored.caption_side.clone(),
            window_style: stored.window_style.clone(),
            menu_style: stored.menu_style.clone(),
            menu_side: stored.menu_side.clone(),
            actions_place: stored.actions_place.clone(),
            actions_align: stored.actions_align.clone(),
            title_align: stored.title_align.clone(),
            font_family: stored.font_family.clone(),
            mono_family: stored.mono_family.clone(),
            ..Self::default()
        })
    }

    /// The profile's look as the 1.0.0 values give it (key space v2 §4.1):
    /// `appearance.*`, the `window.*` framing and the accessibility keys.
    /// The mode is the resolved one (an authored `auto` shows what it
    /// resolves to), the density its step's multiplier.
    pub fn from_view(view: &settings::view::Look) -> Result<Self, String> {
        let a = &view.appearance;
        let f = &view.framing;
        Ok(Self {
            scheme: named("scheme", &a.scheme, Scheme::from_name)?,
            style: match &a.style {
                None => None,
                Some(name) => Some(named("style", name, Style::from_name)?),
            },
            mode: named("mode", view.mode().0.name(), Mode::from_name)?,
            contrast: named("contrast", &a.contrast, Contrast::from_name)?,
            decorations: named("decorations", &f.decorations, Decorations::from_name)?,
            captions: f.caption_side.clone(),
            window_style: f.style.clone(),
            menu_style: f.menu_style.clone(),
            menu_side: f.menu_side.clone(),
            actions_place: f.actions_place.clone(),
            actions_align: f.actions_align.clone(),
            title_align: f.title_align.clone(),
            font_family: a.font_family.clone(),
            mono_family: a.mono_family.clone(),
            text_scale: view.text_scale,
            density: a.density.multiplier(),
            reduced_motion: view.reduced_motion,
        })
    }

    /// `base` (settingsd's look view) with this look's choices laid over
    /// it: what the window previews, drawn as every app draws the profile
    /// (`toolkit::session_look::follow_look`). An unchanged mode keeps the
    /// authored one (`auto` stays `auto`).
    pub fn over_view(&self, base: &settings::view::Look) -> settings::view::Look {
        let mut view = base.clone();
        let a = &mut view.appearance;
        a.scheme = self.scheme.name().into();
        a.style = self.style.map(|s| s.name().into());
        if base.mode().0.name() != self.mode.name() {
            a.mode = self.mode.name().into();
        }
        a.contrast = self.contrast.name().into();
        a.density =
            design::geometry::Density::from_name(density_step(self.density)).unwrap_or_default();
        a.font_family = self.font_family.clone();
        a.mono_family = self.mono_family.clone();
        let f = &mut view.framing;
        f.decorations = self.decorations.name().into();
        f.caption_side = self.captions.clone();
        f.style = self.window_style.clone();
        f.menu_style = self.menu_style.clone();
        f.menu_side = self.menu_side.clone();
        f.actions_place = self.actions_place.clone();
        f.actions_align = self.actions_align.clone();
        f.title_align = self.title_align.clone();
        view.text_scale = self.text_scale;
        view.reduced_motion = self.reduced_motion;
        view
    }

    /// The design context this look draws with.
    pub fn context(&self) -> DesignContext {
        DesignContext {
            scheme: self.scheme,
            mode: self.mode,
            contrast: self.contrast,
            style: self.style,
            ..DesignContext::default()
        }
    }

    /// The `settings.apply` changes that turn `base` into this look.
    pub fn changes(&self, base: &Look) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        let mut put = |key: &str, differs: bool, value: Value| {
            if differs {
                out.insert(change_path(key), value);
            }
        };
        put(
            "scheme",
            self.scheme != base.scheme,
            json!(self.scheme.name()),
        );
        // null is the scheme's own style.
        put(
            "style",
            self.style != base.style,
            json!(self.style.map(Style::name)),
        );
        put("mode", self.mode != base.mode, json!(self.mode.name()));
        put(
            "contrast",
            self.contrast != base.contrast,
            json!(self.contrast.name()),
        );
        put(
            "decorations",
            self.decorations != base.decorations,
            json!(self.decorations.name()),
        );
        put(
            "caption_side",
            self.captions != base.captions,
            json!(self.captions),
        );
        put(
            "window_style",
            self.window_style != base.window_style,
            json!(self.window_style),
        );
        put(
            "font_family",
            self.font_family != base.font_family,
            json!(self.font_family),
        );
        put(
            "menu_style",
            self.menu_style != base.menu_style,
            json!(self.menu_style),
        );
        put(
            "title_align",
            self.title_align != base.title_align,
            json!(self.title_align),
        );
        put(
            "actions_align",
            self.actions_align != base.actions_align,
            json!(self.actions_align),
        );
        put(
            "actions_place",
            self.actions_place != base.actions_place,
            json!(self.actions_place),
        );
        put(
            "menu_side",
            self.menu_side != base.menu_side,
            json!(self.menu_side),
        );
        put(
            "mono_family",
            self.mono_family != base.mono_family,
            json!(self.mono_family),
        );
        for (key, differs, value) in [
            (
                "accessibility.text_scale",
                self.text_scale != base.text_scale,
                json!(self.text_scale),
            ),
            (
                "appearance.density",
                self.density != base.density,
                json!(density_step(self.density)),
            ),
            (
                "accessibility.reduced_motion",
                self.reduced_motion != base.reduced_motion,
                json!(self.reduced_motion),
            ),
        ] {
            if differs {
                out.insert(key.into(), value);
            }
        }
        out
    }

    /// This look with the axes `args` names: `scheme`, `style` (a style or
    /// "own"), `mode`, `contrast`, `decorations`, `caption_side`, `window_style`, font families,
    /// text scale, density and reduced motion. An absent key leaves its axis;
    /// every present one is checked before any applies.
    pub fn with(&self, args: &Value) -> Result<Self, String> {
        fn axis<T>(
            args: &Value,
            key: &str,
            from_name: fn(&str) -> Option<T>,
        ) -> Result<Option<T>, String> {
            match args.get(key) {
                None => Ok(None),
                Some(Value::String(name)) => named(key, name, from_name).map(Some),
                Some(_) => Err(format!("{key} must be a name")),
            }
        }
        fn style(name: &str) -> Option<Option<Style>> {
            if name == "own" {
                Some(None)
            } else {
                Style::from_name(name).map(Some)
            }
        }
        let mut look = self.clone();
        if let Some(scheme) = axis(args, "scheme", Scheme::from_name)? {
            look.scheme = scheme;
        }
        if let Some(style) = axis(args, "style", style)? {
            look.style = style;
        }
        if let Some(mode) = axis(args, "mode", Mode::from_name)? {
            look.mode = mode;
        }
        if let Some(contrast) = axis(args, "contrast", Contrast::from_name)? {
            look.contrast = contrast;
        }
        if let Some(decorations) = axis(args, "decorations", Decorations::from_name)? {
            look.decorations = decorations;
        }
        if let Some(value) = args.get("window_style") {
            let name = value
                .as_str()
                .filter(|name| design::WindowStyle::valid_name(name))
                .ok_or("window_style must be a plain name of up to 64 bytes")?;
            look.window_style = name.into();
        }
        for (key, slot, valid) in [
            (
                "actions_place",
                &mut look.actions_place,
                design::ActionsPlace::valid_name as fn(&str) -> bool,
            ),
            (
                "actions_align",
                &mut look.actions_align,
                design::ActionsAlign::valid_name as fn(&str) -> bool,
            ),
            (
                "title_align",
                &mut look.title_align,
                design::TitleAlign::valid_name as fn(&str) -> bool,
            ),
            (
                "caption_side",
                &mut look.captions,
                CaptionSide::valid_name as fn(&str) -> bool,
            ),
            (
                "menu_style",
                &mut look.menu_style,
                design::MenuStyle::valid_name as fn(&str) -> bool,
            ),
            (
                "menu_side",
                &mut look.menu_side,
                design::MenuSide::valid_name as fn(&str) -> bool,
            ),
        ] {
            if let Some(value) = args.get(key) {
                *slot = value
                    .as_str()
                    .filter(|name| valid(name))
                    .ok_or_else(|| format!("{key} must be a plain name of up to 64 bytes"))?
                    .into();
            }
        }
        for (key, family) in [
            ("font_family", &mut look.font_family),
            ("mono_family", &mut look.mono_family),
        ] {
            if let Some(value) = args.get(key) {
                *family = match value {
                    Value::Null => None,
                    Value::String(name)
                        if !name.is_empty()
                            && name.len() <= 128
                            && name.trim() == name.as_str()
                            && !name.chars().any(char::is_control) =>
                    {
                        Some(name.clone())
                    }
                    _ => {
                        return Err(format!(
                            "{key} must be null or a plain name of up to 128 bytes"
                        ));
                    }
                };
            }
        }
        for (key, number, max) in [
            ("text_scale", &mut look.text_scale, 3.0),
            ("density", &mut look.density, 2.0),
        ] {
            if let Some(value) = args.get(key) {
                *number = value
                    .as_f64()
                    .filter(|n| (0.5..=max).contains(n))
                    .ok_or_else(|| format!("{key} must be a number from 0.5 to {max}"))?;
            }
        }
        // settingsd stores the density as its step (§4.1): the panel holds
        // the step's multiplier, so what it sends is what it reads back.
        if args.get("density").is_some() {
            look.density = design::geometry::Density::from_name(density_step(look.density))
                .unwrap_or_default()
                .multiplier();
        }
        if let Some(value) = args.get("reduced_motion") {
            look.reduced_motion = value.as_bool().ok_or("reduced_motion must be a boolean")?;
        }
        Ok(look)
    }
}

impl Serialize for Look {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        json!({"scheme":self.scheme.name(),"style":self.style.map_or("own", Style::name),
            "mode":self.mode.name(),"contrast":self.contrast.name(),
            "decorations":self.decorations.name(),"caption_side":self.captions,
            "window_style":self.window_style,
            "title_align":self.title_align,
            "actions_align":self.actions_align,
            "actions_place":self.actions_place,
            "menu_style":self.menu_style,"menu_side":self.menu_side,
            "font_family":self.font_family,"mono_family":self.mono_family,
            "text_scale":self.text_scale,"density":self.density,"reduced_motion":self.reduced_motion})
        .serialize(s)
    }
}

/// settingsd as the panel last saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Unknown,
    Available,
    /// Not registered on the Bus: the panel says how to start it.
    Missing,
}

/// One axis of a [`Look`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Scheme,
    Style,
    Mode,
    Contrast,
    Decorations,
    CaptionSide,
    WindowStyle,
    MenuStyle,
    MenuSide,
    ActionsPlace,
    ActionsAlign,
    TitleAlign,
    FontFamily,
    MonoFamily,
    TextScale,
    Density,
    ReducedMotion,
}

impl Axis {
    pub const ALL: [Axis; 17] = [
        Axis::Scheme,
        Axis::Style,
        Axis::Mode,
        Axis::Contrast,
        Axis::Decorations,
        Axis::CaptionSide,
        Axis::WindowStyle,
        Axis::MenuStyle,
        Axis::MenuSide,
        Axis::ActionsPlace,
        Axis::ActionsAlign,
        Axis::TitleAlign,
        Axis::FontFamily,
        Axis::MonoFamily,
        Axis::TextScale,
        Axis::Density,
        Axis::ReducedMotion,
    ];

    /// `a` and `b` differ on this axis.
    pub fn differs(self, a: &Look, b: &Look) -> bool {
        match self {
            Axis::Scheme => a.scheme != b.scheme,
            Axis::Style => a.style != b.style,
            Axis::Mode => a.mode != b.mode,
            Axis::Contrast => a.contrast != b.contrast,
            Axis::Decorations => a.decorations != b.decorations,
            Axis::CaptionSide => a.captions != b.captions,
            Axis::WindowStyle => a.window_style != b.window_style,
            Axis::MenuStyle => a.menu_style != b.menu_style,
            Axis::MenuSide => a.menu_side != b.menu_side,
            Axis::ActionsPlace => a.actions_place != b.actions_place,
            Axis::ActionsAlign => a.actions_align != b.actions_align,
            Axis::TitleAlign => a.title_align != b.title_align,
            Axis::FontFamily => a.font_family != b.font_family,
            Axis::MonoFamily => a.mono_family != b.mono_family,
            Axis::TextScale => a.text_scale != b.text_scale,
            Axis::Density => a.density != b.density,
            Axis::ReducedMotion => a.reduced_motion != b.reduced_motion,
        }
    }

    /// Copy this axis from `from` into `to`.
    fn copy(self, to: &mut Look, from: &Look) {
        match self {
            Axis::Scheme => to.scheme = from.scheme,
            Axis::Style => to.style = from.style,
            Axis::Mode => to.mode = from.mode,
            Axis::Contrast => to.contrast = from.contrast,
            Axis::Decorations => to.decorations = from.decorations,
            Axis::CaptionSide => to.captions = from.captions.clone(),
            Axis::WindowStyle => to.window_style = from.window_style.clone(),
            Axis::MenuStyle => to.menu_style = from.menu_style.clone(),
            Axis::MenuSide => to.menu_side = from.menu_side.clone(),
            Axis::ActionsPlace => to.actions_place = from.actions_place.clone(),
            Axis::ActionsAlign => to.actions_align = from.actions_align.clone(),
            Axis::TitleAlign => to.title_align = from.title_align.clone(),
            Axis::FontFamily => to.font_family = from.font_family.clone(),
            Axis::MonoFamily => to.mono_family = from.mono_family.clone(),
            Axis::TextScale => to.text_scale = from.text_scale,
            Axis::Density => to.density = from.density,
            Axis::ReducedMotion => to.reduced_motion = from.reduced_motion,
        }
    }
}

/// The person's edits: the look they were made on and the look they leave.
/// An axis is edited when the two differ there; every other axis follows
/// settingsd.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Draft {
    pub base: Look,
    pub look: Look,
}

impl Draft {
    pub fn edits(&self) -> Vec<Axis> {
        Axis::ALL
            .into_iter()
            .filter(|a| a.differs(&self.base, &self.look))
            .collect()
    }
}

/// A submitted apply: its operation id and what it asked for.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Submitted {
    pub operation: String,
    pub look: Look,
    pub axes: Vec<Axis>,
}

impl Submitted {
    fn section(&self) -> crate::Section {
        if self.axes.iter().any(|a| {
            matches!(
                a,
                Axis::Decorations
                    | Axis::CaptionSide
                    | Axis::WindowStyle
                    | Axis::MenuStyle
                    | Axis::MenuSide
                    | Axis::TitleAlign
                    | Axis::ActionsAlign
                    | Axis::ActionsPlace
            )
        }) {
            crate::Section::Windows
        } else if self
            .axes
            .iter()
            .any(|a| matches!(a, Axis::TextScale | Axis::ReducedMotion))
        {
            crate::Section::Accessibility
        } else if self
            .axes
            .iter()
            .any(|a| matches!(a, Axis::FontFamily | Axis::MonoFamily))
        {
            crate::Section::TextAndFonts
        } else {
            crate::Section::Appearance
        }
    }
    /// `current` shows every axis this apply asked for.
    fn shown_in(&self, current: &Look) -> bool {
        self.axes.iter().all(|a| !a.differs(&self.look, current))
    }
}

/// One call to settingsd.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Call {
    Read,
    Apply(Submitted),
    /// Settle an apply whose reply was lost, by its receipt.
    Receipt(Submitted),
    /// One terminal edit (the Terminal group), under its operation id.
    Terminal(String, Edit),
    /// One email change (Apps → Email), under its operation id.
    Email(String, EmailEdit),
    /// One text-style change (the Text section), under its operation id.
    Text(String, crate::text::TextEdit),
}

impl Call {
    fn command_name(&self) -> &'static str {
        match self {
            Call::Read => "settings.get",
            Call::Apply(_) => "settings.apply",
            Call::Receipt(_) => "settings.status",
            Call::Terminal(_, Edit::Reset) | Call::Email(_, EmailEdit::Reset) => "settings.reset",
            Call::Terminal(_, Edit::Set(..))
            | Call::Email(_, EmailEdit::Apply(_))
            | Call::Text(..) => "settings.apply",
        }
    }
}

/// How many receipt lookups quitting waits for before it closes anyway.
const QUIT_RECEIPT_TRIES: u32 = 3;

/// The Appearance panel's state.
#[derive(Clone, Debug)]
pub struct Appearance {
    /// The profile edited ([`PROFILE`]); its instance is adopted from
    /// settingsd's answer when the first guess (the host name) is not the
    /// one it serves. The profile is never adopted.
    pub binding: Binding,
    retargeted: bool,
    pub settings: Availability,
    /// The latest settings failure, kept separate from Software status.
    pub problem: Option<String>,
    pub problem_source: Option<crate::Section>,
    pub status_source: crate::Section,
    pub settings_status: Option<String>,
    pub problem_missing: bool,
    /// The look settingsd holds.
    pub current: Option<Look>,
    /// The terminal's preferences as settingsd holds them, from the same
    /// read as the look; the Terminal group shows these, never a copy.
    pub terminal: Option<TerminalPrefs>,
    /// The email app's settings as settingsd holds them, from the same read;
    /// the Email section shows these, never a copy.
    pub email: Option<settings::email::Email>,
    /// The Ced app's settings as settingsd holds them, from the same read;
    /// the Text section shows these.
    pub ced: Option<settings::content::Ced>,
    /// The session's interface font weight and size as settingsd holds them,
    /// from the same read; the Fonts section shows these.
    pub interface_font: Option<InterfaceFont>,
    /// The profile names a design package of its own (`appearance.source`):
    /// the window keeps the session theme rather than the embedded design.
    pub custom_source: bool,
    /// The profile's key bindings, per application, from the same read.
    pub keymaps: settings::keymap::Keymaps,
    /// settingsd's look view from the same read: the accent, tints,
    /// geometry and tunes the panel does not edit, under the preview.
    pub view: Option<settings::view::Look>,
    /// The fence for the next apply: settingsd's incarnation and revision.
    fence: Option<(String, Revision)>,
    /// The person's edits, until applied or reverted.
    pub draft: Option<Draft>,
    /// Draft axes an unsettled apply touched when a close stopped: whether
    /// the session took it is unknown, so the draft's value there is the
    /// person's intent even where it equals `current`. Only the read that
    /// decides (`syncing` holds every apply until then) clears them.
    pinned: Vec<Axis>,
    /// Edited axes another writer changed too, to something else: nothing
    /// is applied until the person keeps their edits or reverts.
    pub conflicts: Vec<Axis>,
    /// An apply whose outcome is unknown: settled by its receipt before
    /// anything else is applied.
    pub uncertain: Option<Submitted>,
    /// An apply settingsd holds no receipt for (evicted, or never
    /// accepted): the next read tells whether the session shows it.
    confirm: Option<Submitted>,
    /// A valid read must come before the next apply: an apply succeeded
    /// and the read that shows its result has not, or a conflict refused
    /// the fence. Only a good read clears it.
    syncing: bool,
    /// The latest apply until a read shows the session after it. Edits are
    /// applied at once: an edit leaves the draft when it is sent, and the
    /// panel shows the expected look (settingsd's with this laid over it).
    pub sent: Option<Submitted>,
    /// The last apply was refused or could not be sent: its edits are back
    /// in the draft and wait for the person (Try again, Revert or an edit),
    /// rather than being retried in a loop.
    pub hold: bool,
    /// A close stopped for unapplied edits: the next one discards them.
    discard_on_quit: bool,
    pub(crate) job: Option<(u64, Call)>,
    reread: bool,
    applies: u64,
    quit_tries: u32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            binding: Binding {
                instance: "host".into(),
                profile: PROFILE.into(),
            },
            retargeted: false,
            settings: Availability::Unknown,
            problem: None,
            problem_source: None,
            status_source: crate::Section::Appearance,
            settings_status: None,
            problem_missing: false,
            current: None,
            terminal: None,
            email: None,
            ced: None,
            interface_font: None,
            custom_source: false,
            keymaps: Default::default(),
            view: None,
            fence: None,
            draft: None,
            pinned: Vec::new(),
            conflicts: Vec::new(),
            uncertain: None,
            confirm: None,
            syncing: false,
            sent: None,
            hold: false,
            discard_on_quit: false,
            job: None,
            reread: false,
            applies: 0,
            quit_tries: 0,
        }
    }
}

impl Appearance {
    /// The look the panel and the window show: the draft, else the
    /// expected look.
    pub fn shown(&self) -> Option<Look> {
        self.draft
            .as_ref()
            .map(|d| d.look.clone())
            .or_else(|| self.expected())
    }

    /// settingsd's look with the apply in flight (or awaiting its read)
    /// laid over it: what the session is about to be.
    pub fn expected(&self) -> Option<Look> {
        let mut look = self.current.clone()?;
        if let Some(sent) = &self.sent {
            for axis in &sent.axes {
                axis.copy(&mut look, &sent.look);
            }
        }
        Some(look)
    }

    /// The person's edits wait for them: a held refusal or a conflict.
    pub fn waiting(&self) -> bool {
        self.draft.is_some() && (self.hold || !self.conflicts.is_empty())
    }

    /// An apply did not happen (refused, not sent, or not shown by the
    /// session after a lost reply): its axes go back into the draft, under
    /// any newer edit of the same axis.
    fn unsend(&mut self) {
        self.return_sent(false);
    }

    /// An apply still unsettled when a close stopped: its axes go back into
    /// the draft like [`Self::unsend`], but `current` may be stale there, so
    /// they stay pinned until the deciding read. A reversal (Studio, Forest
    /// lost, Studio again) is kept rather than dropped as no change.
    fn unsend_unsettled(&mut self) {
        self.return_sent(true);
    }

    fn return_sent(&mut self, pin: bool) {
        let (Some(sent), Some(current)) = (self.sent.take(), self.current.clone()) else {
            return;
        };
        let mut draft = self.draft.take().unwrap_or(Draft {
            base: current.clone(),
            look: current.clone(),
        });
        for axis in &sent.axes {
            let reedited = axis.differs(&draft.look, &draft.base);
            axis.copy(&mut draft.base, &current);
            if !reedited {
                axis.copy(&mut draft.look, &sent.look);
            }
            if pin && !self.pinned.contains(axis) {
                self.pinned.push(*axis);
            }
        }
        self.draft = self.kept(draft);
    }

    /// `draft`, unless it holds nothing: no edit and no pinned axis.
    fn kept(&self, draft: Draft) -> Option<Draft> {
        (draft.base != draft.look || !self.pinned.is_empty()).then_some(draft)
    }

    /// A settingsd call is in flight.
    pub fn working(&self) -> bool {
        self.job.is_some()
    }

    /// An apply is in flight: the controls wait for its answer.
    pub fn applying(&self) -> bool {
        matches!(self.job, Some((_, Call::Apply(_))))
    }

    /// A fresh operation id for one apply, unique to this process and run.
    pub(crate) fn operation(&mut self) -> String {
        self.applies += 1;
        format!(
            "prefs-{}-{}-{}",
            std::process::id(),
            self.applies,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        )
    }

    /// A terminal edit may go out: settingsd read, its fence current, and
    /// nothing in flight, uncertain or awaiting a read.
    pub(crate) fn terminal_ready(&self) -> bool {
        self.settings == Availability::Available
            && self.current.is_some()
            && self.fence.is_some()
            && self.job.is_none()
            && self.uncertain.is_none()
            && self.confirm.is_none()
            && !self.syncing
    }

    /// A group's edit settled (Email): read again before the next apply.
    /// The read shows whether it landed and gives the fence for the next.
    pub(crate) fn settle_by_read(&mut self) {
        self.syncing = true;
        self.reread = true;
    }

    /// The revision the panel was read at.
    pub fn revision(&self) -> Option<u64> {
        self.fence.as_ref().map(|(_, r)| r.0)
    }

    /// settingsd's new look: the draft follows it on every axis the person
    /// did not edit. An edited axis another writer changed to something
    /// else is a conflict; one changed to the person's value is no longer
    /// an edit. A pinned axis is the person's intent over an apply of their
    /// own, never a conflict: it stays an edit unless the session shows it.
    /// Only a deciding read (`decides`) retires the pins; until then a pinned
    /// axis is kept even where it equals `current`.
    fn rebase(&mut self, current: Look, decides: bool) {
        let pinned = if decides {
            std::mem::take(&mut self.pinned)
        } else {
            self.pinned.clone()
        };
        let Some(draft) = self.draft.take() else {
            return;
        };
        let mut edits = draft.edits();
        for axis in &pinned {
            if !edits.contains(axis) {
                edits.push(*axis);
            }
        }
        for &axis in &edits {
            if !pinned.contains(&axis)
                && axis.differs(&current, &draft.base)
                && axis.differs(&current, &draft.look)
                && !self.conflicts.contains(&axis)
            {
                self.conflicts.push(axis);
            }
        }
        let mut look = current.clone();
        for axis in &edits {
            axis.copy(&mut look, &draft.look);
        }
        self.draft = self.kept(Draft {
            base: current,
            look,
        });
        self.conflicts.retain(|a| {
            self.draft
                .as_ref()
                .is_some_and(|d| a.differs(&d.base, &d.look))
        });
    }
}

/// What a settingsd call answered (and, for the Email section, a call to
/// the email app or to apps: their refusals carry a JSON body too).
pub(crate) enum Answer {
    /// rc 0 with a JSON body.
    Ok(Value),
    /// settingsd refused (rc 10, a JSON body with a `status`).
    Refused(Value),
    /// Not registered on the Bus.
    Missing,
    /// Refused before reaching settingsd: nothing happened.
    NotSent(String),
    /// The reply was lost after the call may have arrived.
    Lost(String),
}

pub(crate) fn answer(result: Result<Reply, CallError>) -> Answer {
    let reply = match result {
        Ok(reply) => reply,
        Err(error) if error.outcome_unknown => return Answer::Lost(error.message),
        Err(error) => return Answer::NotSent(error.message),
    };
    if reply.body.trim().is_empty() {
        let not_found = reply
            .error
            .as_deref()
            .is_none_or(|e| e.contains("not found"));
        if reply.rc == 10 && not_found {
            return Answer::Missing;
        }
        return Answer::NotSent(
            reply
                .error
                .unwrap_or_else(|| format!("refused by the broker (rc {})", reply.rc)),
        );
    }
    let value: Value = serde_json::from_str(&reply.body).unwrap_or(Value::String(reply.body));
    if reply.rc == 0 {
        Answer::Ok(value)
    } else {
        Answer::Refused(value)
    }
}

/// A refusal's text: its message, else its diagnostics, else its status.
pub(crate) fn refusal(value: &Value) -> String {
    if let Some(message) = value["message"].as_str() {
        return message.to_owned();
    }
    if let Some(list) = value["diagnostics"].as_array() {
        let lines: Vec<String> = list
            .iter()
            .map(|d| {
                format!(
                    "{}: {}",
                    d["path"].as_str().unwrap_or_default(),
                    d["message"].as_str().unwrap_or_default()
                )
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("; ");
        }
    }
    value["status"]
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// What one `settings.get {view: values}` reply holds for the panels: the
/// fence, the look's typed view and the app domains (contract 1.0.0).
struct Read {
    incarnation: String,
    revision: Revision,
    look: settings::view::Look,
    keymaps: settings::keymap::Keymaps,
    domains: settings::view::AppDomains,
}

/// The values and look in a `settings.get` reply (its `values` view).
fn snapshot(value: &Value) -> Result<(Read, Look), String> {
    use settings::view::View;
    let incarnation = value["incarnation"]
        .as_str()
        .ok_or("values: no incarnation")?
        .to_owned();
    let revision: Revision = serde_json::from_value(value["revision"].clone())
        .map_err(|e| format!("values: revision: {e}"))?;
    let values: settings::registry::Values =
        serde_json::from_value(value["values"].clone()).map_err(|e| format!("values: {e}"))?;
    let view = settings::view::Look::of(&values);
    let look = Look::from_view(&view)?;
    let read = Read {
        incarnation,
        revision,
        look: view,
        keymaps: settings::view::Keymaps::of(&values).apps,
        domains: settings::view::AppDomains::of(&values),
    };
    Ok((read, look))
}

impl Engine {
    /// Apply one control over the latest shown draft, preserving other edits.
    pub fn edit_appearance(&mut self, edit: AppearanceEdit) {
        let _ = self.set_look(&edit.args());
    }

    /// Start the panel on the profile of settingsd `instance` (the host
    /// name; adopted from settingsd's answer when it serves another).
    pub fn start_appearance(&mut self, instance: &str) {
        let binding = Binding {
            instance: instance.to_owned(),
            profile: PROFILE.into(),
        };
        if binding.validate().is_ok() {
            self.look.binding = binding;
        }
        self.read_look();
    }

    /// Read the profile again (a hint arrived, the Bus reconnected, Refresh),
    /// or, with an apply still uncertain, ask for its receipt first. While
    /// quitting only a receipt is asked for (a bounded number of times), or
    /// the read that lets accepted edits drain.
    pub fn read_look(&mut self) {
        if self.look.job.is_some() {
            self.look.reread = true;
            return;
        }
        if !self.connected {
            return;
        }
        match self.look.uncertain.clone() {
            Some(submitted) => {
                if self.quitting {
                    if self.look.quit_tries >= QUIT_RECEIPT_TRIES {
                        return;
                    }
                    self.look.quit_tries += 1;
                }
                self.call_settings(Call::Receipt(submitted));
            }
            // While quitting, only the read that lets accepted edits drain.
            None if !self.quitting
                || self.look.confirm.is_some()
                || (self.look.syncing && self.draining_look()) =>
            {
                self.call_settings(Call::Read);
            }
            None => {}
        }
    }

    pub(crate) fn record_settings_status(&mut self) {
        if self.failed {
            self.look.settings_status = None;
            self.look.problem = Some(self.status.clone());
            self.look.problem_source = Some(self.look.status_source);
            self.look.problem_missing = false;
        } else {
            self.look.settings_status = Some(self.status.clone());
        }
    }

    fn clear_settings_problem(&mut self, source: crate::Section) {
        if self.look.problem_source == Some(source) {
            self.look.problem = None;
            self.look.problem_source = None;
            self.look.problem_missing = false;
        }
    }

    pub fn dismiss_settings_problem(&mut self) {
        if self.look.problem.is_some() && self.look.settings_status == self.look.problem {
            self.look.settings_status = None;
        }
        if self.look.problem.as_ref() == Some(&self.status) {
            self.status.clear();
            self.failed = false;
        }
        self.look.problem = None;
        self.look.problem_source = None;
        self.look.problem_missing = false;
    }

    fn clear_look_problem(&mut self) {
        if let Some(
            source @ (crate::Section::Appearance
            | crate::Section::Windows
            | crate::Section::TextAndFonts
            | crate::Section::Accessibility),
        ) = self.look.problem_source
        {
            self.clear_settings_problem(source);
        }
    }

    pub(crate) fn call_settings(&mut self, call: Call) {
        let ticket = self.next();
        let body = match &call {
            Call::Read => json!({"binding":self.look.binding,"view":"values"}),
            Call::Receipt(submitted) => {
                json!({"binding":self.look.binding,"operation_id":submitted.operation})
            }
            Call::Terminal(operation, edit) => {
                let (incarnation, revision) = self.look.fence.clone().expect("a fenced apply");
                let (changes, reset) = match edit {
                    Edit::Set(path, value) => (
                        BTreeMap::from([((*path).to_owned(), value.clone())]),
                        Vec::new(),
                    ),
                    Edit::Reset => (BTreeMap::new(), vec!["terminal.*".to_owned()]),
                };
                serde_json::to_value(ApplyRequest {
                    binding: self.look.binding.clone(),
                    expected_incarnation: incarnation,
                    expected_revision: revision,
                    operation_id: operation.clone(),
                    changes,
                    reset,
                    request_digest: None,
                })
                .expect("an apply request serialises")
            }
            // Every path of one change in one fenced batch: settingsd checks
            // the group as it leaves them all, never a state between.
            Call::Text(operation, edit) => {
                let (incarnation, revision) = self.look.fence.clone().expect("a fenced apply");
                serde_json::to_value(ApplyRequest {
                    binding: self.look.binding.clone(),
                    expected_incarnation: incarnation,
                    expected_revision: revision,
                    operation_id: operation.clone(),
                    changes: BTreeMap::from([(edit.path.clone(), edit.value.clone())]),
                    reset: Vec::new(),
                    request_digest: None,
                })
                .expect("an apply request serialises")
            }
            Call::Email(operation, edit) => {
                let (incarnation, revision) = self.look.fence.clone().expect("a fenced apply");
                let (changes, reset) = match edit {
                    EmailEdit::Apply(changes) => (changes.clone(), Vec::new()),
                    EmailEdit::Reset => (BTreeMap::new(), vec!["email.*".to_owned()]),
                };
                serde_json::to_value(ApplyRequest {
                    binding: self.look.binding.clone(),
                    expected_incarnation: incarnation,
                    expected_revision: revision,
                    operation_id: operation.clone(),
                    changes,
                    reset,
                    request_digest: None,
                })
                .expect("an apply request serialises")
            }
            Call::Apply(submitted) => {
                let (incarnation, revision) = self.look.fence.clone().expect("a fenced apply");
                let current = self.look.current.clone().unwrap_or_default();
                let changes = submitted
                    .look
                    .changes(&current)
                    .into_iter()
                    .filter(|(key, _)| submitted.axes.iter().any(|a| key == axis_key(*a)))
                    .collect();
                serde_json::to_value(ApplyRequest {
                    binding: self.look.binding.clone(),
                    expected_incarnation: incarnation,
                    expected_revision: revision,
                    operation_id: submitted.operation.clone(),
                    changes,
                    reset: Vec::new(),
                    request_digest: None,
                })
                .expect("an apply request serialises")
            }
        };
        let command_name = call.command_name();
        self.look.job = Some((ticket, call));
        self.effects.push(Effect::Settings {
            ticket,
            command: command_name,
            body,
        });
    }

    /// The panel takes edits: read, no dialog, not quitting. An edit made
    /// while an apply is in flight or settling is measured against the
    /// expected look and applied once it settles.
    pub fn can_edit_look(&self) -> bool {
        self.look.current.is_some() && self.ui.dialog.is_none() && !self.quitting
    }

    /// Edit, and apply at once: the panel's controls and
    /// `prefs.appearance.set`. `look` is the whole look the person now
    /// wants; only the axes where it differs from what was shown are edits.
    pub fn edit_look(&mut self, look: Look) {
        if !self.can_edit_look() {
            return;
        }
        let Some(expected) = self.look.expected() else {
            return;
        };
        let shown = self.look.shown().unwrap_or_else(|| expected.clone());
        self.look.status_source = if look.decorations != shown.decorations
            || look.captions != shown.captions
            || look.window_style != shown.window_style
            || look.menu_style != shown.menu_style
            || look.menu_side != shown.menu_side
            || look.title_align != shown.title_align
            || look.actions_align != shown.actions_align
            || look.actions_place != shown.actions_place
        {
            crate::Section::Windows
        } else if look.text_scale != shown.text_scale || look.reduced_motion != shown.reduced_motion
        {
            crate::Section::Accessibility
        } else if look.font_family != shown.font_family || look.mono_family != shown.mono_family {
            crate::Section::TextAndFonts
        } else {
            crate::Section::Appearance
        };
        let base = self
            .look
            .draft
            .as_ref()
            .map_or(expected, |d| d.base.clone());
        self.look.draft = self.look.kept(Draft { base, look });
        let draft = self.look.draft.as_ref();
        self.look
            .conflicts
            .retain(|a| draft.is_some_and(|d| a.differs(&d.base, &d.look)));
        self.settle_if_retired(self.look.hold);
        self.look.hold = false;
        self.look.discard_on_quit = false;
        self.pump_look();
    }

    /// A held draft (a refusal, or a close that stopped) has just come to
    /// nothing: the session already shows the person's choice. The hold and
    /// its failure status no longer describe anything, so they go.
    fn settle_if_retired(&mut self, held: bool) {
        if held && self.look.draft.is_none() {
            self.clear_look_problem();
            self.look.hold = false;
            self.look.discard_on_quit = false;
            if self.look.problem.is_none() {
                self.status = self.label("settled-look", &[]);
                self.failed = false;
                self.record_settings_status();
            }
        }
    }

    /// Send accepted edits now if settingsd can take them. Edits were
    /// accepted before any dialog opened or quitting began, so neither
    /// holds them back: they drain (quitting waits for them).
    pub(crate) fn pump_look(&mut self) {
        if !self.look.hold && self.may_send_look() {
            self.send_look();
        }
    }

    /// Edits are queued and settingsd can take them: read and idle on a
    /// fresh fence (the last apply read back, no refused conflict pending a
    /// read), no conflict, no uncertainty.
    fn may_send_look(&self) -> bool {
        self.look.draft.is_some()
            && self.look.conflicts.is_empty()
            && self.look.fence.is_some()
            && self.look.job.is_none()
            && self.look.uncertain.is_none()
            && self.look.confirm.is_none()
            && !self.look.syncing
            && self.look.settings == Availability::Available
            && self.connected
    }

    /// Accepted edits still to go out, which quitting waits for: queued and
    /// not held by the person (a refusal or a conflict).
    pub(crate) fn draining_look(&self) -> bool {
        self.look.draft.is_some() && !self.look.hold && self.look.conflicts.is_empty()
    }

    /// Close, unless accepted Appearance edits would be lost: the first
    /// close then stops, keeps them (held: Apply tries again, Revert drops
    /// them) and says so; a second close discards them.
    pub(crate) fn close_now(&mut self) {
        // An apply still unsettled (uncertain, or awaiting the read that
        // decides) counts too: its edits return to the draft, pinned, and
        // the read that decides runs now. Until it answers nothing is
        // applied; then they are applied again (fenced, so never twice) or
        // dropped.
        let unsettled = self.look.uncertain.is_some() || self.look.confirm.is_some();
        if (self.look.draft.is_some() || unsettled) && !self.look.discard_on_quit {
            self.quitting = false;
            if unsettled {
                self.look.uncertain = None;
                self.look.confirm = None;
                self.look.unsend_unsettled();
                self.look.syncing = true;
                self.read_look();
            }
            self.look.discard_on_quit = true;
            self.look.hold = true;
            self.status = self.label("look-kept-open", &[]);
            self.failed = true;
            self.record_settings_status();
            return;
        }
        self.effects.push(Effect::Exit);
    }

    /// A close after one that stopped for unapplied edits: drop them.
    pub(crate) fn discard_look_on_quit(&mut self) {
        if self.look.discard_on_quit {
            self.look.uncertain = None;
            self.look.confirm = None;
            self.look.sent = None;
            self.look.draft = None;
            self.look.pinned.clear();
            self.look.conflicts.clear();
            self.look.hold = false;
            // The read deciding the dropped edits no longer matters: the
            // close does not wait for it (its late reply is ignored).
            if matches!(self.look.job, Some((_, Call::Read))) {
                self.look.job = None;
                self.look.reread = false;
            }
        }
    }

    /// Quitting: start sending accepted edits still queued (a read first
    /// when the fence is stale); the quit waits for the calls.
    pub(crate) fn drain_look(&mut self) {
        if self.draining_look() && self.look.job.is_none() {
            if self.look.syncing {
                self.read_look();
            } else {
                self.pump_look();
            }
        }
    }

    /// Apply (Try again) is available: the person's command, so not under
    /// a dialog nor while quitting.
    pub fn can_apply_look(&self) -> bool {
        self.may_send_look() && self.ui.dialog.is_none() && !self.quitting
    }

    pub fn can_revert_look(&self) -> bool {
        self.look.draft.is_some() && self.can_edit_look()
    }

    /// The edits stand over another writer's change: the conflict is
    /// acknowledged and they are applied.
    pub fn can_keep_look(&self) -> bool {
        !self.look.conflicts.is_empty() && self.can_edit_look()
    }

    /// An uncertain apply can be checked again now.
    pub fn can_recheck_look(&self) -> bool {
        self.look.uncertain.is_some() && self.look.job.is_none() && self.connected
    }

    /// Write the edited axes to settingsd, fenced on the revision read
    /// (Try again, after a held refusal). They leave the draft as they go:
    /// the panel shows them as the expected look.
    pub fn apply_look(&mut self) {
        if self.can_apply_look() {
            self.send_look();
        }
    }

    fn send_look(&mut self) {
        self.look.hold = false;
        let Some(draft) = self.look.draft.take() else {
            return;
        };
        let operation = self.look.operation();
        self.status = self.label("applying-look", &[]);
        self.failed = false;
        self.record_settings_status();
        let submitted = Submitted {
            operation,
            axes: draft.edits(),
            look: draft.look,
        };
        self.look.sent = Some(submitted.clone());
        self.call_settings(Call::Apply(submitted));
    }

    /// Drop the draft: the window shows settingsd's look again.
    pub fn revert_look(&mut self) {
        if !self.can_revert_look() {
            return;
        }
        self.look.draft = None;
        self.look.pinned.clear();
        self.look.conflicts.clear();
        self.look.hold = false;
        self.clear_look_problem();
        if self.look.problem.is_none() {
            self.look.status_source = self.section();
            self.status = self.label("reverted-look", &[]);
            self.failed = false;
            self.record_settings_status();
        }
    }

    /// Keep the edits over the other writer's change, and apply them.
    pub fn keep_look(&mut self) {
        if self.can_keep_look() {
            self.look.conflicts.clear();
            self.look.hold = false;
            self.status = self.label("kept-look", &[]);
            self.failed = false;
            self.record_settings_status();
            self.pump_look();
        }
    }

    /// Ask again for an uncertain apply's receipt.
    pub fn recheck_look(&mut self) {
        if self.can_recheck_look() {
            self.read_look();
        }
    }

    /// settingsd call `ticket` finished.
    pub fn settled(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        let Some((expected, call)) = self.look.job.clone() else {
            return;
        };
        if expected != ticket {
            return;
        }
        self.look.job = None;
        self.look.status_source = match &call {
            Call::Apply(submitted) | Call::Receipt(submitted) => submitted.section(),
            Call::Terminal(..) => crate::Section::Terminal,
            Call::Email(..) => crate::Section::Email,
            Call::Text(_, edit) => edit.section(),
            _ => self.look.status_source,
        };
        let answer = answer(result);
        if matches!(answer, Answer::Ok(_)) && !matches!(call, Call::Read | Call::Receipt(_)) {
            self.clear_settings_problem(self.look.status_source);
        }
        if let Answer::Missing = answer {
            self.look.settings = Availability::Missing;
            if !matches!(call, Call::Read) {
                self.status = self.label("settings-missing", &[]);
                self.failed = true;
                self.record_settings_status();
                self.look.problem_missing = true;
            }
            if matches!(call, Call::Apply(_)) {
                self.look.unsend();
                self.look.hold = true;
            }
        } else {
            match call {
                Call::Read => self.read_answered(answer),
                Call::Apply(submitted) => self.apply_answered(submitted, answer),
                Call::Receipt(submitted) => self.receipt_answered(submitted, answer),
                Call::Terminal(..) => self.terminal_answered(answer),
                Call::Text(..) => self.text_answered(answer),
                Call::Email(..) => self.email_answered(answer),
            }
        }
        if self.look.reread && self.look.job.is_none() {
            self.look.reread = false;
            self.read_look();
        }
        // Edits made meanwhile go out once this settled.
        self.pump_look();
        if self.quitting && !self.busy() {
            self.close_now();
        }
    }

    fn read_answered(&mut self, answer: Answer) {
        match answer {
            Answer::Ok(value) => match snapshot(&value) {
                Ok((read, look)) => {
                    if self.look.problem_missing
                        && let Some(source) = self.look.problem_source
                    {
                        self.clear_settings_problem(source);
                    }
                    self.look.settings = Availability::Available;
                    self.look.keymaps = read.keymaps;
                    self.look.fence = Some((read.incarnation, read.revision));
                    self.look.current = Some(look.clone());
                    self.look.terminal = Some(read.domains.terminal);
                    self.look.ced = Some(read.domains.ced);
                    self.look.interface_font = Some(InterfaceFont {
                        weight: read.look.appearance.font_weight,
                        size: read.look.appearance.font_size,
                    });
                    self.email_read(read.domains.email);
                    self.look.custom_source = read.look.appearance.source.is_some();
                    self.look.view = Some(read.look);
                    // A recovering settingsd may not show a write it has
                    // already taken: it cannot decide pinned axes, and
                    // nothing is applied until a read that can (its change
                    // hint, or Refresh, reads again).
                    let decides = value["recovering"] != true;
                    self.look.syncing = !decides && !self.look.pinned.is_empty();
                    // A lost apply settingsd holds no receipt for: the
                    // session decides. Not shown, its edits return held.
                    if let Some(submitted) = self.look.confirm.take() {
                        if submitted.shown_in(&look) {
                            self.status = self.label("applied-look", &[]);
                            self.failed = false;
                            self.record_settings_status();
                        } else {
                            self.look.unsend();
                            self.look.hold = true;
                            self.status = self.label("look-unconfirmed", &[]);
                            self.failed = true;
                            self.record_settings_status();
                        }
                    }
                    // The session after the last apply is read: nothing is
                    // in flight any more.
                    self.look.sent = None;
                    let held = self.look.hold;
                    self.look.rebase(look, decides);
                    self.settle_if_retired(held);
                }
                Err(message) => self.look_failed(&message),
            },
            Answer::Refused(value)
                if value["status"] == "wrong_target" && !self.look.retargeted =>
            {
                // Only the instance is adopted: another profile is never
                // edited (nor its topic followed) by accident.
                match serde_json::from_value::<Binding>(value["binding"].clone()) {
                    Ok(binding) if binding.validate().is_ok() && binding.profile == PROFILE => {
                        self.look.binding.instance = binding.instance;
                        self.look.retargeted = true;
                        self.call_settings(Call::Read);
                    }
                    _ => self.look_failed(&refusal(&value)),
                }
            }
            Answer::Refused(value) => self.look_failed(&refusal(&value)),
            Answer::NotSent(message) | Answer::Lost(message) => self.look_failed(&message),
            Answer::Missing => {}
        }
    }

    fn apply_answered(&mut self, submitted: Submitted, answer: Answer) {
        match answer {
            Answer::Ok(_) => {
                self.look.syncing = true;
                self.status = self.label("applied-look", &[]);
                self.failed = false;
                self.record_settings_status();
                self.look.reread = true;
            }
            // Another writer was first: the edits return to the draft and
            // the read decides (a conflict, or applied again on the new fence).
            Answer::Refused(value) if value["status"] == "conflict" => {
                self.look.unsend();
                // The fence was refused: nothing more goes out until a
                // valid read gives a new one (a failed read keeps waiting;
                // Refresh reads again).
                self.look.syncing = true;
                self.status = self.label("look-conflict", &[]);
                self.failed = true;
                self.record_settings_status();
                self.look.reread = true;
            }
            Answer::Refused(value) if value["status"] == "outcome_unknown" => {
                self.uncertain_apply(submitted, &refusal(&value));
            }
            Answer::Lost(message) => self.uncertain_apply(submitted, &message),
            // Refused or not sent: nothing happened; the edits wait, held.
            Answer::Refused(value) => {
                self.look.unsend();
                self.look.hold = true;
                self.look_failed(&refusal(&value));
            }
            Answer::NotSent(message) => {
                self.look.unsend();
                self.look.hold = true;
                self.look_failed(&message);
            }
            Answer::Missing => {}
        }
    }

    fn uncertain_apply(&mut self, submitted: Submitted, message: &str) {
        self.look.uncertain = Some(submitted);
        self.status = self.label("look-uncertain", &[("message", message)]);
        self.failed = true;
        self.record_settings_status();
        // Ask for the receipt next (read_look does, while uncertain).
        self.look.reread = true;
    }

    fn receipt_answered(&mut self, submitted: Submitted, answer: Answer) {
        let operation = submitted.operation.clone();
        match answer {
            Answer::Ok(value)
                if value["status"] == "current"
                    && value["receipt"]["operation_id"] == operation.as_str() =>
            {
                self.look.uncertain = None;
                self.look.syncing = true;
                self.status = self.label("applied-look", &[]);
                self.failed = false;
                self.record_settings_status();
                self.look.reread = true;
            }
            // No receipt, and settingsd is not recovering: it may have been
            // evicted or never accepted. Not a verdict: the next read says
            // whether the session shows the change.
            Answer::Ok(value)
                if value["status"] == "unknown_operation" && value["recovering"] != true =>
            {
                self.look.uncertain = None;
                // A read must decide, even while quitting.
                self.look.syncing = true;
                self.look.confirm = Some(submitted);
                self.look.reread = true;
            }
            // Still unknown (settingsd recovering, or the lookup failed):
            // Check again, Refresh, a hint or a reconnection asks again.
            Answer::Ok(value) | Answer::Refused(value) => self.receipt_pending(&refusal(&value)),
            Answer::NotSent(message) | Answer::Lost(message) => self.receipt_pending(&message),
            Answer::Missing => {}
        }
    }

    fn receipt_pending(&mut self, message: &str) {
        self.status = self.label("look-uncertain", &[("message", message)]);
        self.failed = true;
        self.record_settings_status();
        if self.quitting {
            self.look.reread = true;
        }
    }

    fn look_failed(&mut self, message: &str) {
        self.status = self.label("look-failed", &[("message", message)]);
        self.failed = true;
        self.record_settings_status();
    }

    /// A terminal edit settled. Whatever the answer, the session is read
    /// again before the next apply: the read shows whether the edit landed
    /// (a lost reply is no verdict), and it gives the fence for the next.
    /// A text-style apply's answer: the same fenced path as the Terminal
    /// group, with its own words.
    fn text_answered(&mut self, answer: Answer) {
        match answer {
            Answer::Ok(_) => {
                self.status = self.label("applied-text", &[]);
                self.failed = false;
                self.record_settings_status();
            }
            Answer::Refused(value) if value["status"] == "conflict" => {
                self.status = self.label("text-conflict", &[]);
                self.failed = true;
                self.record_settings_status();
            }
            Answer::Refused(value) => {
                let message = refusal(&value);
                self.status = self.label("text-failed", &[("message", message.as_str())]);
                self.failed = true;
                self.record_settings_status();
            }
            Answer::NotSent(message) | Answer::Lost(message) => {
                self.status = self.label("text-failed", &[("message", message.as_str())]);
                self.failed = true;
                self.record_settings_status();
            }
            Answer::Missing => return,
        }
        self.look.syncing = true;
        self.look.reread = true;
    }

    fn terminal_answered(&mut self, answer: Answer) {
        match answer {
            Answer::Ok(_) => {
                self.status = self.label("applied-terminal", &[]);
                self.failed = false;
                self.record_settings_status();
            }
            Answer::Refused(value) if value["status"] == "conflict" => {
                self.status = self.label("terminal-conflict", &[]);
                self.failed = true;
                self.record_settings_status();
            }
            Answer::Refused(value) => {
                let message = refusal(&value);
                self.status = self.label("terminal-failed", &[("message", message.as_str())]);
                self.failed = true;
                self.record_settings_status();
            }
            Answer::NotSent(message) | Answer::Lost(message) => {
                self.status = self.label("terminal-failed", &[("message", message.as_str())]);
                self.failed = true;
                self.record_settings_status();
            }
            Answer::Missing => return,
        }
        self.look.syncing = true;
        self.look.reread = true;
    }

    /// The panel as `prefs.appearance` answers it.
    pub fn appearance_view(&self) -> Value {
        json!({"section":self.section(),"category":self.section().category(),
            "wallpaper":self.wallpaper,"settings":self.look.settings,"binding":self.look.binding,
            "revision":self.look.revision().map(|r| r.to_string()),
            "current":self.look.current,"draft":self.look.draft.as_ref().map(|d| &d.look),
            "edits":self.look.draft.as_ref().map(|d| d.edits()).unwrap_or_default(),
            "conflicts":self.look.conflicts,"shown":self.look.shown(),
            "effective":self.look.shown().map(|look| look.effective()),
            "expected":self.look.expected(),"sent":self.look.sent.as_ref().map(|s| &s.axes),
            "hold":self.look.hold,"problem":self.look.problem,"problem_source":self.look.problem_source.map(crate::Section::name),
            "custom_source":self.look.custom_source,
            "uncertain":self.look.uncertain.as_ref().map(|s| &s.operation),
            "working":self.look.job.as_ref().map(|(_, c)| c.command_name()),
            "can_apply":self.can_apply_look(),"can_revert":self.can_revert_look(),
            "can_keep":self.can_keep_look(),"can_recheck":self.can_recheck_look()})
    }

    /// `prefs.appearance.set`: change the shown look, applied at once.
    pub(crate) fn set_look(&mut self, args: &Value) -> Result<Value, (&'static str, String)> {
        let Some(base) = self.look.shown() else {
            return Err(("NOT_READY", self.label("look-not-ready", &[])));
        };
        if !self.can_edit_look() {
            return Err(("BUSY", self.label("busy", &[])));
        }
        let look = base.with(args).map_err(|e| ("ARGUMENT", e))?;
        self.edit_look(look);
        Ok(self.appearance_view())
    }
}

/// The settings.v2 path of a look field (design §4.1). Window chrome moved
/// under `window.` (window_style became `window.style`); the rest of the
/// look stays under `appearance.`.
fn change_path(field: &str) -> String {
    match field {
        "window_style" => "window.style".into(),
        "decorations" | "caption_side" | "menu_style" | "menu_side" | "title_align"
        | "actions_align" | "actions_place" => format!("window.{field}"),
        _ => format!("appearance.{field}"),
    }
}

/// The v2 density step for a Prefs density: under 0.925 is compact, up to and
/// including 1.1 comfortable, above that spacious (design §4.1).
fn density_step(density: f64) -> &'static str {
    if density < 0.925 {
        "compact"
    } else if density <= 1.1 {
        "comfortable"
    } else {
        "spacious"
    }
}

/// An axis's full `settings.apply` key.
fn axis_key(axis: Axis) -> &'static str {
    match axis {
        Axis::Scheme => "appearance.scheme",
        Axis::Style => "appearance.style",
        Axis::Mode => "appearance.mode",
        Axis::Contrast => "appearance.contrast",
        Axis::Decorations => "window.decorations",
        Axis::CaptionSide => "window.caption_side",
        Axis::WindowStyle => "window.style",
        Axis::MenuStyle => "window.menu_style",
        Axis::MenuSide => "window.menu_side",
        Axis::ActionsPlace => "window.actions_place",
        Axis::ActionsAlign => "window.actions_align",
        Axis::TitleAlign => "window.title_align",
        Axis::FontFamily => "appearance.font_family",
        Axis::MonoFamily => "appearance.mono_family",
        Axis::TextScale => "accessibility.text_scale",
        Axis::Density => "appearance.density",
        Axis::ReducedMotion => "accessibility.reduced_motion",
    }
}

/// For tests: a `settings.get {view: values}` reply at `revision` holding
/// `record` (one app domain's record, as its module serialises it) as the
/// v2 keys under `prefix`: objects are walked, anything else is a leaf.
#[cfg(test)]
pub(crate) fn values_reply(revision: &str, prefix: &str, record: &Value) -> Value {
    fn walk(path: &str, value: &Value, out: &mut serde_json::Map<String, Value>) {
        match value.as_object() {
            Some(members) => {
                for (key, value) in members {
                    walk(&format!("{path}.{key}"), value, out);
                }
            }
            None => {
                out.insert(path.to_owned(), value.clone());
            }
        }
    }
    let mut values = serde_json::Map::new();
    walk(prefix, record, &mut values);
    json!({"status":"current",
        "binding":{"instance":"example","profile":"default"},
        "incarnation":"inc-1","revision":revision,
        "values":values,"resolved":{}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Effect;

    fn label(key: &str, args: &[(&str, &str)]) -> String {
        let mut out = key.to_owned();
        for (k, v) in args {
            out.push_str(&format!(" {k}={v}"));
        }
        out
    }

    fn reply(rc: u8, body: Value) -> Result<Reply, CallError> {
        Ok(Reply {
            rc,
            body: body.to_string(),
            error: None,
        })
    }

    fn lost() -> Result<Reply, CallError> {
        Err(CallError {
            message: "timed out".into(),
            outcome_unknown: true,
        })
    }

    /// A `settings.get {view: values}` reply, as settingsd builds it
    /// (contract 1.0.0), for an appearance given by its field names: the
    /// framing fields under `window.*`, the rest under `appearance.*`.
    fn snapshot_with(revision: &str, appearance: Value) -> Value {
        let mut values = serde_json::Map::new();
        for (field, value) in appearance.as_object().unwrap() {
            values.insert(change_path(field), value.clone());
        }
        json!({"status":"current",
            "binding":{"instance":"example","profile":"default"},
            "incarnation":"inc-1","revision":revision,
            "values":values,"resolved":{}})
    }

    fn snapshot_body(revision: &str, scheme: &str, mode: &str) -> Value {
        snapshot_with(
            revision,
            json!({"scheme":scheme,"mode":mode,"contrast":"normal","source":null}),
        )
    }

    #[test]
    fn automatic_request_and_effective_summary_are_distinct() {
        use design::decorations::{Resolver, SessionFacts, with_resolver};
        use std::sync::Arc;
        let resolver = Arc::new(Resolver::new(
            || SessionFacts {
                wayland: true,
                desktops: "KDE".into(),
                decoration_global: Some(true),
            },
            || None,
            |_| {},
        ));
        with_resolver(resolver, || {
            let mut e = read();
            let body = e.set_look(&json!({"decorations":"auto"})).unwrap();
            assert_eq!(body["shown"]["decorations"], "auto");
            assert_eq!(body["effective"]["decorations"], "ssd");
            assert_eq!(body["effective"]["decorations_reason"], "rule3");
            assert!(e.set_look(&json!({"decorations":"bogus"})).is_err());
        });
    }

    fn changed() -> Value {
        json!({"status":"changed","receipt":{}})
    }

    #[test]
    fn every_appearance_path_prefs_sends_has_a_settings_setter() {
        let base = Look::default();
        // No default tail: adding a Look field must extend this fixture.
        let changed = Look {
            scheme: Scheme::Forest,
            style: Some(Style::Plain),
            mode: Mode::Light,
            contrast: Contrast::High,
            decorations: Decorations::Server,
            captions: "left".into(),
            window_style: "win11".into(),
            menu_style: "button".into(),
            menu_side: "right".into(),
            title_align: "left".into(),
            actions_align: "left".into(),
            actions_place: "titlebar".into(),
            font_family: Some("Inter".into()),
            mono_family: Some("JetBrains Mono".into()),
            text_scale: 1.5,
            density: 1.5,
            reduced_motion: true,
        };
        let changes = changed.changes(&base);
        // The producer's serialised Look is the coverage authority. Adding an
        // axis requires a changed fixture and a settings setter, not a count.
        let serialised = serde_json::to_value(&changed).unwrap();
        let expected: std::collections::BTreeSet<_> = serialised
            .as_object()
            .unwrap()
            .keys()
            .map(|key| match key.as_str() {
                "text_scale" | "reduced_motion" => format!("accessibility.{key}"),
                _ => change_path(key),
            })
            .collect();
        assert_eq!(
            changes
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            expected,
            "every serialised appearance axis must be exercised"
        );
        // The v2 registry takes every change, and the result resolves.
        let values = settings::registry::apply(&Default::default(), &changes, &[]).unwrap();
        settings::resolve::values::resolve(&values, settings::EMBEDDED_DEFAULT_SOURCE).unwrap();
    }

    #[test]
    fn missing_settings_problem_clears_on_recovered_read() {
        let mut e = read();
        e.edit_terminal(crate::terminal::Setting::Padding(8));
        let ticket = settings_calls(&mut e)[0].0;
        e.settled(
            ticket,
            Ok(Reply {
                rc: 10,
                body: String::new(),
                error: Some("settingsd not found".into()),
            }),
        );
        assert!(e.look.problem.is_some());
        e.read_look();
        let ticket = settings_calls(&mut e)[0].0;
        e.settled(ticket, reply(0, snapshot_body("8", "studio", "dark")));
        assert!(e.look.problem.is_none());
    }

    #[test]
    fn refused_interface_apply_stays_visible_after_the_followup_read() {
        let mut e = read();
        e.edit_interface_font(crate::text::InterfaceEdit::Size(Some(16.0)));
        answer_next(
            &mut e,
            10,
            json!({"status":"validation_failed", "diagnostics":[{
                "path":"appearance.font_size", "code":"validation_failed", "message":"Unknown field or scope"
            }]}),
        );
        let problem = e.look.problem.clone().expect("the refusal is displayed");
        assert!(problem.contains("Unknown field or scope"));
        assert_eq!(e.look.problem_source, Some(crate::Section::TextAndFonts));
        answer_next(&mut e, 0, snapshot_body("7", "studio", "dark"));
        assert_eq!(e.look.problem, Some(problem));
        assert_eq!(e.look.interface_font.as_ref().unwrap().size, None);
        assert!(e.failed);
    }

    #[test]
    fn successful_write_clears_only_its_owners_problem() {
        let mut e = read();
        e.edit_terminal(crate::terminal::Setting::Term("BAD".into()));
        assert_eq!(e.look.problem_source, Some(crate::Section::Terminal));
        e.edit_interface_font(crate::text::InterfaceEdit::Size(Some(16.0)));
        let ticket = settings_calls(&mut e)[0].0;
        e.settled(ticket, reply(0, changed()));
        assert_eq!(e.look.problem_source, Some(crate::Section::Terminal));
        let ticket = settings_calls(&mut e)[0].0;
        e.settled(ticket, reply(0, snapshot_body("8", "studio", "dark")));
        e.edit_terminal(crate::terminal::Setting::Term("xterm".into()));
        let ticket = settings_calls(&mut e)[0].0;
        e.settled(ticket, reply(0, changed()));
        assert!(e.look.problem.is_none());
    }

    #[test]
    fn revert_after_cross_page_edit_clears_look_problem() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        answer_next(&mut e, 10, json!({"status":"validation_failed"}));
        e.edit_terminal(crate::terminal::Setting::Padding(8));
        answer_next(&mut e, 0, changed());
        answer_next(&mut e, 0, snapshot_body("8", "studio", "dark"));
        assert_eq!(e.look.status_source, crate::Section::Terminal);
        assert_eq!(e.look.problem_source, Some(crate::Section::Appearance));
        e.revert_look();
        assert!(e.look.problem.is_none());
    }

    #[test]
    fn retired_draft_clears_its_problem_but_preserves_terminal_problem() {
        for source in [crate::Section::Appearance, crate::Section::Terminal] {
            let mut e = read();
            edit(&mut e, json!({"scheme":"forest"}));
            answer_next(&mut e, 10, json!({"status":"validation_failed"}));
            if source == crate::Section::Terminal {
                e.edit_terminal(crate::terminal::Setting::Term("BAD".into()));
            } else {
                e.look.status_source = crate::Section::Terminal;
            }
            e.read_look();
            answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
            assert_eq!(e.look.problem.is_some(), source == crate::Section::Terminal);
        }
    }

    #[test]
    fn submitted_section_uses_edit_priority() {
        let mut submitted = Submitted {
            operation: "example".into(),
            look: Look::default(),
            axes: vec![
                Axis::Scheme,
                Axis::FontFamily,
                Axis::TextScale,
                Axis::Decorations,
            ],
        };
        assert_eq!(submitted.section(), crate::Section::Windows);
        submitted.axes.pop();
        assert_eq!(submitted.section(), crate::Section::Accessibility);
        submitted.axes.pop();
        assert_eq!(submitted.section(), crate::Section::TextAndFonts);
    }

    #[test]
    fn refusal_survives_read_and_can_be_dismissed_with_page_id() {
        let mut e = read();
        e.edit_terminal(crate::terminal::Setting::Padding(8));
        answer_next(&mut e, 10, json!({"status":"validation_failed"}));
        e.read_look();
        answer_next(&mut e, 0, snapshot_body("8", "studio", "dark"));
        assert_eq!(e.appearance_view()["problem_source"], "apps_term");
        e.dismiss_settings_problem();
        assert!(e.look.problem.is_none());
        assert!(e.look.settings_status.is_none());
        assert!(e.status.is_empty());
        assert!(!e.failed);
        assert!(e.appearance_view()["problem_source"].is_null());
    }

    #[test]
    fn dismiss_clears_a_refusal_copied_to_the_page_header() {
        let mut e = read();
        e.edit_terminal(crate::terminal::Setting::Padding(8));
        answer_next(&mut e, 10, json!({"status":"validation_failed"}));
        e.look.settings_status = e.look.problem.clone();
        assert!(e.look.settings_status.is_some());
        e.dismiss_settings_problem();
        assert!(e.look.settings_status.is_none());
        assert!(e.look.problem.is_none());
        assert!(e.status.is_empty());
        assert!(!e.failed);
    }

    #[test]
    fn dismiss_preserves_unrelated_page_feedback() {
        let mut e = read();
        e.edit_terminal(crate::terminal::Setting::Padding(8));
        answer_next(&mut e, 10, json!({"status":"validation_failed"}));
        e.look.settings_status = Some("Saved another setting".into());
        e.dismiss_settings_problem();
        assert_eq!(
            e.look.settings_status.as_deref(),
            Some("Saved another setting")
        );
    }

    fn settings_calls(e: &mut Engine) -> Vec<(u64, &'static str, Value)> {
        e.take_effects()
            .into_iter()
            .filter_map(|x| match x {
                Effect::Settings {
                    ticket,
                    command: command_name,
                    body,
                } => Some((ticket, command_name, body)),
                _ => None,
            })
            .collect()
    }

    /// An engine whose panel has read revision 7 (studio, the scheme's own
    /// style, dark), with nothing else in flight.
    fn read() -> Engine {
        let mut e = Engine::new(label);
        let listing = e
            .take_effects()
            .into_iter()
            .find_map(|x| match x {
                Effect::Releases { ticket, .. } => Some(ticket),
                _ => None,
            })
            .expect("the first listing");
        e.released(listing, reply(0, json!([])));
        e.take_effects();
        e.start_appearance("example");
        let calls = settings_calls(&mut e);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "settings.get");
        assert_eq!(calls[0].2["binding"]["instance"], "example");
        e.settled(calls[0].0, reply(0, snapshot_body("7", "studio", "dark")));
        e
    }

    fn edit(e: &mut Engine, args: Value) {
        let look = e.look.shown().unwrap().with(&args).unwrap();
        e.edit_look(look);
    }

    fn control_cases() -> [(AppearanceEdit, &'static str, Value, Value); 5] {
        [
            (
                AppearanceEdit::FontFamily(Some("Example Sans".into())),
                "appearance.font_family",
                json!("Example Sans"),
                Value::Null,
            ),
            (
                AppearanceEdit::MonoFamily(Some("Example Mono".into())),
                "appearance.mono_family",
                json!("Example Mono"),
                Value::Null,
            ),
            (
                AppearanceEdit::TextScale(1.25),
                "accessibility.text_scale",
                json!(1.25),
                json!(1.0),
            ),
            (
                AppearanceEdit::Density(0.8),
                "appearance.density",
                json!("compact"),
                json!(1.0),
            ),
            (
                AppearanceEdit::ReducedMotion(true),
                "accessibility.reduced_motion",
                json!(true),
                json!(false),
            ),
        ]
    }

    /// A values reply with the v2 key `path` stored as `value`. A density
    /// is given as the Prefs multiplier and stored as its step.
    fn stored_control(revision: &str, path: &str, value: Value) -> Value {
        let mut body = snapshot_body(revision, "studio", "dark");
        let value = match (path, value.as_f64()) {
            ("appearance.density", Some(density)) => json!(density_step(density)),
            _ => value,
        };
        body["values"][path] = value;
        body
    }

    #[test]
    fn appearance_controls_apply_fenced_then_reset_on_the_next_revision() {
        for (control, path, value, default) in control_cases() {
            let mut e = read();
            e.edit_appearance(control);
            let (ticket, command_name, body) = next(&mut e);
            assert_eq!(command_name, "settings.apply");
            assert_eq!(body["expected_incarnation"], "inc-1");
            assert_eq!(body["expected_revision"], "7");
            assert_eq!(body["changes"], json!({path:value.clone()}));
            e.settled(ticket, reply(0, changed()));
            answer_next(&mut e, 0, stored_control("8", path, value));
            let key = path.split_once('.').unwrap().1;
            edit(&mut e, json!({key:default.clone()}));
            let (_, command_name, body) = next(&mut e);
            assert_eq!(command_name, "settings.apply");
            assert_eq!(body["expected_revision"], "8");
            // The density edit is numeric; its reset is the v2 step.
            let reset = if path == "appearance.density" {
                json!("comfortable")
            } else {
                default
            };
            assert_eq!(body["changes"], json!({path:reset}));
        }
    }

    #[test]
    fn appearance_controls_keep_conflicting_drafts_until_the_person_decides() {
        for (control, path, value, _) in control_cases() {
            let mut e = read();
            e.edit_appearance(control);
            answer_next(&mut e, 10, json!({"status":"conflict"}));
            let other = match path {
                "appearance.font_family" | "appearance.mono_family" => json!("Other Family"),
                "accessibility.text_scale" | "appearance.density" => json!(1.5),
                // A boolean has no third value: an agreeing writer retires it.
                _ => json!(true),
            };
            answer_next(&mut e, 0, stored_control("8", path, other));
            if path == "accessibility.reduced_motion" {
                assert!(e.look.draft.is_none());
                assert!(e.look.conflicts.is_empty());
            } else {
                assert_eq!(e.look.conflicts.len(), 1, "{path}");
                assert!(e.look.waiting());
                assert!(settings_calls(&mut e).is_empty());
                e.keep_look();
                let (_, command_name, body) = next(&mut e);
                assert_eq!(command_name, "settings.apply");
                assert_eq!(body["expected_revision"], "8");
                assert_eq!(body["changes"], json!({path:value}));
            }
        }
    }

    #[test]
    fn a_motion_edit_retries_on_a_fresh_fence_after_an_unrelated_conflict() {
        let mut e = read();
        e.edit_appearance(AppearanceEdit::ReducedMotion(true));
        answer_next(&mut e, 10, json!({"status":"conflict"}));
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        let (_, _, body) = next(&mut e);
        assert_eq!(body["expected_revision"], "8");
        assert_eq!(
            body["changes"],
            json!({"accessibility.reduced_motion":true})
        );
    }

    #[test]
    fn new_controls_queue_reversals_while_an_apply_settles() {
        for (control, path, value, default) in control_cases() {
            let mut e = read();
            e.edit_appearance(control);
            let (ticket, _, _) = next(&mut e);
            let key = path.split_once('.').unwrap().1;
            edit(&mut e, json!({key:default.clone()}));
            assert!(settings_calls(&mut e).is_empty());
            e.settled(ticket, reply(0, changed()));
            answer_next(&mut e, 0, stored_control("8", path, value));
            let (_, _, body) = next(&mut e);
            assert_eq!(body["expected_revision"], "8");
            // The density edit is numeric; its reversal is the v2 step.
            let reversal = if path == "appearance.density" {
                json!("comfortable")
            } else {
                default
            };
            assert_eq!(body["changes"], json!({path:reversal}));
            assert!(e.look.conflicts.is_empty());
        }
    }

    #[test]
    fn new_control_arguments_are_checked_before_any_axis_is_edited() {
        let mut e = read();
        for invalid in [
            json!({"font_family":""}),
            json!({"mono_family":" padded "}),
            json!({"font_family":"bad\nname"}),
            json!({"font_family":"a".repeat(129)}),
            json!({"text_scale":3.05}),
            json!({"density":0.45}),
            json!({"reduced_motion":1}),
        ] {
            let mut args = invalid;
            args["scheme"] = json!("forest");
            assert!(e.set_look(&args).is_err());
            assert!(settings_calls(&mut e).is_empty());
            assert_eq!(e.look.shown().unwrap().scheme, Scheme::Studio);
        }
    }

    /// The one settings call now queued.
    fn next(e: &mut Engine) -> (u64, &'static str, Value) {
        let calls = settings_calls(e);
        assert_eq!(calls.len(), 1, "{calls:?}");
        calls[0].clone()
    }

    /// Answer the next settings call with `body`.
    fn answer_next(e: &mut Engine, rc: u8, body: Value) -> (&'static str, Value) {
        let (ticket, command_name, sent) = next(e);
        e.settled(ticket, reply(rc, body));
        (command_name, sent)
    }

    #[test]
    fn the_first_read_fills_the_panel() {
        let e = read();
        assert_eq!(e.look.settings, Availability::Available);
        assert_eq!(e.look.current.as_ref().unwrap().scheme, Scheme::Studio);
        assert_eq!(e.look.revision(), Some(7));
        assert!(e.look.draft.is_none() && !e.can_apply_look());
    }

    #[test]
    fn density_and_window_paths_are_the_v2_names() {
        assert_eq!(density_step(0.8), "compact");
        assert_eq!(density_step(0.925), "comfortable");
        assert_eq!(density_step(1.0), "comfortable");
        assert_eq!(density_step(1.1), "comfortable");
        assert_eq!(density_step(1.2), "spacious");
        assert_eq!(change_path("caption_side"), "window.caption_side");
        assert_eq!(change_path("window_style"), "window.style");
        assert_eq!(change_path("style"), "appearance.style");
        assert_eq!(change_path("font_family"), "appearance.font_family");
    }

    #[test]
    fn window_style_uses_the_immediate_apply_path_and_keeps_future_names() {
        for name in design::WindowStyle::NAMES
            .iter()
            .copied()
            .chain(["future-style"])
        {
            let mut e = read();
            assert_eq!(e.look.current.as_ref().unwrap().window_style, "mac");
            if name == "mac" {
                continue;
            }
            e.set_look(&json!({"window_style":name})).unwrap();
            let (_, command, request) = next(&mut e);
            assert_eq!(command, "settings.apply");
            assert_eq!(request["changes"], json!({"window.style":name}));
            assert_eq!(request["expected_revision"], "7");
            assert_eq!(e.look.shown().unwrap().window_style, name);
        }
        let stored = Stored {
            window_style: "future-style".into(),
            ..Stored::default()
        };
        let look = Look::from_stored(&stored).unwrap();
        assert_eq!(
            look.with(&json!({"mode":"light"})).unwrap().window_style,
            "future-style"
        );
        assert_eq!(look.effective()["window_style"], "mac");
        assert_eq!(
            serde_json::to_value(look).unwrap()["window_style"],
            "future-style"
        );
    }

    #[test]
    fn menu_axes_apply_immediately_and_keep_unknown_names() {
        for (key, names, default) in [
            ("menu_style", design::MenuStyle::NAMES, "bar"),
            ("menu_side", design::MenuSide::NAMES, "left"),
            ("title_align", design::TitleAlign::NAMES, "auto"),
            ("actions_align", design::ActionsAlign::NAMES, "right"),
            ("actions_place", design::ActionsPlace::NAMES, "row"),
            ("caption_side", design::CaptionSide::NAMES, "auto"),
        ] {
            for name in names
                .iter()
                .copied()
                .chain(["future-menu"])
                .filter(|name| *name != default)
            {
                let mut e = read();
                let mut args = json!({});
                args[key] = json!(name);
                e.set_look(&args).unwrap();
                let (_, command, request) = next(&mut e);
                assert_eq!(command, "settings.apply");
                assert_eq!(request["changes"][change_path(key)], name);
                assert_eq!(
                    serde_json::to_value(e.look.shown().unwrap()).unwrap()[key],
                    name
                );
            }
        }
        let look = Look::from_stored(&Stored {
            menu_style: "future-style".into(),
            menu_side: "future-side".into(),
            ..Stored::default()
        })
        .unwrap();
        assert_eq!(look.effective()["menu_style"], "bar");
        assert_eq!(look.effective()["menu_side"], "left");
        let changed = look.with(&json!({"mode":"light"})).unwrap();
        assert_eq!(changed.menu_style, "future-style");
        assert_eq!(changed.menu_side, "future-side");
        assert!(
            look.with(&json!({"menu_style":"bad name", "mode":"light"}))
                .is_err()
        );
    }

    #[test]
    fn a_wrong_target_adopts_the_instance_once_never_the_profile() {
        let mut e = Engine::new(label);
        e.take_effects();
        e.start_appearance("guess");
        answer_next(
            &mut e,
            10,
            json!({"status":"wrong_target","binding":{"instance":"real","profile":"default"}}),
        );
        let (ticket, _, body) = next(&mut e);
        assert_eq!(body["binding"]["instance"], "real");
        e.settled(
            ticket,
            reply(
                10,
                json!({"status":"wrong_target","binding":{"instance":"other","profile":"default"}}),
            ),
        );
        assert!(
            settings_calls(&mut e).is_empty(),
            "a second retarget is refused"
        );
        assert!(e.failed);

        let mut e = Engine::new(label);
        e.take_effects();
        e.start_appearance("guess");
        answer_next(
            &mut e,
            10,
            json!({"status":"wrong_target","binding":{"instance":"real","profile":"other"}}),
        );
        assert!(
            settings_calls(&mut e).is_empty(),
            "another profile is never adopted"
        );
        assert_eq!(e.look.binding.profile, PROFILE);
    }

    #[test]
    fn an_edit_applies_at_once_fenced_with_only_its_axes() {
        let mut e = read();
        edit(
            &mut e,
            json!({"scheme":"forest","style":"pro","decorations":"ssd"}),
        );
        let (ticket, command_name, body) = next(&mut e);
        assert_eq!(command_name, "settings.apply");
        assert_eq!(body["expected_incarnation"], "inc-1");
        assert_eq!(body["expected_revision"], "7");
        assert_eq!(
            body["changes"],
            json!({"appearance.scheme":"forest","appearance.style":"pro","window.decorations":"ssd"})
        );
        assert!(
            body["operation_id"]
                .as_str()
                .unwrap()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        );
        // The panel already shows it; nothing is waiting for the person.
        assert!(e.look.draft.is_none() && !e.look.waiting());
        assert_eq!(e.look.shown().unwrap().scheme, Scheme::Forest);
        e.settled(ticket, reply(0, changed()));
        assert_eq!(
            e.look.shown().unwrap().scheme,
            Scheme::Forest,
            "no flicker back"
        );
        let mut stored = snapshot_body("8", "forest", "dark");
        stored["values"]["appearance.style"] = json!("pro");
        stored["values"]["window.decorations"] = json!("ssd");
        answer_next(&mut e, 0, stored);
        assert!(e.look.draft.is_none() && e.look.sent.is_none());
        assert_eq!(e.look.revision(), Some(8));
        assert!(settings_calls(&mut e).is_empty());
    }

    #[test]
    fn edits_while_an_apply_settles_queue_and_go_out_after_it() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        // Two more edits while it is in flight: the second wins.
        edit(&mut e, json!({"scheme":"ocean"}));
        edit(&mut e, json!({"mode":"light"}));
        assert!(settings_calls(&mut e).is_empty(), "one apply at a time");
        assert_eq!(e.look.shown().unwrap().scheme, Scheme::Ocean);
        e.settled(apply, reply(0, changed()));
        // The read-back first, then the queued edits, on its fence.
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        let (_, command_name, body) = next(&mut e);
        assert_eq!(command_name, "settings.apply");
        assert_eq!(body["expected_revision"], "8");
        assert_eq!(
            body["changes"],
            json!({"appearance.scheme":"ocean","appearance.mode":"light"})
        );
        assert!(e.look.conflicts.is_empty(), "its own write is no conflict");
    }

    #[test]
    fn going_back_to_the_old_value_while_applying_is_an_edit() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        edit(&mut e, json!({"scheme":"studio"}));
        e.settled(apply, reply(0, changed()));
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        let (_, _, body) = next(&mut e);
        assert_eq!(body["changes"], json!({"appearance.scheme":"studio"}));
    }

    #[test]
    fn the_scheme_s_own_style_is_sent_as_null() {
        let look = Look {
            style: Some(Style::Classic),
            ..Look::default()
        };
        let own = look.with(&json!({"style":"own"})).unwrap();
        assert_eq!(own.changes(&look)["appearance.style"], Value::Null);
    }

    #[test]
    fn a_refusal_holds_the_edit_instead_of_retrying() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        answer_next(
            &mut e,
            10,
            json!({"status":"validation_failed","message":"no"}),
        );
        assert!(e.look.hold && e.look.waiting() && e.failed);
        assert_eq!(e.look.problem.as_deref(), Some(e.status.as_str()));
        // Software status updates must not replace a settings failure banner.
        e.status = "listed".into();
        e.failed = false;
        assert!(e.look.problem.is_some());
        assert_eq!(e.look.draft.as_ref().unwrap().look.scheme, Scheme::Forest);
        assert!(settings_calls(&mut e).is_empty(), "not retried in a loop");
        assert!(e.can_apply_look() && e.can_revert_look());
        // Apply tries again; an edit would too; Revert drops it.
        e.apply_look();
        assert!(e.look.problem.is_some(), "a retry has not succeeded yet");
        assert_eq!(next(&mut e).1, "settings.apply");
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        answer_next(&mut e, 10, json!({"status":"validation_failed"}));
        e.revert_look();
        assert!(e.look.draft.is_none() && !e.look.hold);
        assert_eq!(e.look.shown().unwrap().scheme, Scheme::Studio);
    }

    #[test]
    fn another_writer_on_an_untouched_axis_is_followed_not_undone() {
        let mut e = read();
        // A refusal holds the mode edit; meanwhile the scheme changes elsewhere.
        edit(&mut e, json!({"mode":"light"}));
        answer_next(&mut e, 10, json!({"status":"validation_failed"}));
        e.read_look();
        answer_next(&mut e, 0, snapshot_body("8", "ocean", "dark"));
        let draft = e.look.draft.as_ref().unwrap();
        assert_eq!(
            draft.look.scheme,
            Scheme::Ocean,
            "the untouched axis follows"
        );
        assert_eq!(draft.look.mode, Mode::Light, "the edit stays");
        e.apply_look();
        let (_, _, body) = next(&mut e);
        assert_eq!(body["changes"], json!({"appearance.mode":"light"}));
        assert_eq!(body["expected_revision"], "8");
    }

    #[test]
    fn a_conflict_refusal_rereads_and_reapplies_when_nothing_clashed() {
        let mut e = read();
        edit(&mut e, json!({"mode":"light"}));
        answer_next(
            &mut e,
            10,
            json!({"status":"conflict","incarnation":"inc-1","revision":"9"}),
        );
        // Another writer changed the scheme only: re-applied on the new fence.
        answer_next(&mut e, 0, snapshot_body("9", "ocean", "dark"));
        let (_, command_name, body) = next(&mut e);
        assert_eq!(command_name, "settings.apply");
        assert_eq!(body["expected_revision"], "9");
        assert_eq!(body["changes"], json!({"appearance.mode":"light"}));
    }

    #[test]
    fn a_failed_read_after_a_conflict_never_reapplies_on_the_refused_fence() {
        let mut e = read();
        edit(&mut e, json!({"mode":"light"}));
        answer_next(
            &mut e,
            10,
            json!({"status":"conflict","incarnation":"inc-1","revision":"9"}),
        );
        let (read_after, command_name, _) = next(&mut e);
        assert_eq!(command_name, "settings.get");
        e.settled(read_after, lost());
        assert!(
            settings_calls(&mut e).is_empty(),
            "no apply on the refused fence"
        );
        assert!(!e.can_apply_look());
        assert_eq!(
            e.look.draft.as_ref().unwrap().look.mode,
            Mode::Light,
            "the edit is kept"
        );
        e.ui.section = crate::Section::Appearance;
        e.refresh();
        answer_next(&mut e, 0, snapshot_body("9", "studio", "dark"));
        let (_, command_name, body) = next(&mut e);
        assert_eq!(
            (command_name, &body["expected_revision"]),
            ("settings.apply", &json!("9"))
        );
    }

    #[test]
    fn edits_queued_under_a_dialog_go_out_when_it_closes() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        edit(&mut e, json!({"mode":"light"}));
        e.open(crate::Dialog::Shortcuts);
        e.settled(apply, reply(0, changed()));
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        // Accepted before the dialog: it drains regardless.
        let (_, command_name, body) = next(&mut e);
        assert_eq!(
            (command_name, &body["changes"]),
            ("settings.apply", &json!({"appearance.mode":"light"}))
        );
        // And if it had waited, closing the dialog sends it.
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        answer_next(
            &mut e,
            10,
            json!({"status":"conflict","incarnation":"inc-1","revision":"8"}),
        );
        e.open(crate::Dialog::About);
        let (read_after, _, _) = next(&mut e);
        e.settled(read_after, lost());
        e.ui.section = crate::Section::Appearance;
        e.close_dialog();
        assert!(settings_calls(&mut e).is_empty(), "still no fresh fence");
    }

    #[test]
    fn quitting_drains_accepted_edits_before_closing() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        edit(&mut e, json!({"mode":"light"}));
        e.quit();
        assert!(!e.take_effects().contains(&Effect::Exit));
        e.settled(apply, reply(0, changed()));
        let (read_back, command_name, _) = next(&mut e);
        assert_eq!(
            command_name, "settings.get",
            "the read-back the queued edit needs"
        );
        e.settled(read_back, reply(0, snapshot_body("8", "forest", "dark")));
        let calls: Vec<_> = e
            .take_effects()
            .into_iter()
            .filter_map(|x| match x {
                Effect::Settings {
                    ticket,
                    command: command_name,
                    body,
                } => Some((ticket, command_name, body)),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "settings.apply");
        assert_eq!(calls[0].2["changes"], json!({"appearance.mode":"light"}));
        e.settled(calls[0].0, reply(0, changed()));
        assert!(
            e.take_effects().contains(&Effect::Exit),
            "nothing left: close"
        );
    }

    #[test]
    fn quitting_through_an_unknown_operation_still_drains_the_queued_edit() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        edit(&mut e, json!({"mode":"light"}));
        e.quit();
        e.settled(apply, lost());
        answer_next(
            &mut e,
            0,
            json!({"status":"unknown_operation","receipt":null,"recovering":false}),
        );
        // The read that decides runs although quitting.
        let (command_name, _) = answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        assert_eq!(command_name, "settings.get");
        let (apply2, command_name, body) = next(&mut e);
        assert_eq!(
            (command_name, &body["changes"]),
            ("settings.apply", &json!({"appearance.mode":"light"}))
        );
        e.settled(apply2, reply(0, changed()));
        assert!(e.take_effects().contains(&Effect::Exit));
    }

    #[test]
    fn a_failed_drain_read_keeps_prefs_open_and_a_second_close_discards() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        edit(&mut e, json!({"mode":"light"}));
        e.quit();
        e.settled(apply, reply(0, changed()));
        let (read_back, _, _) = next(&mut e);
        e.settled(read_back, lost());
        assert!(
            !e.take_effects().contains(&Effect::Exit),
            "the edit is not dropped silently"
        );
        assert!(!e.quitting && e.look.hold && e.failed);
        assert_eq!(e.status, "look-kept-open");
        assert_eq!(e.look.draft.as_ref().unwrap().look.mode, Mode::Light);
        e.quit();
        assert!(
            e.take_effects().contains(&Effect::Exit),
            "closing again discards it"
        );
    }

    #[test]
    fn a_clash_on_an_edited_axis_waits_for_keep_or_revert() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        answer_next(
            &mut e,
            10,
            json!({"status":"conflict","incarnation":"inc-1","revision":"9"}),
        );
        answer_next(&mut e, 0, snapshot_body("9", "ocean", "dark"));
        assert_eq!(e.look.conflicts, vec![Axis::Scheme]);
        assert!(settings_calls(&mut e).is_empty() && !e.can_apply_look());
        assert!(e.look.waiting() && e.can_keep_look() && e.can_revert_look());
        e.keep_look();
        let (_, _, body) = next(&mut e);
        assert_eq!(body["changes"], json!({"appearance.scheme":"forest"}));
    }

    #[test]
    fn another_writer_agreeing_with_a_held_edit_retires_it() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        answer_next(&mut e, 10, json!({"status":"validation_failed"}));
        e.read_look();
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        assert!(e.look.draft.is_none() && e.look.conflicts.is_empty());
        assert!(!e.look.hold && !e.failed && e.status == "settled-look");
        // A no-op edit that empties a held draft settles it too.
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        answer_next(&mut e, 10, json!({"status":"validation_failed"}));
        edit(&mut e, json!({"scheme":"studio"}));
        assert!(e.look.draft.is_none() && !e.look.hold && !e.failed);
        assert_eq!(e.status, "settled-look");
    }

    #[test]
    fn a_lost_apply_is_settled_by_its_receipt_before_anything_else() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, body) = next(&mut e);
        let operation = body["operation_id"].as_str().unwrap().to_owned();
        e.settled(apply, lost());
        assert!(e.look.uncertain.is_some() && !e.can_apply_look());
        // Edits queue meanwhile and go out only after the receipt and read.
        edit(&mut e, json!({"mode":"light"}));
        let (command_name, sent) = answer_next(
            &mut e,
            0,
            json!({"status":"current","receipt":{"operation_id":operation},"recovering":false}),
        );
        assert_eq!(
            (command_name, sent["operation_id"].as_str()),
            ("settings.status", Some(operation.as_str()))
        );
        assert!(e.look.uncertain.is_none());
        assert_eq!(e.status, "applied-look");
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        let (_, command_name, body) = next(&mut e);
        assert_eq!(command_name, "settings.apply");
        assert_eq!(body["changes"], json!({"appearance.mode":"light"}));
    }

    #[test]
    fn no_receipt_is_no_verdict_the_read_decides() {
        for (shows, status) in [(true, "applied-look"), (false, "look-unconfirmed")] {
            let mut e = read();
            edit(&mut e, json!({"scheme":"forest"}));
            answer_next(
                &mut e,
                10,
                json!({"status":"outcome_unknown","message":"recovering"}),
            );
            answer_next(
                &mut e,
                0,
                json!({"status":"unknown_operation","receipt":null,"recovering":false}),
            );
            assert!(e.look.uncertain.is_none());
            let scheme = if shows { "forest" } else { "studio" };
            let (command_name, _) = answer_next(&mut e, 0, snapshot_body("8", scheme, "dark"));
            assert_eq!(command_name, "settings.get");
            assert_eq!(e.status, status);
            assert_eq!(
                e.look.hold, !shows,
                "not shown: the edit returns, held for the person"
            );
            assert_eq!(e.look.draft.is_some(), !shows);
            assert!(settings_calls(&mut e).is_empty());
        }
    }

    #[test]
    fn a_recovering_settingsd_keeps_the_apply_uncertain_and_check_again_asks_again() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        e.settled(apply, lost());
        answer_next(
            &mut e,
            0,
            json!({"status":"unknown_operation","receipt":null,"recovering":true}),
        );
        assert!(e.look.uncertain.is_some() && !e.can_apply_look());
        assert!(settings_calls(&mut e).is_empty());
        assert!(e.can_recheck_look());
        e.recheck_look();
        assert_eq!(next(&mut e).1, "settings.status");
    }

    #[test]
    fn a_failed_receipt_lookup_can_be_checked_again() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        e.settled(apply, lost());
        let (receipt, _, _) = next(&mut e);
        e.settled(receipt, lost());
        assert!(e.look.uncertain.is_some());
        e.recheck_look();
        assert_eq!(next(&mut e).1, "settings.status");
    }

    #[test]
    fn a_failed_read_back_waits_and_refresh_reads_again() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        e.settled(apply, reply(0, changed()));
        edit(&mut e, json!({"mode":"light"}));
        let (read_back, _, _) = next(&mut e);
        e.settled(read_back, lost());
        assert!(
            settings_calls(&mut e).is_empty(),
            "no apply on a stale fence"
        );
        e.ui.section = crate::Section::Appearance;
        assert!(e.can_refresh());
        e.refresh();
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        let (_, command_name, body) = next(&mut e);
        assert_eq!(
            (command_name, &body["changes"]),
            ("settings.apply", &json!({"appearance.mode":"light"}))
        );
    }

    #[test]
    fn quitting_settles_a_lost_apply_by_receipt_a_bounded_number_of_times() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        e.quit();
        assert!(
            !e.take_effects().contains(&Effect::Exit),
            "waits for the apply"
        );
        e.settled(apply, lost());
        for _ in 0..QUIT_RECEIPT_TRIES {
            let calls: Vec<_> = e
                .take_effects()
                .into_iter()
                .filter_map(|x| match x {
                    Effect::Settings {
                        ticket,
                        command: command_name,
                        ..
                    } => Some((ticket, command_name)),
                    _ => None,
                })
                .collect();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].1, "settings.status");
            e.settled(calls[0].0, lost());
        }
        // Still unknown: the edit is kept and Preferences stays open once.
        assert!(!e.take_effects().contains(&Effect::Exit));
        assert_eq!(e.status, "look-kept-open");
        assert_eq!(e.look.draft.as_ref().unwrap().look.scheme, Scheme::Forest);
        e.quit();
        assert!(e.take_effects().contains(&Effect::Exit));
    }

    #[test]
    fn quitting_with_a_lone_unconfirmed_edit_reads_before_deciding() {
        for (shows, exits) in [(true, true), (false, false)] {
            let mut e = read();
            edit(&mut e, json!({"scheme":"forest"}));
            let (apply, _, _) = next(&mut e);
            e.quit();
            e.settled(apply, lost());
            answer_next(
                &mut e,
                0,
                json!({"status":"unknown_operation","receipt":null,"recovering":false}),
            );
            let scheme = if shows { "forest" } else { "studio" };
            let (ticket, command_name, _) = next(&mut e);
            assert_eq!(
                command_name, "settings.get",
                "the deciding read runs while quitting"
            );
            e.settled(ticket, reply(0, snapshot_body("8", scheme, "dark")));
            assert_eq!(e.take_effects().contains(&Effect::Exit), exits);
            if !exits {
                assert_eq!(e.look.draft.as_ref().unwrap().look.scheme, Scheme::Forest);
            }
        }
        // And a failed deciding read keeps the edit and Preferences open.
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        e.quit();
        e.settled(apply, lost());
        answer_next(
            &mut e,
            0,
            json!({"status":"unknown_operation","receipt":null,"recovering":false}),
        );
        let (ticket, _, _) = next(&mut e);
        e.settled(ticket, lost());
        assert!(!e.take_effects().contains(&Effect::Exit));
        assert_eq!(e.look.draft.as_ref().unwrap().look.scheme, Scheme::Forest);
    }

    /// Studio, Forest (its reply lost), then Studio again, queued; the
    /// close waits out the receipts: Preferences stays open with the reversal.
    fn reversal_kept_open_after_exhausted_receipts() -> Engine {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        edit(&mut e, json!({"scheme":"studio"}));
        e.quit();
        e.settled(apply, lost());
        for _ in 0..QUIT_RECEIPT_TRIES {
            let (ticket, command_name, _) = next(&mut e);
            assert_eq!(command_name, "settings.status");
            e.settled(ticket, lost());
        }
        let effects = e.take_effects();
        assert!(!effects.contains(&Effect::Exit));
        assert_eq!(e.status, "look-kept-open");
        assert_eq!(e.look.shown().unwrap().scheme, Scheme::Studio);
        assert!(e.look.draft.is_some(), "the reversal is not dropped");
        // The read that decides goes out at once; nothing is applied first.
        let reads: Vec<_> = effects
            .into_iter()
            .filter_map(|x| match x {
                Effect::Settings {
                    ticket,
                    command: command_name,
                    ..
                } => Some((ticket, command_name)),
                _ => None,
            })
            .collect();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].1, "settings.get");
        assert!(!e.can_apply_look(), "no apply before the read decides");
        assert_eq!(e.look.job.as_ref().map(|j| j.0), Some(reads[0].0));
        e
    }

    #[test]
    fn a_reversal_behind_exhausted_receipts_survives_when_the_apply_landed() {
        let mut e = reversal_kept_open_after_exhausted_receipts();
        let (ticket, _) = e.look.job.clone().unwrap();
        // Forest did land: the reversal is an edit of the person's own,
        // held for Try again, not a conflict.
        e.settled(ticket, reply(0, snapshot_body("8", "forest", "dark")));
        assert!(settings_calls(&mut e).is_empty(), "held, not retried");
        let draft = e.look.draft.as_ref().unwrap();
        assert_eq!(
            (draft.base.scheme, draft.look.scheme),
            (Scheme::Forest, Scheme::Studio)
        );
        assert!(e.look.conflicts.is_empty() && e.look.hold);
        e.apply_look();
        let (_, command_name, body) = next(&mut e);
        assert_eq!(command_name, "settings.apply");
        assert_eq!(body["expected_revision"], "8");
        assert_eq!(body["changes"], json!({"appearance.scheme":"studio"}));
    }

    #[test]
    fn a_reversal_behind_exhausted_receipts_retires_when_the_apply_did_not_land() {
        let mut e = reversal_kept_open_after_exhausted_receipts();
        let (ticket, _) = e.look.job.clone().unwrap();
        e.settled(ticket, reply(0, snapshot_body("7", "studio", "dark")));
        assert!(e.look.draft.is_none() && settings_calls(&mut e).is_empty());
        // Nothing is held any more: no "not applied" text over nothing.
        assert!(!e.look.hold && !e.failed);
        assert_eq!(e.status, "settled-look");
        e.quit();
        assert!(e.take_effects().contains(&Effect::Exit));
    }

    #[test]
    fn a_recovering_read_does_not_decide_a_pinned_reversal() {
        for (landed, kept) in [("forest", true), ("studio", false)] {
            let mut e = reversal_kept_open_after_exhausted_receipts();
            let (ticket, _) = e.look.job.clone().unwrap();
            // settingsd still shows Studio from memory while it recovers a
            // Forest write it may already have taken.
            let mut recovering = snapshot_body("7", "studio", "dark");
            recovering["recovering"] = json!(true);
            e.settled(ticket, reply(0, recovering));
            assert!(
                e.look.draft.is_some(),
                "a recovering read keeps the reversal"
            );
            assert_eq!(e.look.shown().unwrap().scheme, Scheme::Studio);
            assert!(settings_calls(&mut e).is_empty() && !e.can_apply_look());
            // Its change hint reads again: recovered, that read decides.
            e.read_look();
            answer_next(&mut e, 0, snapshot_body("8", landed, "dark"));
            assert_eq!(e.look.draft.is_some(), kept, "{landed}");
            if kept {
                let draft = e.look.draft.as_ref().unwrap();
                assert_eq!(
                    (draft.base.scheme, draft.look.scheme),
                    (Scheme::Forest, Scheme::Studio)
                );
                assert!(e.can_apply_look() && e.look.conflicts.is_empty());
            }
        }
    }

    #[test]
    fn a_second_close_does_not_wait_for_the_deciding_read() {
        let mut e = reversal_kept_open_after_exhausted_receipts();
        let (ticket, _) = e.look.job.clone().unwrap();
        e.quit();
        assert!(
            e.take_effects().contains(&Effect::Exit),
            "closing again discards at once"
        );
        // The abandoned read's late reply changes nothing.
        e.settled(ticket, reply(0, snapshot_body("8", "forest", "dark")));
        assert!(e.look.draft.is_none());
    }

    #[test]
    fn a_reversal_behind_a_failed_deciding_read_survives() {
        let mut e = read();
        edit(&mut e, json!({"scheme":"forest"}));
        let (apply, _, _) = next(&mut e);
        edit(&mut e, json!({"scheme":"studio"}));
        e.quit();
        e.settled(apply, lost());
        answer_next(
            &mut e,
            0,
            json!({"status":"unknown_operation","receipt":null,"recovering":false}),
        );
        // The deciding read fails: Preferences stays open with the reversal, and
        // reads again rather than trusting the stale look.
        let (ticket, command_name, _) = next(&mut e);
        assert_eq!(command_name, "settings.get");
        e.settled(ticket, lost());
        let (again, command_name, _) = next(&mut e);
        assert_eq!(command_name, "settings.get");
        assert_eq!(e.status, "look-kept-open");
        assert_eq!(e.look.shown().unwrap().scheme, Scheme::Studio);
        // That fails too: still kept, nothing applied, Refresh reads again.
        e.settled(again, lost());
        assert!(settings_calls(&mut e).is_empty() && !e.can_apply_look());
        assert_eq!(e.look.shown().unwrap().scheme, Scheme::Studio);
        e.ui.section = crate::Section::Appearance;
        e.refresh();
        answer_next(&mut e, 0, snapshot_body("8", "forest", "dark"));
        let draft = e.look.draft.as_ref().unwrap();
        assert_eq!(
            (draft.base.scheme, draft.look.scheme),
            (Scheme::Forest, Scheme::Studio)
        );
        assert!(e.look.conflicts.is_empty());
    }

    #[test]
    fn a_hint_while_busy_rereads_once_afterwards() {
        let mut e = read();
        e.read_look();
        let (first, _, _) = next(&mut e);
        e.read_look();
        e.read_look();
        assert!(settings_calls(&mut e).is_empty());
        e.settled(first, reply(0, snapshot_body("7", "studio", "dark")));
        assert_eq!(settings_calls(&mut e).len(), 1);
    }

    #[test]
    fn a_late_reply_is_ignored() {
        let mut e = read();
        e.read_look();
        let (ticket, _, _) = next(&mut e);
        e.settled(ticket + 100, reply(0, snapshot_body("99", "ocean", "dark")));
        assert_eq!(e.look.revision(), Some(7));
    }

    #[test]
    fn missing_settingsd_reads_as_missing() {
        let mut e = Engine::new(label);
        e.take_effects();
        e.start_appearance("example");
        let (ticket, _, _) = next(&mut e);
        e.settled(
            ticket,
            Ok(Reply {
                rc: 10,
                body: String::new(),
                error: Some("Service 'settingsd' not found".into()),
            }),
        );
        assert_eq!(e.look.settings, Availability::Missing);
    }

    #[test]
    fn set_checks_every_axis_first_then_applies() {
        let mut e = read();
        assert!(
            e.set_look(&json!({"scheme":"forest","mode":"dim"}))
                .is_err()
        );
        assert!(settings_calls(&mut e).is_empty());
        let view = e
            .set_look(&json!({"scheme":"forest","caption_side":"left"}))
            .unwrap();
        assert_eq!(view["shown"]["scheme"], "forest");
        assert_eq!(view["sent"], json!(["scheme", "caption_side"]));
        assert_eq!(next(&mut e).1, "settings.apply");
    }
}
