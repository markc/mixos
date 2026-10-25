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
    pub scheme: Option<String>,
    pub style: Option<String>,
    pub mode: Option<String>,
    pub decorations: Option<String>,
    pub captions: Option<String>,
}
impl From<crate::theme::Choice> for ThemeChoice {
    fn from(c: crate::theme::Choice) -> Self {
        Self {
            scheme: c.scheme.map(|v| v.name().into()),
            style: c.style.map(|v| v.map_or("own", |s| s.name()).into()),
            mode: c.mode.map(|v| v.name().into()),
            decorations: c.decorations.map(|v| v.name().into()),
            captions: c.captions.map(|v| v.name().into()),
        }
    }
}
impl ThemeChoice {
    pub fn choice(&self) -> crate::theme::Choice {
        crate::theme::Choice {
            scheme: self.scheme.as_deref().and_then(design::Scheme::from_name),
            style: self.style.as_deref().and_then(|s| {
                if s == "own" {
                    Some(None)
                } else {
                    design::Style::from_name(s).map(Some)
                }
            }),
            mode: self.mode.as_deref().and_then(design::Mode::from_name),
            decorations: self
                .decorations
                .as_deref()
                .and_then(design::RequestedDecorations::from_name),
            captions: self
                .captions
                .as_deref()
                .and_then(design::CaptionSide::from_name),
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
    pub output: String,
    pub workspace: u32,
    pub x: f64,
    pub y: f64,
    pub width: u32,
    pub height: u32,
}
impl Frame {
    /// Apply portable values to the app's titlebar viewport builder.
    pub fn viewport(&self, mut builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        if let Some([w, h]) = self
            .size
            .filter(|s| s.iter().all(|n| n.is_finite() && *n > 0.0))
        {
            builder = builder.with_inner_size([w, h]);
        }
        builder.with_maximized(self.maximised)
    }
    /// Keep the last normal size while maximised or minimised.
    pub fn capture(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            let v = i.viewport();
            if let Some(max) = v.maximized {
                self.maximised = max;
            }
            if !self.maximised
                && v.minimized != Some(true)
                && let Some(rect) = v.inner_rect
                && rect.width() > 0.0
                && rect.height() > 0.0
            {
                self.size = Some([rect.width(), rect.height()]);
            }
        });
    }

    /// Probe compd by reading its window catalogue. `None` means absent,
    /// refused, or no window belonging to this process. Never match another PID.
    pub async fn own_window<F, Fut>(app_id: &str, mut call: F) -> Option<Value>
    where
        F: FnMut(&'static str, Value) -> Fut,
        Fut: Future<Output = Option<Value>>,
    {
        let list = call("comp.windows.list", json!({})).await?;
        list["windows"]
            .as_array()?
            .iter()
            .find(|w| {
                w["app_id"] == app_id
                    && w["pid"].as_u64() == Some(u64::from(std::process::id()))
                    && w["id"].is_u64()
                    && w["generation"].is_u64()
            })
            .cloned()
    }

    /// Runtime probe, wait for mapping, find the same id/generation as
    /// `workshop.show`, then restore without activating or following the window.
    pub async fn restore<F, Fut>(&self, app_id: &str, mut call: F)
    where
        F: FnMut(&'static str, Value) -> Fut,
        Fut: Future<Output = Option<Value>>,
    {
        let Some(p) = &self.placement else {
            return;
        };
        if call("comp.windows.list", json!({})).await.is_none() {
            return;
        }
        if call(
            "comp.window.wait",
            json!({"match":{"app_id":app_id},"until":"mapped","timeout_ms":10000}),
        )
        .await
        .is_none()
        {
            return;
        }
        let Some(w) = Self::own_window(app_id, &mut call).await else {
            return;
        };
        let target = json!({"id":w["id"],"generation":w["generation"]});
        let mut workspace = target.clone();
        workspace["index"] = json!(p.workspace);
        workspace["follow"] = json!(false);
        if call("comp.window.send_to_workspace", workspace)
            .await
            .is_none()
        {
            return;
        }
        let restored = if self.maximised {
            call("comp.window.maximize", target.clone()).await
        } else {
            let mut place = target.clone();
            for (key, value) in [
                ("output", json!(p.output)),
                ("x", json!(p.x)),
                ("y", json!(p.y)),
                ("width", json!(p.width)),
                ("height", json!(p.height)),
            ] {
                place[key] = value;
            }
            call("comp.window.place", place).await
        };
        // A placement refusal must not strand the window on a different
        // workspace. Restore its previous workspace with the same generation
        // fence, without following it. A newer window is never addressed.
        if restored.is_none()
            && w["workspace"].as_u64() != Some(u64::from(p.workspace))
            && let Some(index) = w["workspace"].as_u64()
        {
            let mut previous = target;
            previous["index"] = json!(index);
            previous["follow"] = json!(false);
            let _ = call("comp.window.send_to_workspace", previous).await;
        }
    }

    /// Read the owning window and its output at exit. Place coordinates are
    /// output-local window geometry, unlike the catalogue's global surface box.
    pub async fn capture_exit<F, Fut>(&mut self, app_id: &str, mut call: F)
    where
        F: FnMut(&'static str, Value) -> Fut,
        Fut: Future<Output = Option<Value>>,
    {
        let Some(window) = Self::own_window(app_id, &mut call).await else { return; };
        let Some(output) = window["output"].as_str() else { return; };
        let Some(output) = call("comp.props.get", json!({"path":format!("outputs.{output}")})).await else { return; };
        self.capture_comp(&window, &output);
    }

    /// Capture window geometry relative to the owning output, excluding the
    /// client's shadows and surface margins. Incomplete reads retain old state.
    pub fn capture_comp(&mut self, w: &Value, output: &Value) {
        let parsed = (|| {
            Some(Placement {
                output: w["output"].as_str()?.into(),
                workspace: u32::try_from(w["workspace"].as_u64()?).ok()?,
                x: w["window_x"].as_f64()? - output["x"].as_f64()?,
                y: w["window_y"].as_f64()? - output["y"].as_f64()?,
                width: dimension(&w["window_width"])?,
                height: dimension(&w["window_height"])?,
            })
        })();
        if let Some(p) = parsed {
            self.placement = Some(p);
        }
    }
}

fn dimension(value: &Value) -> Option<u32> {
    let n = value.as_f64()?;
    (n.is_finite() && n >= 1.0 && n <= f64::from(u32::MAX)).then_some(n.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> Frame {
        Frame {
            size: Some([900.0, 600.0]),
            maximised: false,
            placement: Some(Placement {
                output: "o_main".into(),
                workspace: 2,
                x: 20.0,
                y: 30.0,
                width: 900,
                height: 600,
            }),
            ..Frame::default()
        }
    }
    fn window() -> Value {
        json!({"id":7,"generation":3,"app_id":"dev.mixos.example","pid":std::process::id(),
            "output":"o_main","workspace":2,"x":1930.0,"y":20.0,"width":920.0,"height":620.0,
            "window_x":1940.0,"window_y":30.0,"window_width":900.0,"window_height":600.0})
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    #[test]
    fn comp_restore_bodies_are_fenced_ordered_and_never_follow() {
        for maximised in [false, true] {
            let mut frame = frame();
            frame.maximised = maximised;
            let mut calls = Vec::new();
            runtime().block_on(frame.restore("dev.mixos.example", |command, args| {
                calls.push((command, args));
                std::future::ready(Some(if command == "comp.windows.list" {
                    json!({"windows":[window()]})
                } else {
                    json!({})
                }))
            }));
            assert_eq!(
                calls.iter().map(|c| c.0).collect::<Vec<_>>(),
                [
                    "comp.windows.list",
                    "comp.window.wait",
                    "comp.windows.list",
                    "comp.window.send_to_workspace",
                    if maximised {
                        "comp.window.maximize"
                    } else {
                        "comp.window.place"
                    }
                ]
            );
            assert_eq!(
                calls[3].1,
                json!({"id":7,"generation":3,"index":2,"follow":false})
            );
            if maximised {
                assert_eq!(calls[4].1, json!({"id":7,"generation":3}));
            } else {
                assert_eq!(
                    calls[4].1,
                    json!({"id":7,"generation":3,"output":"o_main","x":20.0,"y":30.0,"width":900,"height":600})
                );
            }
        }
    }
    #[test]
    fn comp_refusals_stop_without_focusing_or_switching() {
        for refused in 0..5 {
            for _reason in [
                "invalid_state",
                "off_output",
                "unknown_output",
                "missing_workspace",
            ] {
                let mut count = 0;
                runtime().block_on(frame().restore("dev.mixos.example", |command, _| {
                    let at = count;
                    count += 1;
                    std::future::ready(if at == refused {
                        None
                    } else {
                        Some(if command == "comp.windows.list" {
                            json!({"windows":[window()]})
                        } else {
                            json!({})
                        })
                    })
                }));
                assert_eq!(count, refused + 1);
            }
        }
    }
    #[test]
    fn no_comp_or_no_saved_placement_makes_no_mutating_calls() {
        let mut calls = Vec::new();
        runtime().block_on(frame().restore("dev.mixos.example", |command, _| {
            calls.push(command);
            std::future::ready(None)
        }));
        assert_eq!(calls, ["comp.windows.list"]);
        calls.clear();
        runtime().block_on(Frame::default().restore("dev.mixos.example", |command, _| {
            calls.push(command);
            std::future::ready(None)
        }));
        assert!(calls.is_empty());
    }
    #[test]
    fn placement_refusals_restore_the_original_workspace_without_following() {
        for reason in [
            "invalid_state",
            "off_output",
            "unknown_output",
            "missing_workspace",
        ] {
            let mut workspace = 3;
            runtime().block_on(frame().restore("dev.mixos.example", |command, args| {
                let answer = if command == "comp.windows.list" {
                    let mut w = window();
                    w["workspace"] = json!(3);
                    Some(json!({"windows":[w]}))
                } else if command == "comp.window.send_to_workspace" {
                    assert_eq!(args["follow"], false);
                    workspace = args["index"].as_u64().unwrap();
                    Some(json!({}))
                } else if command == "comp.window.place" {
                    let _ = reason;
                    None
                } else {
                    Some(json!({}))
                };
                std::future::ready(answer)
            }));
            assert_eq!(workspace, 3, "{reason}");
        }
    }
    #[test]
    fn all_theme_axes_and_frame_round_trip_and_float_comp_dimensions_capture() {
        let choice = crate::theme::Choice {
            scheme: Some(design::Scheme::Forest),
            style: Some(None),
            mode: Some(design::Mode::Dark),
            decorations: Some(design::RequestedDecorations::Auto),
            captions: Some(design::CaptionSide::Left),
        };
        let mut f = frame();
        f.maximised = true;
        f.theme = choice.into();
        f.capture_comp(&window(), &json!({"x":1920,"y":0}));
        let raw = config::to_conf_mix_string(&f).unwrap();
        let back: Frame = config::from_conf_mix_str(&raw).unwrap();
        assert_eq!(back, f);
        assert_eq!(back.theme.choice(), choice);
        assert_eq!(
            ThemeChoice::default().choice(),
            crate::theme::Choice::default()
        );
        assert_eq!(back.placement.unwrap().width, 900);
    }
    #[test]
    fn exit_reads_geometry_in_output_local_coordinates() {
        let mut f = Frame::default(); let mut calls = Vec::new();
        runtime().block_on(f.capture_exit("dev.mixos.example", |command, args| {
            calls.push((command, args));
            std::future::ready(Some(if command == "comp.windows.list" { json!({"windows":[window()]}) }
                else { json!({"x":1920,"y":0}) }))
        }));
        assert_eq!(calls[1], ("comp.props.get", json!({"path":"outputs.o_main"})));
        let p = f.placement.unwrap(); assert_eq!((p.x, p.y, p.width, p.height), (20.0, 30.0, 900, 600));
    }
}
