// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Lucide icons: unmodified upstream SVGs (Lucide 1.54.0, ISC; NOTICE),
//! rendered through egui's SVG loader.
//!
//! **Stroke weight is a token.** Lucide draws every icon on a 24 px grid with
//! `stroke-width="2"`. [`stroke_width`] reads the design metric
//! `icon.stroke_width` and falls back to [`DEFAULT_STROKE`]; each icon's
//! stroke is rewritten to it when the image is built, so a lighter icon set
//! (for a light type profile) is a design change, not an asset change.
//!
//! **Colour comes from the theme.** Upstream SVGs stroke in `currentColor`.
//! The loader has no current colour, so the stroke is rendered as a white
//! mask (`style.icon_mask`) and tinted at draw time with the widget's text colour.
//! The mask is a rendering device, not a UI colour: every visible colour is
//! the tint.

use crate::theme::Theme;
use design::ResolvedMetricKind;
use egui::{Color32, Image, ImageSource, Vec2};
use std::borrow::Cow;

/// The stroke used when the design authors none: the shipped baseline, which
/// matches the `icon.stroke_width` token (1.5 on the 24-unit grid).
pub const DEFAULT_STROKE: f32 = 1.5;

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        /// The icons the toolkit ships. Add an icon by copying its SVG from
        /// the pinned Lucide release into `assets/icons/` and listing it here.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon { $($variant),+ }

        impl Icon {
            /// Every shipped icon.
            pub const ALL: &[Icon] = &[$(Icon::$variant),+];

            /// The Lucide name.
            pub fn name(self) -> &'static str {
                match self { $(Icon::$variant => $file),+ }
            }

            fn svg(self) -> &'static str {
                match self {
                    $(Icon::$variant => include_str!(concat!("../assets/icons/", $file, ".svg"))),+
                }
            }
        }
    };
}

icons! {
    Accessibility => "accessibility",
    AppWindow => "app-window",
    ArrowDown => "arrow-down",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    ArrowUp => "arrow-up",
    BookUser => "book-user",
    Calendar => "calendar",
    Camera => "camera",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ChevronsLeft => "chevrons-left",
    ChevronsRight => "chevrons-right",
    Circle => "circle",
    CircleAlert => "circle-alert",
    CircleCheck => "circle-check",
    Contact => "contact",
    Copy => "copy",
    Crop => "crop",
    Disc => "disc",
    Download => "download",
    Ellipsis => "ellipsis",
    ExternalLink => "external-link",
    Eye => "eye",
    EyeOff => "eye-off",
    File => "file",
    FileArchive => "file-archive",
    FileCode => "file-code",
    FileCog => "file-cog",
    FileImage => "file-image",
    FileMusic => "file-music",
    FileSpreadsheet => "file-spreadsheet",
    FileTerminal => "file-terminal",
    FileText => "file-text",
    FileType => "file-type",
    FileVideoCamera => "file-video-camera",
    Film => "film",
    Flag => "flag",
    Folder => "folder",
    FolderInput => "folder-input",
    FolderOpen => "folder-open",
    FolderPlus => "folder-plus",
    Forward => "forward",
    Globe => "globe",
    Hash => "hash",
    HardDrive => "hard-drive",
    Highlighter => "highlighter",
    House => "house",
    Inbox => "inbox",
    Image => "image",
    Info => "info",
    Keyboard => "keyboard",
    LayoutTemplate => "layout-template",
    LoaderCircle => "loader-circle",
    Mail => "mail",
    MailOpen => "mail-open",
    Menu => "menu",
    MessageSquare => "message-square",
    Minus => "minus",
    Monitor => "monitor",
    Moon => "moon",
    MousePointer2 => "mouse-pointer-2",
    MoveUpRight => "move-up-right",
    Music => "music",
    Package => "package",
    Palette => "palette",
    PanelLeft => "panel-left",
    PanelRight => "panel-right",
    PanelBottom => "panel-bottom",
    Paperclip => "paperclip",
    Pencil => "pencil",
    Play => "play",
    Plug => "plug",
    Presentation => "presentation",
    Printer => "printer",
    Redo2 => "redo-2",
    RefreshCw => "refresh-cw",
    Reply => "reply",
    ReplyAll => "reply-all",
    Save => "save",
    Scan => "scan",
    ScrollText => "scroll-text",
    Search => "search",
    Send => "send",
    Server => "server",
    ShieldAlert => "shield-alert",
    Settings => "settings",
    Square => "square",
    SquareDashed => "square-dashed",
    SquarePen => "square-pen",
    Star => "star",
    Sun => "sun",
    Trash => "trash",
    Type => "type",
    Undo2 => "undo-2",
    Unplug => "unplug",
    X => "x",
    ZoomIn => "zoom-in",
    ZoomOut => "zoom-out",
}

/// The icon stroke width for `theme`: the session's choice when one is set
/// (see [`crate::theme::Overlay`]), else metric `icon.stroke_width` in px on
/// Lucide's 24 px grid, else [`DEFAULT_STROKE`].
pub fn stroke_width(theme: &Theme) -> f32 {
    if let Some(stroke) = theme.overlay().stroke {
        return stroke;
    }
    theme
        .dictionary()
        .metrics
        .get("icon.stroke_width")
        .filter(|metric| metric.kind == ResolvedMetricKind::Px && metric.value > 0.0)
        .map_or(DEFAULT_STROKE, |metric| metric.value as f32)
}

fn stroke_key() -> egui::Id {
    egui::Id::unique("toolkit.icon-stroke")
}

/// Make `stroke` the width [`stroke_of`] returns on `ctx` (done by
/// [`crate::install`] from the theme).
pub fn install(ctx: &egui::Context, stroke: f32) {
    ctx.data_mut(|d| d.insert_temp(stroke_key(), stroke));
}

/// The icon stroke width installed on `ctx`, else [`DEFAULT_STROKE`]: the
/// toolkit's own widgets draw their icons with it.
pub fn stroke_of(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp(stroke_key()))
        .unwrap_or(DEFAULT_STROKE)
}

/// `icon`'s SVG source with its stroke at `stroke` and coloured `style.icon_mask`.
pub fn source(icon: Icon, stroke: f32) -> String {
    let mask = mask_string(crate::style::default_tokens().icon_mask);
    icon.svg()
        .replace("stroke=\"currentColor\"", &format!("stroke=\"{mask}\""))
        .replace("stroke-width=\"2\"", &format!("stroke-width=\"{stroke}\""))
}

/// `icon` as an egui image `size` points square, stroked at `stroke` and
/// tinted `colour`.
pub fn image(icon: Icon, stroke: f32, size: f32, colour: Color32) -> Image<'static> {
    let mask = mask_string(crate::style::default_tokens().icon_mask);
    let uri = format!(
        "bytes://lucide/{}@{stroke}@{}.svg",
        icon.name(),
        mask.trim_start_matches('#')
    );
    let bytes = source(icon, stroke).into_bytes();
    Image::new(ImageSource::Bytes {
        uri: Cow::Owned(uri),
        bytes: bytes.into(),
    })
    .fit_to_exact_size(Vec2::splat(size))
    .tint(colour)
}

/// The installed stroke and mask, resolved for the context being drawn.
pub fn image_of(ctx: &egui::Context, icon: Icon, size: f32, colour: Color32) -> Image<'static> {
    let stroke = stroke_of(ctx);
    let mask = mask_string(crate::chrome::Chrome::of(ctx).style.icon_mask);
    let uri = format!(
        "bytes://lucide/{}@{stroke}@{}.svg",
        icon.name(),
        mask.trim_start_matches('#')
    );
    let bytes = icon
        .svg()
        .replace("stroke=\"currentColor\"", &format!("stroke=\"{mask}\""))
        .replace("stroke-width=\"2\"", &format!("stroke-width=\"{stroke}\""))
        .into_bytes();
    Image::new(ImageSource::Bytes {
        uri: Cow::Owned(uri),
        bytes: bytes.into(),
    })
    .fit_to_exact_size(Vec2::splat(size))
    .tint(colour)
}

fn mask_string(mask: design::family::style::StyleColour) -> String {
    let [r, g, b, a] = mask.0;
    format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_an_unmodified_lucide_stroke_svg() {
        for icon in Icon::ALL {
            let svg = icon.svg();
            assert!(svg.contains("viewBox=\"0 0 24 24\""), "{}", icon.name());
            assert!(svg.contains("stroke=\"currentColor\""), "{}", icon.name());
            assert!(svg.contains("stroke-width=\"2\""), "{}", icon.name());
        }
    }

    #[test]
    fn stroke_and_colour_are_rewritten() {
        let svg = source(Icon::Play, 1.25);
        assert!(svg.contains("stroke-width=\"1.25\""));
        assert!(svg.contains("stroke=\"#ffffffff\""));
        assert!(!svg.contains("currentColor"));
        let image = image(Icon::Play, 1.25, 16.0, Color32::WHITE);
        let uri = image.uri().unwrap();
        assert!(uri.ends_with("@ffffffff.svg"));
        assert!(
            !uri.contains('#'),
            "a URI fragment hides the SVG extension from the loader"
        );
    }

    #[test]
    fn the_shipped_design_uses_lucide_default_stroke() {
        // The approved baseline: the shipped token is 1.5 on the 24-unit grid.
        assert_eq!(stroke_width(&Theme::embedded()), DEFAULT_STROKE);
    }
}
