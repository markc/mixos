// SPDX-License-Identifier: MIT OR Apache-2.0
//! Portable frame state and optional compd placement. The caller supplies its
//! existing Bus transport; this module never starts a connection or switches
//! the current workspace. Refusals stop the restore sequence silently.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Names preserve all five choice axes, including `None` = follow session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeChoice {
    pub scheme: Option<String>, pub style: Option<String>, pub mode: Option<String>,
    pub decorations: Option<String>, pub captions: Option<String>,
}
impl From<crate::theme::Choice> for ThemeChoice {
    fn from(c: crate::theme::Choice) -> Self {
        Self { scheme: c.scheme.map(|v| v.name().into()),
            style: c.style.map(|v| v.map_or("own", |s| s.name()).into()),
            mode: c.mode.map(|v| v.name().into()),
            decorations: c.decorations.map(|v| v.name().into()),
            captions: c.captions.map(|v| v.name().into()) }
    }
}
impl ThemeChoice {
    pub fn choice(&self) -> crate::theme::Choice {
        crate::theme::Choice {
            scheme: self.scheme.as_deref().and_then(design::Scheme::from_name),
            style: self.style.as_deref().and_then(|s| if s == "own" { Some(None) } else { design::Style::from_name(s).map(Some) }),
            mode: self.mode.as_deref().and_then(design::Mode::from_name),
            decorations: self.decorations.as_deref().and_then(design::RequestedDecorations::from_name),
            captions: self.captions.as_deref().and_then(design::CaptionSide::from_name),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Frame {
    pub size: Option<[f32; 2]>,
    pub maximised: bool,
    pub theme: ThemeChoice,
    pub placement: Option<Placement>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub output: String, pub workspace: u32,
    pub x: f64, pub y: f64, pub width: u32, pub height: u32,
}
impl Frame {
    /// Apply portable values to the app's titlebar viewport builder.
    pub fn viewport(&self, mut builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        if let Some([w, h]) = self.size.filter(|s| s.iter().all(|n| n.is_finite() && *n > 0.0)) {
            builder = builder.with_inner_size([w, h]);
        }
        builder.with_maximized(self.maximised)
    }
    /// Keep the last normal size while maximised or minimised.
    pub fn capture(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            let v = i.viewport();
            if let Some(max) = v.maximized { self.maximised = max; }
            if !self.maximised && v.minimized != Some(true)
                && let Some(rect) = v.inner_rect
                && rect.width() > 0.0 && rect.height() > 0.0 {
                self.size = Some([rect.width(), rect.height()]);
            }
        });
    }

    /// Probe compd by reading its window catalogue. `None` means absent,
    /// refused, or no window belonging to this process. Never match another PID.
    pub async fn own_window<F, Fut>(app_id: &str, mut call: F) -> Option<Value>
    where F: FnMut(&'static str, Value) -> Fut, Fut: Future<Output = Option<Value>> {
        let list = call("comp.windows.list", json!({})).await?;
        list["windows"].as_array()?.iter().find(|w|
            w["app_id"] == app_id && w["pid"].as_u64() == Some(u64::from(std::process::id()))
            && w["id"].is_u64() && w["generation"].is_u64()).cloned()
    }

    /// Runtime probe, wait for mapping, find the same id/generation as
    /// `workshop.show`, then restore without activating or following the window.
    pub async fn restore<F, Fut>(&self, app_id: &str, mut call: F)
    where F: FnMut(&'static str, Value) -> Fut, Fut: Future<Output = Option<Value>> {
        let Some(p) = &self.placement else { return; };
        if call("comp.windows.list", json!({})).await.is_none() { return; }
        if call("comp.window.wait", json!({"match":{"app_id":app_id},"until":"mapped","timeout_ms":10000})).await.is_none() { return; }
        let Some(w) = Self::own_window(app_id, &mut call).await else { return; };
        let target = json!({"id":w["id"],"generation":w["generation"]});
        let mut workspace = target.clone();
        workspace["index"] = json!(p.workspace); workspace["follow"] = json!(false);
        if call("comp.window.send_to_workspace", workspace).await.is_none() { return; }
        if self.maximised {
            let _ = call("comp.window.maximize", target).await;
        } else {
            let mut place = target;
            for (key, value) in [("output", json!(p.output)), ("x", json!(p.x)), ("y", json!(p.y)),
                ("width", json!(p.width)), ("height", json!(p.height))] { place[key] = value; }
            let _ = call("comp.window.place", place).await;
        }
    }

    /// Read placement at exit only when the runtime catalogue identified compd.
    pub fn capture_comp(&mut self, w: &Value) {
        let parsed = (|| Some(Placement { output: w["output"].as_str()?.into(),
            workspace: u32::try_from(w["workspace"].as_u64()?).ok()?,
            x: w["x"].as_f64()?, y: w["y"].as_f64()?,
            width: dimension(&w["width"])? ,
            height: dimension(&w["height"])? }))();
        if let Some(p) = parsed { self.placement = Some(p); }
    }
}

fn dimension(value: &Value) -> Option<u32> {
    let n = value.as_f64()?;
    (n.is_finite() && n >= 1.0 && n <= f64::from(u32::MAX)).then_some(n.round() as u32)
}
