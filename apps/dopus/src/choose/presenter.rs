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
}

pub(super) enum Report {
    Drive(drive::Answer),
    Executed(Vec<Value>),
}

struct NativePresenter {
    ctx: egui::Context,
    windows: Windows,
}

fn viewport(id: &str) -> egui::ViewportId { egui::ViewportId::from_hash_of(("files", id)) }

impl Presenter for NativePresenter {
    fn open(&mut self, open: &OpenRequest) -> Result<(), Unavailable> {
        let mut window = Window::new(open);
        window.wake_events(&self.ctx).map_err(|e| Unavailable(e.to_string()))?;
        let id = open.id.clone();
        self.windows.lock().unwrap().insert(id.clone(), Arc::new(Mutex::new(window)));
        drive::register_viewport(&self.ctx, egui::ViewportId::ROOT, &id, viewport(&id));
        self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaintWhileHidden);
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
            self.ctx.send_viewport_cmd_to(viewport(id), egui::ViewportCommand::Close);
            drive::unregister_viewport(&self.ctx, egui::ViewportId::ROOT, id, viewport(id));
            self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaintWhileHidden);
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
}

/// A missing viewport is only unambiguous with exactly one open chooser.
fn target(windows: &Windows, mut args: Value) -> Result<Value, &'static str> {
    let Some(object) = args.as_object_mut() else { return Err("expected an argument object"); };
    if !object.contains_key("viewport") {
        let windows = windows.lock().unwrap();
        match windows.len() {
            1 => { object.insert("viewport".into(), json!(windows.keys().next().unwrap())); }
            _ => return Err("viewport is required unless exactly one chooser is open"),
        }
    }
    Ok(args)
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        for answer in drive::logic(ctx) { let _ = self.answers.send(Report::Drive(answer)); }
        while let Ok(request) = self.requests.try_recv() {
            let answer = match target(&self.windows, request.args) {
                Ok(args) => drive::request(ctx, request.id, &request.command, &args),
                Err(message) => Some(drive::Answer { id: request.id, rc: 10, body: json!({"error_code":"ARGUMENT","message":message}) }),
            };
            if let Some(answer) = answer { let _ = self.answers.send(Report::Drive(answer)); }
        }
        let windows: Vec<_> = self.windows.lock().unwrap().values().cloned().collect();
        for window in windows {
            let mut window = window.lock().unwrap();
            window.pump();
            if let Some(end) = window.take_end() {
                let _ = self.events.send(Presentation { request: window.id.clone(), end });
            }
        }
    }

    fn ui(&mut self, _ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let windows: Vec<_> = self.windows.lock().unwrap().values().cloned().collect();
        for window in windows {
            let (id, title) = {
                let window = window.lock().unwrap();
                (window.id.clone(), window.request.title.clone())
            };
            // Request windows intentionally have no saved geometry: the contract
            // starts every one centred on its parent's output or the focused one.
            let builder = toolkit::titlebar::viewport(&format!("dev.mixos.dopus.choose.{id}"), &title)
                .with_inner_size([820.0, 600.0]).with_min_inner_size([520.0, 360.0]);
            let events = self.events.clone();
            let root = self.ctx.clone();
            let reports = self.answers.clone();
            let weak = Arc::downgrade(&window);
            self.ctx.show_viewport_deferred(viewport(&id), builder, move |ui, _| {
                if let Some(window) = weak.upgrade() {
                    let mut window = window.lock().unwrap();
                    window.show(ui);
                    if !window.executed.is_empty() { let _ = reports.send(Report::Executed(std::mem::take(&mut window.executed))); }
                    if let Some(end) = window.take_end() {
                        let _ = events.send(Presentation { request: window.id.clone(), end });
                        root.request_repaint_of(egui::ViewportId::ROOT);
                    }
                }
            });
        }
    }

    fn on_exit(&mut self) {
        for answer in drive::finish(&self.ctx) { let _ = self.answers.send(Report::Drive(answer)); }
    }
}

pub(super) fn run(url: &str) -> Result<(), String> {
    let url = url.to_owned();
    let theme = toolkit::Theme::load();
    let result = Arc::new(Mutex::new(None));
    let status = result.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_visible(false).with_app_id("dev.mixos.dopus.files"),
        ..Default::default()
    };
    eframe::run_native("dev.mixos.dopus.files", options, Box::new(move |cc| {
        toolkit::decorations::initialise(cc);
        toolkit::install(&cc.egui_ctx, &theme);
        drive::install(&cc.egui_ctx, "files");
        drive::set_service(&cc.egui_ctx, Some("files"));
        cc.egui_ctx.set_embed_viewports(false);
        let windows = Windows::default();
        let (events, receive_events) = tokio::sync::mpsc::unbounded_channel();
        let (requests, receive_requests) = mpsc::channel();
        let (answers, receive_answers) = tokio::sync::mpsc::unbounded_channel();
        let presenter = NativePresenter { ctx: cc.egui_ctx.clone(), windows: windows.clone() };
        let driver = DriveHost { ctx: cc.egui_ctx.clone(), requests, answers: receive_answers };
        let ctx = cc.egui_ctx.clone();
        std::thread::Builder::new().name("files-bus".into()).spawn(move || {
            *status.lock().unwrap() = Some(super::run_host(&url, presenter, receive_events, Some(driver)));
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        })?;
        Ok(Box::new(Shell { ctx: cc.egui_ctx.clone(), windows, events, requests: receive_requests, answers }))
    })).map_err(|e| e.to_string())?;
    result.lock().unwrap().take().unwrap_or(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn viewport_selection_never_falls_through_to_the_hidden_owner() {
        let windows = Windows::default();
        assert!(target(&windows, json!({})).is_err());
        assert_eq!(target(&windows, json!({"viewport":"explicit"})).unwrap()["viewport"], "explicit");
        assert!(target(&windows, json!([])).is_err());
    }
}
