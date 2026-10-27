// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Which windows get chrome, and how much room it takes.
//!
//! The installed theme is per compositor thread, like every other piece of
//! compd frame state: the backend installs it once at startup
//! ([`install`]); a later reinstall (a theme change over the Bus) bumps the
//! generation, which repaints every chrome once.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::layout::{ChromePart, DecoExtents, Vec2};
use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::utils::{Logical, Rectangle, Size};

use crate::theme::ChromeTheme;

thread_local! {
    static THEME: RefCell<Option<Rc<ChromeTheme>>> = const { RefCell::new(None) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static MENU_SERVICES: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
    static MENU_EMIT: RefCell<Option<MenuEmit>> = const { RefCell::new(None) };
    static ACTIONS: RefCell<BTreeMap<String, icons::Bar>> = const { RefCell::new(BTreeMap::new()) };
    static ACTION_EMIT: RefCell<Option<ActionEmit>> = const { RefCell::new(None) };
}

type MenuEmit = Rc<dyn Fn(&str, Option<&str>, design::MenuSide, f32) -> bool>;
type ActionEmit = Rc<dyn Fn(&str, Option<&str>, &str) -> bool>;

pub fn action_emitter(emit: impl Fn(&str, Option<&str>, &str) -> bool + 'static) {
    ACTION_EMIT.with_borrow_mut(|slot| *slot = Some(Rc::new(emit)));
}

pub fn update_actions(service: &str, bar: icons::Bar) -> bool {
    if !bar.valid() || !MENU_SERVICES.with_borrow(|live| live.contains(service)) {
        return false;
    }
    let changed = ACTIONS.with_borrow_mut(|bars| {
        if bars.get(service) == Some(&bar) {
            return false;
        }
        bars.insert(service.into(), bar);
        true
    });
    if changed {
        GENERATION.set(GENERATION.get().wrapping_add(1));
    }
    changed
}

pub fn action_service(window: &Window) -> Option<String> {
    if installed()?.band.place != design::ActionsPlace::Titlebar || !decorated(window) {
        return None;
    }
    let app = app_id(window)?;
    MENU_SERVICES.with_borrow(|live| menu_service(&app, live).map(str::to_owned))
}

pub fn actions(window: &Window) -> icons::Bar {
    action_service(window)
        .and_then(|service| ACTIONS.with_borrow(|bars| bars.get(&service).cloned()))
        .unwrap_or_default()
}

pub fn execute_action(window: &Window, command: &str) -> bool {
    let Some(service) = action_service(window) else {
        return false;
    };
    let bar = actions(window);
    if !bar
        .items
        .iter()
        .any(|item| item.command == command && item.enabled)
    {
        return false;
    }
    let app = app_id(window).unwrap_or_default();
    ACTION_EMIT.with_borrow(|emit| {
        emit.as_ref()
            .is_some_and(|emit| emit(&service, menu_viewport(&app), command))
    })
}

pub fn click_action(window: &Window, revision: u64, index: usize) -> bool {
    let bar = actions(window);
    if bar.revision != revision {
        return false;
    }
    bar.items
        .get(index)
        .is_some_and(|item| execute_action(window, &item.command))
}

pub fn layout(window: &Window, size: Vec2) -> Option<Rc<crate::layout::ChromeLayout>> {
    crate::render::layout_for(window, size)
}

/// The scene host owns the Bus connection; chrome only queues through it.
pub fn menu_emitter(emit: impl Fn(&str, Option<&str>, design::MenuSide, f32) -> bool + 'static) {
    MENU_EMIT.with_borrow_mut(|slot| *slot = Some(Rc::new(emit)));
}

pub fn menu_services(services: BTreeSet<String>) -> bool {
    let services = services
        .into_iter()
        .filter(|name| menu_service_name(name))
        .collect();
    let changed = MENU_SERVICES.with_borrow_mut(|live| {
        if *live == services {
            return false;
        }
        *live = services;
        true
    });
    let repaint = changed
        && installed().is_some_and(|theme| {
            theme.menu_style == design::MenuStyle::Titlebar
                || theme.band.place == design::ActionsPlace::Titlebar
        });
    MENU_SERVICES.with_borrow(|live| {
        ACTIONS.with_borrow_mut(|bars| bars.retain(|service, _| live.contains(service)))
    });
    if repaint {
        GENERATION.set(GENERATION.get().wrapping_add(1));
    }
    repaint
}

/// The same live registry used by hit testing and `comp.window.menu`.
pub fn live_menu_services() -> Vec<String> {
    MENU_SERVICES.with_borrow(|live| live.iter().cloned().collect())
}

fn menu_service_name(service: &str) -> bool {
    !service.is_empty() && !service.contains('.')
}

pub fn menu_service<'a>(app_id: &'a str, services: &BTreeSet<String>) -> Option<&'a str> {
    let service = app_id.strip_prefix("dev.mixos.")?.split('.').next()?;
    (menu_service_name(service) && services.contains(service)).then_some(service)
}

/// A dotted app-id suffix names the application's drive viewport. The first
/// segment remains the registered Bus service (e.g. email.compose-2).
pub fn menu_viewport(app_id: &str) -> Option<&str> {
    app_id
        .strip_prefix("dev.mixos.")?
        .split_once('.')
        .map(|(_, viewport)| viewport)
        .filter(|v| !v.is_empty())
}

fn app_id(window: &Window) -> Option<String> {
    if let Some(x11) = window.x11_surface() {
        return Some(x11.class());
    }
    window.toplevel().and_then(|toplevel| {
        smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok().and_then(|role| role.app_id.clone()))
        })
    })
}

pub fn menu_side(window: &Window) -> Option<design::MenuSide> {
    menu_target(window).map(|(_, _, side)| side)
}

fn menu_target(window: &Window) -> Option<(String, Option<String>, design::MenuSide)> {
    let theme = installed()?;
    if theme.menu_style != design::MenuStyle::Titlebar || !decorated(window) {
        return None;
    }
    let app_id = app_id(window)?;
    MENU_SERVICES.with_borrow(|live| {
        menu_service(&app_id, live).map(|service| {
            (
                service.to_owned(),
                menu_viewport(&app_id).map(str::to_owned),
                theme.menu_side,
            )
        })
    })
}

pub fn open_menu(window: &Window) -> bool {
    let Some((service, viewport, side)) = menu_target(window) else {
        return false;
    };
    let Some(theme) = installed() else {
        return false;
    };
    let geometry = window.geometry().size;
    let size = crate::render::layout_of(window)
        .filter(|layout| layout.content.w > 1.0 && layout.content.h > 1.0)
        .map_or(
            crate::layout::vec2(geometry.w as f32, geometry.h as f32),
            |layout| crate::layout::vec2(layout.content.w, layout.content.h),
        );
    let Some(layout) = layout(window, size) else {
        return false;
    };
    let Some(x) = menu_x(&layout, geometry.w as f32, side) else {
        return false;
    };
    let emit = MENU_EMIT.with_borrow(Clone::clone);
    emit.is_some_and(|emit| emit(&service, viewport.as_deref(), side, x))
}

/// The popup anchor in client content units: the button's left edge, or its
/// right edge on the right, where the menu right-aligns to hang from it.
fn menu_x(
    layout: &crate::layout::ChromeLayout,
    content_width: f32,
    side: design::MenuSide,
) -> Option<f32> {
    let button = layout.menu_button?;
    let edge = match side {
        design::MenuSide::Left => button.x,
        design::MenuSide::Center => button.x + button.w / 2.0,
        design::MenuSide::Right => button.x + button.w,
    };
    // Layout is in logical slot pixels; the client may be scaled into that slot.
    (layout.content.w > 0.0 && content_width > 0.0).then(|| {
        ((edge - layout.content.x) * content_width / layout.content.w).clamp(0.0, content_width)
    })
}

/// Install `theme` for this compositor thread (startup, or a theme change).
pub fn install(theme: ChromeTheme) {
    THEME.with_borrow_mut(|slot| *slot = Some(Rc::new(theme)));
    GENERATION.set(GENERATION.get().wrapping_add(1));
}

/// The installed theme, if any (none: no chrome is drawn).
pub fn installed() -> Option<Rc<ChromeTheme>> {
    THEME.with_borrow(Clone::clone)
}

/// Moves on every [`install`]: chrome state carries it, so a new theme
/// repaints every window's chrome once.
pub fn generation() -> u64 {
    GENERATION.get()
}

/// Whether server-side decorations are enabled (the `decorations_ssd`
/// preference, as the xdg-decoration handlers read it).
pub fn ssd_enabled() -> bool {
    model::environment::preference::base::decorations_ssd()
}

/// Whether compd draws `window`'s chrome, with a theme installed:
/// - an xdg toplevel that acked server-side decorations and still has its
///   decoration object, not fullscreen (fullscreen owns its whole output);
/// - an X11 window by `policy::x11::decoration_mode`: not
///   override-redirect, decorations on, not fullscreen, and no Motif
///   refusal. Read live, so a MotifHints change (property_notify schedules
///   the frame) takes effect at the next frame with no stored mode.
pub fn decorated(window: &Window) -> bool {
    if let Some(x11) = window.x11_surface() {
        return installed().is_some()
            && policy::x11::decoration_mode(
                x11.is_override_redirect(),
                ssd_enabled(),
                x11.is_fullscreen(),
                x11.is_decorated(),
            ) == policy::x11::DecorationMode::ServerSide;
    }
    let Some(toplevel) = window.toplevel() else {
        return false;
    };
    installed().is_some()
        && dispatcher::wayland::xdg::decoration::mode::server_side(toplevel)
        && !toplevel.with_committed_state(|state| {
            state.is_some_and(|state| state.states.contains(xdg_toplevel::State::Fullscreen))
        })
}

/// The room `window`'s chrome takes around its content (logical px), when it
/// is [`decorated`].
pub fn extents(window: &Window) -> Option<DecoExtents> {
    if !decorated(window) {
        return None;
    }
    installed().map(|theme| DecoExtents::of(&theme.deco))
}

/// How far `window`'s chrome reaches outside its content on any side (logical
/// px; 0 without chrome): what a cull of the content's slot must grow by so it
/// never drops a window whose titlebar still shows. The shadow is not counted:
/// losing it under an occluder costs nothing visible.
pub fn margin(window: &Window) -> f64 {
    extents(window).map_or(0.0, |e| e.top.max(e.left).max(e.right).max(e.bottom) as f64)
}

/// What a point over a window's chrome hits: the part, and whether it is over
/// the caption-button cluster (mac shows its glyphs on cluster hover).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeHit {
    pub part: ChromePart,
    pub in_cluster: bool,
}

/// The chrome part of `window` under a point `content_relative` to its slot's
/// top-left (logical px), for a slot of `slot_size`. `None` without chrome,
/// over the content, or outside the chrome and its resize band: those are not
/// the chrome's to answer.
pub fn hit(
    window: &Window,
    content_relative: (f64, f64),
    slot_size: Size<i32, Logical>,
) -> Option<ChromeHit> {
    let extents = extents(window)?;
    let theme = installed()?;
    let layout = layout(
        window,
        crate::layout::vec2(slot_size.w.max(0) as f32, slot_size.h.max(0) as f32),
    )?;
    let p = frame_point(extents, content_relative);
    match layout.hit_test(p) {
        ChromePart::Content | ChromePart::Outside => None,
        part => Some(ChromeHit {
            part,
            in_cluster: layout.button_cluster.contains(p),
        }),
    }
}

/// Where `window`'s chrome parts are in its Space (logical px), for a slot at
/// `slot_origin` of `slot_size`: what a gate needs to aim injected input at a
/// titlebar or a caption button without assuming any style's layout.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceParts {
    /// The whole frame: what the content is cut to, rounded by the radius.
    pub frame: Rectangle<f64, Logical>,
    pub titlebar: Rectangle<f64, Logical>,
    pub buttons: Vec<(crate::layout::CaptionButton, Rectangle<f64, Logical>)>,
}

pub fn parts(
    window: &Window,
    slot_origin: smithay::utils::Point<i32, Logical>,
    slot_size: Size<i32, Logical>,
) -> Option<SpaceParts> {
    let extents = extents(window)?;
    let theme = installed()?;
    let layout = layout(
        window,
        crate::layout::vec2(slot_size.w.max(0) as f32, slot_size.h.max(0) as f32),
    )?;
    let (ox, oy) = (
        slot_origin.x as f64 - extents.left as f64,
        slot_origin.y as f64 - extents.top as f64,
    );
    let place = |r: crate::layout::Rect| -> Rectangle<f64, Logical> {
        Rectangle::new(
            (ox + r.x as f64, oy + r.y as f64).into(),
            (r.w as f64, r.h as f64).into(),
        )
    };
    Some(SpaceParts {
        frame: place(layout.window),
        titlebar: place(layout.titlebar),
        buttons: layout
            .buttons
            .iter()
            .copied()
            .chain(
                layout
                    .menu_button
                    .map(|rect| (crate::layout::CaptionButton::Menu, rect)),
            )
            .map(|(button, rect)| (button, place(rect)))
            .collect(),
    })
}

/// Projection of the owning placement model, independent of configure ACKs.
#[derive(Default)]
struct Constrained(std::sync::atomic::AtomicBool);

pub fn set_constrained(window: &Window, constrained: bool) {
    window
        .user_data()
        .insert_if_missing_threadsafe(Constrained::default);
    window
        .user_data()
        .get::<Constrained>()
        .unwrap()
        .0
        .store(constrained, std::sync::atomic::Ordering::Relaxed);
}

pub fn constrained(window: &Window) -> bool {
    window
        .user_data()
        .get::<Constrained>()
        .is_some_and(|value| value.0.load(std::sync::atomic::Ordering::Relaxed))
}

/// The content rectangle that leaves room for `window`'s chrome inside
/// `outer` (an output's usable area, for a maximised window). `outer` itself
/// when the window has no chrome.
pub fn content_area(window: &Window, outer: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    // A placement is also computed while fullscreen exits. Its old committed
    // fullscreen flag must not suppress the chrome inset of the normal frame.
    let decorated = if let Some(x11) = window.x11_surface() {
        policy::x11::decoration_mode(
            x11.is_override_redirect(),
            ssd_enabled(),
            false,
            x11.is_decorated(),
        ) == policy::x11::DecorationMode::ServerSide
    } else {
        window
            .toplevel()
            .is_some_and(dispatcher::wayland::xdg::decoration::mode::server_side)
    };
    match installed()
        .filter(|_| decorated)
        .map(|theme| DecoExtents::of(&theme.deco))
    {
        Some(extents) => inset(outer, extents),
        None => outer,
    }
}

/// `outer` less `extents`, rounded outward so the chrome always fits.
pub fn inset(outer: Rectangle<i32, Logical>, e: DecoExtents) -> Rectangle<i32, Logical> {
    let (left, top) = (e.left.ceil() as i32, e.top.ceil() as i32);
    let (right, bottom) = (e.right.ceil() as i32, e.bottom.ceil() as i32);
    Rectangle::new(
        (outer.loc.x + left, outer.loc.y + top).into(),
        (
            (outer.size.w - left - right).max(1),
            (outer.size.h - top - bottom).max(1),
        )
            .into(),
    )
}

/// The window-local frame point for a point relative to the content's
/// top-left: the chrome's layout space has the frame's corner at 0,0.
pub fn frame_point(extents: DecoExtents, content_relative: (f64, f64)) -> Vec2 {
    crate::layout::vec2(
        content_relative.0 as f32 + extents.left,
        content_relative.1 as f32 + extents.top,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{ChromeLayout, ChromeStyle, Mode, Scheme, presets, vec2};

    #[test]
    fn a_maximised_content_area_leaves_the_chrome_on_the_output() {
        for style in ChromeStyle::ALL {
            let theme = presets::resolve(style, Scheme::Ocean, Mode::Light);
            let e = DecoExtents::of(&theme);
            let outer = Rectangle::new((0, 30).into(), (1920, 1050).into());
            let content = inset(outer, e);
            assert_eq!(
                content.loc.y - outer.loc.y,
                e.top.ceil() as i32,
                "{style:?}"
            );
            // The frame around the content fits inside the usable area.
            let layout =
                ChromeLayout::compute(&theme, vec2(content.size.w as f32, content.size.h as f32));
            assert!(layout.window.w <= outer.size.w as f32 + 0.5, "{style:?}");
            assert!(layout.window.h <= outer.size.h as f32 + 0.5, "{style:?}");
        }
    }

    #[test]
    fn content_points_map_into_the_frame_by_the_extents() {
        let theme = presets::resolve(ChromeStyle::Win11, Scheme::Ocean, Mode::Light);
        let e = DecoExtents::of(&theme);
        let layout = ChromeLayout::compute(&theme, vec2(400.0, 300.0));
        let origin = frame_point(e, (0.0, 0.0));
        assert_eq!(origin, layout.content_offset());
        // Just above the content's top edge is the titlebar.
        let above = frame_point(e, (200.0, -2.0));
        assert_eq!(
            layout.hit_test(above),
            crate::layout::ChromePart::TitlebarDrag
        );
    }

    #[test]
    fn install_bumps_the_generation() {
        let before = generation();
        install(ChromeTheme::from_source(ChromeStyle::Mac, None));
        assert_ne!(generation(), before);
        assert!(installed().is_some());
    }

    #[test]
    fn only_registered_mixos_app_services_get_a_menu() {
        let services = BTreeSet::from(["ced".into(), "prefs".into()]);
        assert_eq!(menu_service("dev.mixos.ced", &services), Some("ced"));
        for id in ["ced", "org.example.ced", "dev.mixos.", "dev.mixos.term"] {
            assert_eq!(menu_service(id, &services), None);
        }
    }

    #[test]
    fn composers_share_the_service_but_address_their_own_menu() {
        let live = BTreeSet::from(["email".into()]);
        for (app, viewport) in [
            ("dev.mixos.email", None),
            ("dev.mixos.email.compose-2", Some("compose-2")),
        ] {
            assert_eq!(menu_service(app, &live), Some("email"));
            assert_eq!(menu_viewport(app), viewport);
        }
        assert_eq!(menu_service("dev.mixos.missing.compose-2", &live), None);
    }

    #[test]
    fn registry_changes_repaint_only_relevant_titlebar_menus() {
        menu_services(BTreeSet::new());
        install(ChromeTheme::from_source(ChromeStyle::Mac, None));
        let before = generation();
        assert!(!menu_services(BTreeSet::from(["ced".into()])));
        assert_eq!(generation(), before);
        let mut theme = ChromeTheme::from_source(ChromeStyle::Mac, None);
        theme.menu_style = design::MenuStyle::Titlebar;
        install(theme);
        let before = generation();
        assert!(!menu_services(BTreeSet::from([
            "ced".into(),
            "background.worker".into(),
            "".into()
        ])));
        assert_eq!(generation(), before);
        assert!(menu_services(BTreeSet::from([
            "ced".into(),
            "prefs".into()
        ])));
        assert_ne!(generation(), before);
        let before = generation();
        assert!(!menu_services(BTreeSet::from([
            "ced".into(),
            "prefs".into(),
            "another.worker".into()
        ])));
        assert_eq!(generation(), before);
        assert!(menu_services(BTreeSet::from(["prefs".into()])));
        assert_ne!(generation(), before);
        install(ChromeTheme::from_source(ChromeStyle::Mac, None));
        let before = generation();
        assert!(!menu_services(BTreeSet::new()));
        assert_eq!(generation(), before);
    }

    #[test]
    fn menu_anchor_is_in_client_content_coordinates() {
        for style in ChromeStyle::ALL {
            let theme = presets::resolve(style, Scheme::Ocean, Mode::Light);
            for side in design::MenuSide::ALL {
                let layout =
                    ChromeLayout::compute_with_menu(&theme, vec2(640.0, 480.0), Some(side));
                let button = layout.menu_button.unwrap();
                let edge = match side {
                    design::MenuSide::Left => button.x,
                    design::MenuSide::Center => button.x + button.w / 2.0,
                    design::MenuSide::Right => button.x + button.w,
                };
                let x = edge - layout.content.x;
                for ratio in [0.5, 1.0, 2.0] {
                    assert!(
                        (menu_x(&layout, 640.0 * ratio, side).unwrap() - x * ratio).abs() < 0.001
                    );
                }
                assert!(menu_x(&layout, 0.0, side).is_none());
            }
            let layout = ChromeLayout::compute(&theme, vec2(640.0, 480.0));
            assert!(menu_x(&layout, 640.0, design::MenuSide::Left).is_none());
        }
    }
}
