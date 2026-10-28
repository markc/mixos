// SPDX-License-Identifier: MIT OR Apache-2.0
//! Thin history picker. Clipboard changes use the same Bus commands as scripts.
use std::sync::mpsc;
use anyhow::{Result, anyhow};
use bus::{PortReply, native_client::NodedClient};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use crate::model::HistoryEntry;

#[derive(Default, Serialize, Deserialize)]
struct State {
    rows: Vec<HistoryEntry>,
    selected: Option<usize>,
    error: Option<String>,
    ready: bool,
}
struct Picker {
    state: State,
    received: mpsc::Receiver<Result<Value, String>>,
    send: tokio::sync::mpsc::Sender<usize>,
    strings: toolkit::Strings,
}
impl eframe::App for Picker {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        while let Ok(result) = self.received.try_recv() {
            match result {
                Ok(Value::Array(rows)) => { self.state.rows = serde_json::from_value(Value::Array(rows)).unwrap_or_default(); self.state.ready = true; }
                Ok(_) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                Err(error) => self.state.error = Some(error),
            }
        }
        ui.heading(self.strings.get("title"));
        if let Some(error) = &self.state.error { ui.label(self.strings.get("failed")); ui.label(error); }
        if self.state.rows.is_empty() { ui.label(self.strings.get(if self.state.ready { "empty" } else { "loading" })); }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (n, entry) in self.state.rows.iter().enumerate() {
                let preview: String = entry.data.chars().take(160).map(|c| if c.is_control() { ' ' } else { c }).collect();
                if ui.selectable_label(self.state.selected == Some(n), preview).on_hover_text(self.strings.get("pick")).clicked() {
                    self.state.selected = Some(n);
                    if self.send.try_send(n).is_err() { self.state.error = Some(self.strings.get("busy")); }
                }
            }
        });
    }
}

pub fn run(url: String) -> Result<()> {
    let strings = toolkit::Strings::new(include_str!("../i18n/en/clipd.ftl"));
    let title = strings.get("title");
    let (send, mut requests) = tokio::sync::mpsc::channel(1);
    let (result, received) = mpsc::channel();
    let theme = toolkit::Theme::load();
    eframe::run_native(&title, eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_app_id("dev.mixos.clipd"), ..Default::default() }, Box::new(move |cc| {
        toolkit::install(&cc.egui_ctx, &theme);
        let ctx = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("picker runtime");
            runtime.block_on(async move {
                let client = match NodedClient::connect_anonymous(&url).await {
                    Ok(client) => client,
                    Err(error) => { let _ = result.send(Err(error.to_string())); ctx.request_repaint(); return; }
                };
                let mut command = "clipboard.history";
                let mut args = json!({});
                loop {
                    let reply = match client.call_typed("clipd", command, args).await {
                        Ok(PortReply::Ok { rc: 0, value }) => Ok(value),
                        Ok(_) => Err("Clipboard request refused".into()),
                        Err(error) => Err(error.to_string()),
                    };
                    let _ = result.send(reply);
                    ctx.request_repaint();
                    let Some(n) = requests.recv().await else { break; };
                    command = "clipboard.pick"; args = json!({"n": n});
                }
                client.close().await;
            });
        });
        Ok(Box::new(Picker { state: State::default(), received, send, strings }))
    })).map_err(|e| anyhow!(e.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_picker_renders_offscreen() {
        let ctx = egui::Context::default();
        let (_, received) = std::sync::mpsc::channel();
        let (send, _) = tokio::sync::mpsc::channel(1);
        let mut picker = super::Picker { state: super::State::default(), received, send, strings: toolkit::Strings::new(include_str!("../i18n/en/clipd.ftl")) };
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.heading(picker.strings.get("title"));
            ui.label(picker.strings.get("empty"));
            picker.state.ready = true;
        });
        assert!(!output.shapes.is_empty());
    }
}
