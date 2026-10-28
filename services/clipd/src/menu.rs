// SPDX-License-Identifier: MIT OR Apache-2.0
//! Thin history picker. Clipboard changes use the same Bus commands as scripts.
use std::sync::mpsc;
use anyhow::{Result, anyhow};
use bus::{PortReply, native_client::NodedClient};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use crate::model::{HistoryEntry, Pick};

#[derive(Default, Serialize, Deserialize)]
struct State {
    rows: Vec<HistoryEntry>,
    selected: Option<usize>,
    error: Option<String>,
    ready: bool,
    pending: Option<Pick>,
}
struct Picker {
    state: State,
    received: mpsc::Receiver<Result<Value, String>>,
    send: tokio::sync::mpsc::Sender<Pick>,
    strings: toolkit::Strings,
    registry: action::Registry<State>,
}
impl eframe::App for Picker {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.draw(ui);
    }
}

fn registry() -> action::Registry<State> {
    let mut registry = action::Registry::new();
    registry.add(action::Action {
        id: "clipboard.pick", label: "pick", menu: Some("clipboard"), submenu: None,
        group: 0, shortcut: None, icon: None, enabled: action::always,
        params: Some(action::ParamSpec { type_name: "clipd::model::Pick", schema: || schemars::schema_for!(Pick) }),
        class: action::Class::Intent,
        run: |state: &mut State, params| {
            let pick: Pick = serde_json::from_value(params.clone()).map_err(|e| action::Error::BadParams { id: "clipboard.pick", msg: e.to_string() })?;
            if pick.n >= state.rows.len() { return Err(action::Error::Failed("HISTORY_UNAVAILABLE".into())); }
            state.selected = Some(pick.n);
            state.pending = Some(pick);
            Ok(json!({"queued": true}))
        },
    });
    registry
}

impl Picker {
    fn draw(&mut self, ui: &mut egui::Ui) {
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
        let mut choose = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (n, entry) in self.state.rows.iter().enumerate() {
                let preview: String = entry.data.chars().take(160).map(|c| if c.is_control() { ' ' } else { c }).collect();
                if ui.selectable_label(self.state.selected == Some(n), preview).on_hover_text(self.strings.get("pick")).clicked() {
                    choose = Some(Pick { n, expected_id: Some(entry.id.clone()) });
                }
            }
        });
        if let Some(pick) = choose {
            if self.registry.execute("clipboard.pick", &mut self.state, json!(pick), action::Origin::Ui).is_err() { self.state.error = Some(self.strings.get("history-changed")); }
        }
        if let Some(pick) = self.state.pending.take() {
            if self.send.try_send(pick).is_err() { self.state.error = Some(self.strings.get("busy")); }
        }
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
                let strings = toolkit::Strings::new(include_str!("../i18n/en/clipd.ftl"));
                let client = match NodedClient::connect_anonymous(&url).await {
                    Ok(client) => client,
                    Err(error) => { let _ = result.send(Err(error.to_string())); ctx.request_repaint(); return; }
                };
                let mut journal = action::Journal::default();
                let mut picked = None;
                let mut command = "clipboard.history";
                let mut args = json!({});
                loop {
                    let reply = match client.call_typed("clipd", command, args).await {
                        Ok(PortReply::Ok { rc: 0, value }) => Ok(value),
                        Ok(_) => Err(strings.get("refused")),
                        Err(error) => Err(error.to_string()),
                    };
                    if reply.is_ok() {
                        if let Some(params) = picked.take() {
                            journal.record("clipboard.pick", action::Origin::Ui, params, json!({"applied": true}));
                            for event in journal.take().0 {
                                let mut frame = bus::BusMessage::new().with_header("command", "clipd.command.executed");
                                frame.body = event.body().to_string();
                                let headers = std::collections::BTreeMap::from([("name".into(), "clipd.command.executed".into())]);
                                let _ = client.send_with_headers("noded", "topic.publish", &headers, &frame.to_string()).await;
                            }
                        }
                    }
                    let _ = result.send(reply);
                    ctx.request_repaint();
                    let Some(pick) = requests.recv().await else { break; };
                    command = "clipboard.pick"; args = json!(pick); picked = Some(args.clone());
                }
                client.close().await;
            });
        });
        Ok(Box::new(Picker { state: State::default(), received, send, strings, registry: registry() }))
    })).map_err(|e| anyhow!(e.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn picker_snapshot() {
        let (_, received) = std::sync::mpsc::channel();
        let (send, _) = tokio::sync::mpsc::channel(1);
        let mut state = super::State { ready: true, ..Default::default() };
        state.rows = vec![super::HistoryEntry { id: "sample".into(), mime: "text/plain".into(), data: "A recent clipboard entry".into() }];
        let mut picker = super::Picker { state, received, send, strings: toolkit::Strings::new(include_str!("../i18n/en/clipd.ftl")), registry: super::registry() };
        let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(520.0, 280.0)).wgpu().build_ui(move |ui| picker.draw(ui));
        toolkit::install(&harness.ctx, &toolkit::Theme::load());
        harness.run();
        harness.snapshot("clipboard_history");
    }
}
