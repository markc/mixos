// SPDX-License-Identifier: MIT OR Apache-2.0
//! One hidden eframe owner and one native secondary viewport per request.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};

use dopus_core::chooser::{OpenRequest, Presenter, Unavailable};
use serde_json::{Value, json};
use toolkit::drive;

use super::{Presentation, window::Window};

type Windows = Arc<Mutex<BTreeMap<String, Arc<Mutex<Window>>>>>;

pub(super) struct DriveRequest {
    pub id: u64,
    pub command: String,
    pub args: Value,
}

pub(super) struct DriveHost {
    pub ctx: egui::Context,
    pub requests: mpsc::Sender<DriveRequest>,
    pub answers: tokio::sync::mpsc::UnboundedReceiver<Report>,
    windows: Windows,
}

impl DriveHost {
    pub fn target(&self, args: Value) -> Result<Value, &'static str> {
        target(&self.windows, args)
    }

    pub fn allow_map(&self, id: &str) {
        if let Some(window) = self.windows.lock().unwrap().get(id) {
            window.lock().unwrap().map_allowed = true;
            self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

pub(super) enum Report {
    Drive(drive::Answer),
    Executed(Vec<Value>),
}

struct NativePresenter {
    ctx: egui::Context,
    windows: Windows,
}

fn viewport(id: &str) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("files", id))
}

impl Presenter for NativePresenter {
    fn refused(&mut self, id: &str, refusal: &dopus_core::chooser::Refusal) {
        if let Some(window) = self.windows.lock().unwrap().get(id) {
            window.lock().unwrap().refused(&refusal.message);
            self.ctx.request_repaint_of(viewport(id));
        }
    }

    fn mapped(&self, id: &str) -> bool {
        self.windows
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|window| window.lock().unwrap().mapped)
    }

    fn open(&mut self, open: &OpenRequest) -> Result<(), Unavailable> {
        let mut window = Window::new(open);
        window
            .wake_events(&self.ctx)
            .map_err(|e| Unavailable(e.to_string()))?;
        let id = open.id.clone();
        self.windows
            .lock()
            .unwrap()
            .insert(id.clone(), Arc::new(Mutex::new(window)));
        drive::register_viewport(&self.ctx, egui::ViewportId::ROOT, &id, viewport(&id));
        self.ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::RequestPaintWhileHidden,
        );
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        Ok(())
    }

    fn input_ready(&mut self, id: &str) {
        if let Some(window) = self.windows.lock().unwrap().get(id) {
            window.lock().unwrap().ready = true;
            self.ctx.request_repaint_of(viewport(id));
            self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    fn close(&mut self, id: &str) {
        if self.windows.lock().unwrap().remove(id).is_some() {
            self.ctx
                .send_viewport_cmd_to(viewport(id), egui::ViewportCommand::Close);
            drive::unregister_viewport(&self.ctx, egui::ViewportId::ROOT, id, viewport(id));
            self.ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::RequestPaintWhileHidden,
            );
            self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

struct Shell {
    ctx: egui::Context,
    windows: Windows,
    events: tokio::sync::mpsc::UnboundedSender<Presentation>,
    requests: mpsc::Receiver<DriveRequest>,
    answers: tokio::sync::mpsc::UnboundedSender<Report>,
    states: BTreeMap<u64, super::window::State>,
}

impl Shell {
    fn answer(&mut self, mut answer: drive::Answer) {
        if let Some(state) = self.states.remove(&answer.id)
            && answer.rc == 0
        {
            answer.body["chooser"] = json!(state);
        }
        let _ = self.answers.send(Report::Drive(answer));
    }
}

/// A missing viewport is only unambiguous with exactly one open chooser.
fn target(windows: &Windows, mut args: Value) -> Result<Value, &'static str> {
    let Some(object) = args.as_object_mut() else {
        return Err("expected an argument object");
    };
    if !object.contains_key("viewport") {
        let windows = windows.lock().unwrap();
        match windows.len() {
            1 => {
                object.insert("viewport".into(), json!(windows.keys().next().unwrap()));
            }
            _ => return Err("viewport is required unless exactly one chooser is open"),
        }
    }
    if !object["viewport"]
        .as_str()
        .is_some_and(|id| windows.lock().unwrap().contains_key(id))
    {
        return Err("viewport is not an open chooser");
    }
    Ok(args)
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        for answer in drive::logic(ctx) {
            self.answer(answer);
        }
        while let Ok(request) = self.requests.try_recv() {
            // Already bound at Bus arrival. Revalidate only that it is still open.
            let answer = match request
                .args
                .get("viewport")
                .filter(|v| v.is_string())
                .ok_or("viewport is required")
                .and_then(|_| target(&self.windows, request.args.clone()))
            {
                Ok(args) => {
                    if request.command == "window.state"
                        && let Some(id) = args["viewport"].as_str()
                        && let Some(window) = self.windows.lock().unwrap().get(id)
                    {
                        self.states
                            .insert(request.id, window.lock().unwrap().state());
                    }
                    drive::request(ctx, request.id, &request.command, &args)
                }
                Err(message) => Some(drive::Answer {
                    id: request.id,
                    rc: 10,
                    body: json!({"error_code":"ARGUMENT","message":message}),
                }),
            };
            if let Some(answer) = answer {
                self.answer(answer);
            }
        }
        let windows: Vec<_> = self.windows.lock().unwrap().values().cloned().collect();
        for window in windows {
            let mut window = window.lock().unwrap();
            window.pump();
            if let Some(end) = window.take_end() {
                let _ = self.events.send(Presentation {
                    request: window.id.clone(),
                    end,
                });
            }
        }
    }

    fn ui(&mut self, _ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let windows: Vec<_> = self.windows.lock().unwrap().values().cloned().collect();
        for window in windows {
            let (id, title) = {
                let window = window.lock().unwrap();
                if !window.map_allowed {
                    continue;
                }
                (
                    window.id.clone(),
                    dopus_core::sanitise_display_text(&window.request.title),
                )
            };
            // Request windows intentionally have no saved geometry: the contract
            // starts every one centred on its parent's output or the focused one.
            let builder =
                toolkit::titlebar::viewport(&format!("dev.mixos.dopus.choose.{id}"), &title)
                    .with_inner_size([820.0, 600.0])
                    .with_min_inner_size([520.0, 360.0]);
            let events = self.events.clone();
            let root = self.ctx.clone();
            let reports = self.answers.clone();
            let weak = Arc::downgrade(&window);
            self.ctx
                .show_viewport_deferred(viewport(&id), builder, move |ui, _| {
                    if let Some(window) = weak.upgrade() {
                        let mut window = window.lock().unwrap();
                        window.mapped = ui.ctx().input(|i| i.viewport().inner_rect.is_some());
                        window.show(ui);
                        if !window.executed.is_empty() {
                            let _ = reports
                                .send(Report::Executed(std::mem::take(&mut window.executed)));
                        }
                        if let Some(end) = window.take_end() {
                            let _ = events.send(Presentation {
                                request: window.id.clone(),
                                end,
                            });
                            root.request_repaint_of(egui::ViewportId::ROOT);
                        }
                    }
                });
        }
    }

    fn on_exit(&mut self) {
        for answer in drive::finish(&self.ctx) {
            self.answer(answer);
        }
    }
}

pub(super) fn run(url: &str) -> Result<(), String> {
    let _ = dopus_core::DopusCore::chooser_mime();
    let url = url.to_owned();
    let theme = toolkit::Theme::load();
    let result = Arc::new(Mutex::new(None));
    let status = result.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_visible(false)
            .with_app_id("dev.mixos.dopus.files"),
        ..Default::default()
    };
    eframe::run_native(
        "dev.mixos.dopus.files",
        options,
        Box::new(move |cc| {
            toolkit::decorations::initialise(cc);
            toolkit::install(&cc.egui_ctx, &theme);
            drive::install(&cc.egui_ctx, "files");
            drive::set_service(&cc.egui_ctx, Some("files"));
            cc.egui_ctx.set_embed_viewports(false);
            let windows = Windows::default();
            let (events, receive_events) = tokio::sync::mpsc::unbounded_channel();
            let (requests, receive_requests) = mpsc::channel();
            let (answers, receive_answers) = tokio::sync::mpsc::unbounded_channel();
            let presenter = NativePresenter {
                ctx: cc.egui_ctx.clone(),
                windows: windows.clone(),
            };
            let driver = DriveHost {
                ctx: cc.egui_ctx.clone(),
                requests,
                answers: receive_answers,
                windows: windows.clone(),
            };
            let ctx = cc.egui_ctx.clone();
            std::thread::Builder::new()
                .name("files-bus".into())
                .spawn(move || {
                    *status.lock().unwrap() = Some(super::run_host(
                        &url,
                        presenter,
                        receive_events,
                        Some(driver),
                    ));
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                })?;
            Ok(Box::new(Shell {
                ctx: cc.egui_ctx.clone(),
                windows,
                events,
                requests: receive_requests,
                answers,
                states: BTreeMap::new(),
            }))
        }),
    )
    .map_err(|e| e.to_string())?;
    result.lock().unwrap().take().unwrap_or(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewport_selection_never_falls_through_to_the_hidden_owner() {
        let windows = Windows::default();
        assert!(target(&windows, json!({})).is_err());
        assert!(target(&windows, json!({"viewport":"bogus"})).is_err());
        assert!(target(&windows, json!([])).is_err());
    }

    #[test]
    fn refused_acceptance_reopens_input_and_can_still_cancel() {
        use dopus_core::chooser::{Acceptance, End, Engine};
        let ctx = egui::Context::default();
        drive::install(&ctx, "files");
        let windows = Windows::default();
        let mut presenter = NativePresenter {
            ctx,
            windows: windows.clone(),
        };
        let mut engine = Engine::new(1, super::super::defaults(), b"/tmp".to_vec()).unwrap();
        let id = engine.choose("{}", "local:a", 1, &mut presenter).unwrap()["request"]
            .as_str()
            .unwrap()
            .to_owned();
        engine.reply_sent(&id, &mut presenter);
        let end = End::Accept(Acceptance {
            paths: vec![],
            folder: None,
            current_filter: None,
            choices: BTreeMap::new(),
        });
        windows.lock().unwrap()[&id].lock().unwrap().finish(end);
        let end = windows.lock().unwrap()[&id]
            .lock()
            .unwrap()
            .take_end()
            .unwrap();
        assert!(
            super::super::end_presentation(
                &mut engine,
                &mut presenter,
                Presentation {
                    request: id.clone(),
                    end
                },
                2
            )
            .is_none()
        );
        let state = windows.lock().unwrap()[&id].lock().unwrap().state();
        assert!(!state.ending);
        assert!(state.message.is_some());
        assert!(state.ready);
        assert!(
            engine
                .end(&id, End::Cancel, 3, &mut presenter)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn never_mapped_presenter_fails_within_five_seconds_and_cleans_up() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let ctx = egui::Context::default();
            drive::install(&ctx, "files");
            let windows = Windows::default();
            let mut presenter = NativePresenter {
                ctx,
                windows: windows.clone(),
            };
            let mut engine =
                dopus_core::chooser::Engine::new(1, super::super::defaults(), b"/tmp".to_vec())
                    .unwrap();
            let id = engine.choose("{}", "local:a", 1, &mut presenter).unwrap()["request"]
                .as_str()
                .unwrap()
                .to_owned();
            engine.reply_sent(&id, &mut presenter);
            let mut mapping = super::super::Mapping::default();
            let before = tokio::time::Instant::now();
            mapping.opened(&id);
            assert!(
                mapping.deadlines[&id]
                    <= before
                        + std::time::Duration::from_secs(5)
                        + std::time::Duration::from_millis(10)
            );
            // Inject the bound's expiry without spending five seconds in a unit test.
            mapping.deadlines.insert(id.clone(), before);
            assert_eq!(mapping.expired().await, [id.clone()]);
            let outcome =
                super::super::fail_unmapped(&mut engine, &mut presenter, &id, 5002).unwrap();
            assert_eq!(outcome.response, 2);
            assert_eq!(outcome.reason, "failed");
            assert!(outcome.message.unwrap().starts_with("UNAVAILABLE:"));
            assert!(windows.lock().unwrap().is_empty());
            assert!(target(&windows, json!({"viewport":id})).is_err());
            assert!(mapping.deadlines.is_empty());
        });
    }

    #[test]
    fn every_engine_end_removes_only_its_own_native_viewport() {
        use dopus_core::chooser::{Acceptance, End, Engine};
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        drive::install(&ctx, "files");
        let windows = Windows::default();
        let mut presenter = NativePresenter {
            ctx,
            windows: windows.clone(),
        };
        let mut engine = Engine::new(1, super::super::defaults(), b"/tmp".to_vec()).unwrap();
        for end in [
            End::Cancel,
            End::Caller,
            End::Failed("surface failure".into()),
            End::Accept(Acceptance {
                paths: vec![dir.path().as_os_str().as_encoded_bytes().to_vec()],
                folder: None,
                current_filter: None,
                choices: BTreeMap::new(),
            }),
        ] {
            let body = json!({"directory":true,"current_folder":dir.path()}).to_string();
            let id = engine.choose(&body, "local:a", 1, &mut presenter).unwrap()["request"]
                .as_str()
                .unwrap()
                .to_owned();
            let other = engine.choose(&body, "local:b", 1, &mut presenter).unwrap()["request"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(!windows.lock().unwrap()[&id].lock().unwrap().ready);
            engine.reply_sent(&id, &mut presenter);
            engine.reply_sent(&other, &mut presenter);
            assert!(target(&windows, json!({})).is_err());
            assert!(engine.end(&id, end, 2, &mut presenter).unwrap().is_some());
            assert!(
                engine
                    .end(&id, End::Cancel, 3, &mut presenter)
                    .unwrap()
                    .is_none()
            );
            assert!(!windows.lock().unwrap().contains_key(&id));
            assert!(target(&windows, json!({"viewport":id})).is_err());
            assert_eq!(target(&windows, json!({})).unwrap()["viewport"], other);
            engine
                .cancel(&json!({"request":other}).to_string(), 3, &mut presenter)
                .unwrap();
            assert!(windows.lock().unwrap().is_empty());
        }
    }
}
