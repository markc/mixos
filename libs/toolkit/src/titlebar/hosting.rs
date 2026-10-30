// SPDX-License-Identifier: MIT OR Apache-2.0
use egui::Context;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Published {
    bar: icons::Bar,
    fingerprint: Option<u64>,
    at: Option<Instant>,
    dirty: bool,
}

impl Default for Published {
    fn default() -> Self {
        let revision = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        Self {
            bar: icons::Bar {
                revision,
                items: Vec::new(),
            },
            fingerprint: None,
            at: None,
            dirty: false,
        }
    }
}

fn published(ctx: &Context) -> Arc<Mutex<Published>> {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Published>>>(egui::Id::unique(
            "toolkit.titlebar.published",
        ))
        .clone()
    })
}

pub fn hosted(ctx: &Context) -> bool {
    let chrome = crate::chrome::Chrome::of(ctx);
    chrome.decorations == design::Decorations::Server
        && design::decorations::has_host()
        && chrome.band.place == design::ActionsPlace::Titlebar
        && crate::drive::menu_service(ctx).is_some()
}

pub fn publish(ctx: &Context, items: &[super::IconBarItem]) {
    publish_at(ctx, items, Instant::now());
}

fn publish_at(ctx: &Context, items: &[super::IconBarItem], now: Instant) {
    if !hosted(ctx) {
        return;
    }
    let mut hash = DefaultHasher::new();
    items.len().hash(&mut hash);
    for item in items {
        item.command.hash(&mut hash);
        item.icon.hash(&mut hash);
        item.tooltip.hash(&mut hash);
        item.enabled.hash(&mut hash);
        item.toggled.hash(&mut hash);
    }
    let fingerprint = hash.finish();
    let shared = published(ctx);
    let mut published = shared.lock().unwrap();
    if published.fingerprint == Some(fingerprint) {
        return;
    }
    let delay = Duration::from_millis(100);
    if let Some(at) = published.at {
        let elapsed = now.saturating_duration_since(at);
        if elapsed < delay {
            drop(published);
            ctx.request_repaint_after(delay - elapsed);
            return;
        }
    }
    published.bar.revision = published.bar.revision.wrapping_add(1);
    published.bar.items = items
        .iter()
        .map(|item| icons::BarItem {
            command: item.command.into(),
            icon: item.icon.name().into(),
            tooltip: item.tooltip.clone(),
            enabled: item.enabled,
            toggled: item.toggled,
        })
        .collect();
    published.fingerprint = Some(fingerprint);
    published.at = Some(now);
    published.dirty = true;
    drop(published);
    ctx.request_repaint();
}

pub(crate) fn registered(ctx: &Context) {
    published(ctx).lock().unwrap().dirty = true;
}

pub(crate) fn take_changed(ctx: &Context) -> Option<(String, serde_json::Value)> {
    if !hosted(ctx) {
        return None;
    }
    let service = crate::drive::menu_service(ctx)?;
    let shared = published(ctx);
    let mut published = shared.lock().unwrap();
    if !std::mem::take(&mut published.dirty) {
        return None;
    }
    Some((
        format!("{service}.ui.titlebar.changed"),
        serde_json::json!({"revision": published.bar.revision}),
    ))
}

pub fn form(ctx: &Context) -> serde_json::Value {
    let chrome = crate::chrome::Chrome::of(ctx);
    let hosted = hosted(ctx);
    let shared = published(ctx);
    let published = shared.lock().unwrap();
    let own = if chrome.decorations == design::Decorations::Server
        && chrome.band.style == design::WindowStyle::Mac
    {
        design::title_band::Alignment::Left
    } else {
        design::title_band::Alignment::Right
    };
    serde_json::json!({
        "place": chrome.band.place.name(),
        "effective": if hosted { "titlebar" } else { "row" },
        "title_align": chrome.band.title.alignment(chrome.band.style.title_alignment()).name(),
        "actions_align": chrome.band.actions.name(),
        "menu_side": chrome.menu_side.name(),
        "caption_side": chrome.captions.alignment(own).name(),
        "requested": {"title_align": chrome.band.title.name(), "caption_side": chrome.captions.name()},
        "hosted": hosted,
        "service": crate::drive::menu_service(ctx),
        "bar": if hosted { published.bar.clone() } else { icons::Bar::default() },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_is_hosted_coalesced_and_revisioned() {
        design::decorations::with_resolver(Arc::new(design::decorations::Resolver::new(
            || design::decorations::SessionFacts { wayland: true, desktops: "MixOS".into(), decoration_global: Some(true) },
            || None, |_| {},
        )), publication_on_host);
    }

    fn publication_on_host() {
        let ctx = Context::default();
        crate::install(
            &ctx,
            &crate::Theme::embedded()
                .framed(design::Decorations::Server, design::CaptionSide::Auto)
                .with_band(design::title_band::Settings::from_names(
                    "auto", "right", "titlebar", "mac",
                )),
        );
        crate::drive::install(&ctx, "demo");
        let item = super::super::IconBarItem {
            command: "file.save",
            icon: crate::Icon::Save,
            tooltip: "Save".into(),
            enabled: true,
            toggled: false,
        };
        let now = Instant::now();
        publish_at(&ctx, std::slice::from_ref(&item), now);
        assert!(take_changed(&ctx).is_none());
        crate::drive::set_service(&ctx, Some("demo"));
        publish_at(&ctx, std::slice::from_ref(&item), now);
        let first = form(&ctx);
        let revision = first["bar"]["revision"].as_u64().unwrap();
        assert!(revision > 1);
        assert_eq!(take_changed(&ctx).unwrap().1["revision"], revision);
        assert!(take_changed(&ctx).is_none());
        let changed = super::super::IconBarItem {
            enabled: false,
            ..item
        };
        publish_at(
            &ctx,
            std::slice::from_ref(&changed),
            now + Duration::from_millis(10),
        );
        assert_eq!(form(&ctx), first);
        assert!(take_changed(&ctx).is_none());
        publish_at(
            &ctx,
            std::slice::from_ref(&changed),
            now + Duration::from_millis(100),
        );
        assert_eq!(take_changed(&ctx).unwrap().1["revision"], revision + 1);
        assert_eq!(form(&ctx)["bar"]["items"][0]["enabled"], false);
        publish_at(&ctx, &[changed], now + Duration::from_millis(200));
        assert!(take_changed(&ctx).is_none());
        crate::drive::set_service(&ctx, None);
        assert_eq!(form(&ctx)["effective"], "row");
    }

    #[test]
    fn foreign_ssd_keeps_menu_and_actions_in_the_app() {
        for desktop in ["KDE", "GNOME", "MixOS", "KDE:mixos"] {
            design::decorations::with_resolver(Arc::new(design::decorations::Resolver::new(
                move || design::decorations::SessionFacts { wayland: true, desktops: desktop.into(), decoration_global: Some(true) },
                || None, |_| {},
            )), || {
                let ctx = Context::default();
                crate::install(&ctx, &crate::Theme::embedded()
                    .framed(design::Decorations::Server, design::CaptionSide::Auto)
                    .with_menus(design::MenuStyle::Titlebar, design::MenuSide::Left)
                    .with_band(design::title_band::Settings::from_names("auto", "right", "titlebar", "mac")));
                crate::drive::install(&ctx, "demo");
                crate::drive::set_service(&ctx, Some("demo"));
                let host = design::decorations::prefers_client_side(desktop);
                assert_eq!(hosted(&ctx), host);
                assert_eq!(crate::menu::effective(&ctx), if host { design::MenuStyle::Titlebar } else { design::MenuStyle::Button });
                assert_eq!(crate::menu::form(&ctx)["hosted"], host);
                assert_eq!(form(&ctx)["hosted"], host);
            });
        }
    }
}
