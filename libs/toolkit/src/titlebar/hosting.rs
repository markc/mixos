// SPDX-License-Identifier: MIT OR Apache-2.0
use egui::Context;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct Published {
    bar: icons::Bar,
    at: Option<Instant>,
}

fn key() -> egui::Id { egui::Id::unique("toolkit.titlebar.published") }

pub fn hosted(ctx: &Context) -> bool {
    let chrome = crate::chrome::Chrome::of(ctx);
    chrome.decorations == design::Decorations::Server
        && chrome.band.place == design::ActionsPlace::Titlebar
        && crate::drive::menu_service(ctx).is_some()
}

pub fn publish(ctx: &Context, items: &[super::IconBarItem]) {
    publish_at(ctx,items,Instant::now());
}

fn publish_at(ctx: &Context, items: &[super::IconBarItem], now: Instant) {
    if !hosted(ctx) { return; }
    let items: Vec<_> = items.iter().map(|item| icons::BarItem {
        command: item.command.into(), icon: item.icon.name().into(), tooltip: item.tooltip.clone(),
        enabled: item.enabled, toggled: item.toggled,
    }).collect();
    let mut published = ctx.data(|d| d.get_temp::<Published>(key())).unwrap_or_default();
    if published.bar.items == items { return; }
    let delay = Duration::from_millis(100);
    if let Some(at) = published.at && now.saturating_duration_since(at) < delay {
        ctx.request_repaint_after(delay - now.saturating_duration_since(at));
        return;
    }
    published.bar.revision = published.bar.revision.wrapping_add(1);
    published.bar.items = items;
    published.at = Some(now);
    ctx.data_mut(|d| d.insert_temp(key(), published));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_is_hosted_coalesced_and_revisioned() {
        let ctx = Context::default();
        crate::install(&ctx,&crate::Theme::embedded().framed(design::Decorations::Server,design::CaptionSide::Auto)
            .with_band(design::title_band::Settings::from_names("auto","right","titlebar","mac")));
        crate::drive::install(&ctx,"demo");
        let item = super::super::IconBarItem { command:"file.save",icon:crate::Icon::Save,tooltip:"Save".into(),enabled:true,toggled:false };
        let now = Instant::now();
        publish_at(&ctx,&[item.clone()],now);
        assert_eq!(form(&ctx)["bar"]["revision"],0);
        crate::drive::set_service(&ctx,Some("demo"));
        publish_at(&ctx,&[item.clone()],now);
        let first = form(&ctx);
        assert_eq!(first["bar"]["revision"],1);
        let changed = super::super::IconBarItem { enabled:false,..item };
        publish_at(&ctx,&[changed.clone()],now+Duration::from_millis(10));
        assert_eq!(form(&ctx),first);
        publish_at(&ctx,&[changed.clone()],now+Duration::from_millis(100));
        assert_eq!(form(&ctx)["bar"]["revision"],2);
        assert_eq!(form(&ctx)["bar"]["items"][0]["enabled"],false);
        publish_at(&ctx,&[changed],now+Duration::from_millis(200));
        assert_eq!(form(&ctx)["bar"]["revision"],2);
        crate::drive::set_service(&ctx,None);
        assert_eq!(form(&ctx)["effective"],"row");
    }
}

pub fn form(ctx: &Context) -> serde_json::Value {
    let chrome = crate::chrome::Chrome::of(ctx);
    let hosted = hosted(ctx);
    let published = ctx.data(|d| d.get_temp::<Published>(key())).unwrap_or_default();
    let own = if chrome.decorations == design::Decorations::Server && chrome.band.style == design::WindowStyle::Mac {
        design::title_band::Alignment::Left
    } else { design::title_band::Alignment::Right };
    serde_json::json!({
        "place":chrome.band.place.name(), "effective":if hosted { "titlebar" } else { "row" },
        "title_align":chrome.band.title.alignment(chrome.band.style).name(), "actions_align":chrome.band.actions.name(),
        "menu_side":chrome.menu_side.name(), "caption_side":chrome.captions.alignment(own).name(),
        "requested":{"title_align":chrome.band.title.name(), "caption_side":chrome.captions.name()},
        "hosted":hosted, "service":crate::drive::menu_service(ctx),
        "bar":if hosted { published.bar } else { icons::Bar::default() },
    })
}
