// SPDX-License-Identifier: MIT OR Apache-2.0
//! Compose viewports. Deferred callbacks render secret-free state and queue
//! input; the root shell alone applies input and performs account effects.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use egui::ViewportId;
use toolkit::{Action, Registry, action::Origin};

use crate::APP;
use crate::shell::{Done, Event};
use crate::state::{State, UiState};
use crate::view::UiEvent;

pub(crate) struct Input {
    pub events: Vec<UiEvent>,
    pub request: Option<u64>,
    pub mixed: bool,
}

pub(crate) struct Window {
    pub name: String,
    pub viewport: ViewportId,
    pub view: State,
    pub pending: Vec<Input>,
    pub draft_due: Option<std::time::Instant>,
    pub focused: Option<std::time::Instant>,
    pub had_compose: bool,
    pub closed: bool,
    keymap: toolkit::keymap::Overrides,
    palette: editor::Palette,
}

/// Only draft-local state travels. Account clients, secrets, ticket counters,
/// send snapshots and draft-store operations stay in the one real engine.
pub(crate) fn swap(a: &mut State, b: &mut State) {
    std::mem::swap(&mut a.compose, &mut b.compose);
    std::mem::swap(&mut a.confirm, &mut b.confirm);
    std::mem::swap(&mut a.attachment_chooser, &mut b.attachment_chooser);
    std::mem::swap(&mut a.attachment_notice, &mut b.attachment_notice);
    std::mem::swap(&mut a.identity_editor, &mut b.identity_editor);
    std::mem::swap(&mut a.documents, &mut b.documents);
    std::mem::swap(&mut a.compose_clipboard, &mut b.compose_clipboard);
    std::mem::swap(&mut a.status, &mut b.status);
}

fn context(state: &State) -> State {
    let mut view = State::new(UiState::default(), state.mode, state.selected.clone(),
        state.settings.clone(), crate::secrets::Secrets::default());
    view.ui = state.ui.clone();
    for account in &mut view.accounts {
        if let Some(real) = state.account(&account.entry.id) {
            account.phase = real.phase.clone();
            account.session = real.session.clone();
        }
    }
    view.take_work();
    view
}

#[derive(Default)]
pub(crate) struct Windows {
    pub compose_only: bool,
    pub windows: BTreeMap<String, Arc<Mutex<Window>>>,
    pub root: Option<String>,
    pub owner_closing: bool,
    pub quitting: bool,
    pub keymap: toolkit::keymap::Overrides,
    next: u64,
}

impl Windows {
    pub fn selected(&self, args: &serde_json::Value) -> Option<Arc<Mutex<Window>>> {
        if let Some(name) = args.get("window") {
            return name.as_str().and_then(|n| self.windows.get(n)).cloned();
        }
        self.windows.values().max_by_key(|w| w.lock().unwrap().focused).cloned()
    }

    pub fn adopt(&mut self, state: &mut State, palette: editor::Palette) {
        if state.exiting || (state.compose.is_none() && !state.documents.opening()) { return; }
        if let Some(root) = self.root.as_ref().and_then(|n| self.windows.get(n)) {
            let mut root = root.lock().unwrap();
            if !root.had_compose && root.view.compose.is_none() && !root.view.documents.opening() {
                swap(state, &mut root.view);
                root.had_compose = root.view.compose.is_some();
                return;
            }
        }
        self.insert(state, palette);
    }

    pub fn insert(&mut self, state: &mut State, palette: editor::Palette) {
        self.next += 1;
        let name = format!("compose-{}", self.next);
        let viewport = if self.compose_only && self.root.is_none() {
            self.root = Some(name.clone());
            ViewportId::ROOT
        } else { ViewportId::from_hash_of((APP, &name)) };
        let mut view = context(state);
        swap(state, &mut view);
        let had_compose = view.compose.is_some();
        self.windows.insert(name.clone(), Arc::new(Mutex::new(Window {
            name, viewport, view, pending: Vec::new(), draft_due: None,
            focused: Some(std::time::Instant::now()), palette,
            had_compose, closed: false,
            keymap: self.keymap.clone(),
        })));
    }

    pub fn target(&self, event: &Event) -> Option<Arc<Mutex<Window>>> {
        self.windows.values().find(|window| {
            let window = window.lock().unwrap();
            if let Event::MailDocument(ticket, _) = event {
                return window.view.documents.owns(*ticket);
            }
            let Some(compose) = &window.view.compose else { return false; };
            match event {
                Event::ComposeAttachments(inspection, _) => compose.draft_id == inspection.draft_id,
                Event::Account(tag, Done::Sent(ticket, ..)) =>
                    compose.account == tag.account && compose.sending == Some(*ticket),
                Event::Account(_, Done::Suggested(ticket, _)) | Event::SuggestDue(ticket) =>
                    compose.suggest.as_ref().is_some_and(|s| s.ticket == *ticket),
                _ => false,
            }
        }).cloned()
    }

    pub fn refresh(&mut self, ctx: &egui::Context, state: &State, palette: &editor::Palette) {
        let children = self.windows.len() > usize::from(self.root.is_some());
        self.windows.retain(|name, window| {
            let mut window = window.lock().unwrap();
            window.had_compose |= window.view.compose.is_some();
            let owner_held = window.viewport == ViewportId::ROOT && children;
            if !owner_held && (window.closed || (window.had_compose && window.view.compose.is_none() && !window.view.documents.opening())) {
                toolkit::saved::secondary::finish(ctx, APP, name);
                toolkit::drive::unregister_viewport(ctx, ViewportId::ROOT, name, window.viewport);
                // No native surface ever changes identity or adopts another draft.
                return false;
            }
            window.view.refresh_compose_view(state);
            window.palette = palette.clone();
            window.keymap = self.keymap.clone();
            true
        });
    }

    /// Root Close is deferred while children exist. The owner remains visible
    /// and keeps its identity; it never adopts a child's name or draft.
    pub fn hold_owner(&mut self, viewport: ViewportId, state: &mut State) -> bool {
        let children = self.windows.len() > usize::from(self.root.is_some());
        if viewport == ViewportId::ROOT && children && !self.quitting {
            self.owner_closing = true;
            state.status = crate::label("compose-owner-wait");
            true
        } else { false }
    }

    pub fn show(&self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        for window in self.windows.values() {
            let (name, viewport) = { let w = window.lock().unwrap(); (w.name.clone(), w.viewport) };
            if viewport == ViewportId::ROOT { window.lock().unwrap().show(ui); continue; }
            let builder = toolkit::titlebar::viewport(crate::APP_ID, &crate::label("compose-title"))
                .with_title(format!("{} — {}", crate::label("compose-title"), name))
                .with_decorations(toolkit::chrome::Chrome::of(&ctx).decorations == design::Decorations::Server)
                .with_inner_size([900.0,700.0]).with_min_inner_size([640.0,480.0]);
            let builder = toolkit::saved::secondary::builder(&ctx, APP, &name, builder);
            let weak = Arc::downgrade(window);
            ctx.show_viewport_deferred(viewport, builder, move |ui, _| {
                if let Some(window) = weak.upgrade() { window.lock().unwrap().show(ui); }
            });
        }
    }
}

impl Window {
    fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.had_compose && self.view.compose.is_none() && !self.view.documents.opening() {
            self.view.status = crate::label("compose-owner-wait");
        }
        if self.viewport == ViewportId::ROOT {
            let _ = toolkit::saved::secondary::builder(&ctx, APP, &self.name, egui::ViewportBuilder::default());
        }
        toolkit::drive::register_viewport(&ctx, ViewportId::ROOT, &self.name, self.viewport);
        toolkit::saved::secondary::capture(&ctx, APP, &self.name);
        if ctx.input(|i| i.events.contains(&egui::Event::WindowFocused(true))) {
            self.focused = Some(std::time::Instant::now());
        }
        if let Some(kind) = self.view.compose_clipboard.take() {
            if let Some(id) = crate::view::compose::body_id(&ctx) {
                ctx.memory_mut(|m| m.request_focus(id));
            }
            match kind {
                editor::Clipboard::Copy => ctx.input_mut(|i| i.events.push(egui::Event::Copy)),
                editor::Clipboard::Cut => ctx.input_mut(|i| i.events.push(egui::Event::Cut)),
                editor::Clipboard::Paste => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            }
        }
        let mut commands = registry();
        commands.rebind(&self.keymap);
        let mut events = view(ui, &self.view, &commands, &self.palette);
        if let Some(outcome) = self.view.documents.chooser.as_mut().and_then(|c| c.show(&ctx)) {
            self.view.documents.last_directory = self.view.documents.chooser.as_ref().map(|c| c.directory().to_owned());
            self.view.documents.chooser = None;
            events.push(UiEvent::DocumentChosen(outcome));
        }
        if let Some(outcome) = self.view.attachment_chooser.as_mut().and_then(|c| c.show(&ctx)) {
            self.view.attachment_chooser = None;
            if let toolkit::file_chooser::Outcome::Picked(paths) = outcome {
                events.push(UiEvent::ComposeAttachmentsPicked(paths));
            }
        }
        let dropped = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect::<Vec<_>>());
        if !dropped.is_empty() { events.push(UiEvent::ComposeDrop(dropped)); }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            events.push(UiEvent::CloseCompose);
        }
        if !events.is_empty() {
            self.pending.push(Input { events, request: toolkit::drive::input_request(&ctx), mixed: toolkit::drive::input_mixed(&ctx) });
            ctx.request_repaint_of(ViewportId::ROOT);
        }
    }
}

/// Draw the same compose chrome and form in native and headless viewports.
pub fn view(ui: &mut egui::Ui, state: &State, registry: &Registry<State>, palette: &editor::Palette) -> Vec<UiEvent> {
    let ctx = ui.ctx().clone();
    let mut events: Vec<_> = toolkit::titlebar::show_bar(ui, &crate::label("compose-title"),
        Some(toolkit::Icon::SquarePen), registry, state, &crate::strings(),
        &[toolkit::titlebar::Action::new("mail.send",toolkit::Icon::Send)])
        .into_iter().map(|(id,o)| UiEvent::Command(id,o)).collect();
    events.extend(crate::actions::shortcuts(&ctx,state,registry).into_iter().map(|id| UiEvent::Command(id,Origin::Shortcut)));
    egui::CentralPanel::default().show(ui, |ui| {
        if state.status == crate::label("compose-owner-wait") { ui.label(&state.status); }
        if let Some(compose) = &state.compose {
            crate::view::compose::form(ui,compose,state,palette,&mut events);
        } else {
            ui.label(if state.status.is_empty() { crate::label("no-accounts-body") } else { state.status.clone() });
            if ui.add(toolkit::button::PushButton::secondary(crate::label("open-settings"))).clicked() {
                events.push(UiEvent::Command("file.accounts", Origin::Menu));
            }
        }
    });
    crate::view::compose::confirm(ui,state,&mut events);
    crate::view::compose::document_replace(ui,state,&mut events);
    crate::view::identities::dialog(ui,state,&mut events);
    toolkit::titlebar::edges(ui);
    events
}

pub fn registry() -> Registry<State> {
    let original = crate::actions::registry();
    let mut registry = Registry::new();
    for (id, menu) in [("compose.new","file"),("compose.open","file"),("compose.save_as","file"),("mail.send","file"),("compose.close","file"),
        ("compose.undo","edit"),("compose.redo","edit"),("compose.cut","edit"),("compose.copy","edit"),("compose.paste","edit"),
        ("compose.attach","compose-attachments-menu"),("compose.attachments.clear","compose-attachments-menu"),("view.mode","view"),("file.quit","file")] {
        let a = original.get(id).unwrap();
        registry.add(Action { id:a.id,label:a.label,menu:Some(menu),submenu:None,group:a.group,
            shortcut: if id == "compose.new" { Some(egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::N)) } else { a.shortcut },
            icon:a.icon,enabled:a.enabled,run: if id == "file.quit" { |s| s.exiting = true } else { a.run } });
    }
    let a = original.get("file.accounts").unwrap();
    registry.add(Action {id:a.id,label:a.label,menu:None,submenu:None,group:0,shortcut:None,icon:a.icon,enabled:a.enabled,run:a.run});
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(standalone: bool) -> (State, Windows, toolkit::Theme, egui::Context) {
        let state = crate::state::tests::connected_many(&["a"]);
        let theme = toolkit::Theme::embedded();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        toolkit::install(&ctx, &theme);
        toolkit::drive::install(&ctx, APP);
        toolkit::saved::Store::<serde_json::Value>::load(None, 0).install_windows(&ctx, APP);
        (state, Windows { compose_only: standalone, ..Default::default() }, theme, ctx)
    }

    fn frame(ctx: &egui::Context, windows: &Windows) {
        let mut input = egui::RawInput::default();
        for window in windows.windows.values() {
            input.viewports.insert(window.lock().unwrap().viewport, Default::default());
        }
        let output = ctx.run_ui(input.clone(), |ui| windows.show(ui));
        for (viewport, output) in &output.viewport_output {
            if *viewport == ViewportId::ROOT { continue; }
            input.viewport_id = *viewport;
            let callback = output.viewport_ui_cb.as_ref().unwrap();
            ctx.run_ui(input.clone(), |ui| callback(ui)).drop_without_applying_deltas();
        }
        output.drop_without_applying_deltas();
    }

    #[test]
    fn two_composers_keep_documents_attachments_and_completions_independent() {
        let (mut state, mut windows, theme, ctx) = fixture(false);
        for subject in ["first", "second"] {
            state.compose();
            state.apply_ui(UiEvent::ComposeField(crate::state::ComposeField::Subject, subject.into()));
            windows.adopt(&mut state, editor::Palette::from_theme(&theme));
        }
        let first = windows.windows["compose-1"].clone();
        let second = windows.windows["compose-2"].clone();
        assert_ne!(first.lock().unwrap().view.compose.as_ref().unwrap().draft_id,
            second.lock().unwrap().view.compose.as_ref().unwrap().draft_id);
        {
            let mut w = second.lock().unwrap();
            swap(&mut state, &mut w.view);
            state.read_document("a".into(), "/home/user/message.md".into(), None, false, false).unwrap();
            let ticket = state.take_work().into_iter().find_map(|w| match w {
                crate::work::Work::MailDocument(crate::documents::DocumentWork::Read {ticket,..}) => Some(ticket), _ => None,
            }).unwrap();
            state.compose.as_mut().unwrap().sending = Some(123);
            swap(&mut state, &mut w.view);
            drop(w);
            assert!(Arc::ptr_eq(&windows.target(&Event::MailDocument(ticket, crate::documents::Answer::Read(
                Err((crate::state::code::DOCUMENT_PATH, "test".into()))))).unwrap(), &second));
        }
        assert!(Arc::ptr_eq(&windows.target(&Event::Account(crate::state::Tag {account:"a".into(),generation:1},
            Done::Sent(123, Default::default(), Err(jmap::Error::Protocol("test".into()))))).unwrap(), &second));
        {
            let mut w = first.lock().unwrap();
            swap(&mut state, &mut w.view);
            state.apply_ui(UiEvent::CloseCompose);
            assert!(state.confirm.is_some());
            state.apply_ui(UiEvent::ConfirmDraft(crate::state::DraftChoice::Keep));
            swap(&mut state, &mut w.view);
        }
        windows.refresh(&ctx, &state, &editor::Palette::from_theme(&theme));
        assert_eq!(windows.windows.len(), 1);
        assert_eq!(second.lock().unwrap().view.compose.as_ref().unwrap().draft.subject,"second");
        assert!(second.lock().unwrap().view.documents.opening());
    }

    #[test]
    fn fixed_root_owner_is_never_promoted_or_renamed() {
        let (mut state, mut windows, theme, ctx) = fixture(true);
        for _ in 0..2 { state.compose(); windows.adopt(&mut state, editor::Palette::from_theme(&theme)); }
        assert!(windows.hold_owner(ViewportId::ROOT, &mut state));
        assert!(windows.owner_closing);
        assert_eq!(windows.root.as_deref(), Some("compose-1"));
        assert_eq!(windows.windows["compose-2"].lock().unwrap().viewport, ViewportId::from_hash_of((APP,"compose-2")));
        windows.windows["compose-1"].lock().unwrap().view.compose = None; // successful send
        windows.refresh(&ctx,&state,&editor::Palette::from_theme(&theme));
        assert_eq!(windows.windows.len(),2,"the empty owner still hosts its own native surface");
        windows.windows["compose-2"].lock().unwrap().closed = true;
        windows.refresh(&ctx,&state,&editor::Palette::from_theme(&theme));
        assert_eq!(windows.windows.len(),1);
        windows.refresh(&ctx,&state,&editor::Palette::from_theme(&theme));
        assert!(windows.windows.is_empty());
        assert_eq!(windows.root.as_deref(),Some("compose-1"));
    }

    #[test]
    fn child_tree_menus_and_close_are_scoped_to_the_target_viewport() {
        let (mut state, mut windows, theme, ctx) = fixture(false);
        for _ in 0..2 { state.compose(); windows.adopt(&mut state, editor::Palette::from_theme(&theme)); }
        frame(&ctx,&windows);
        for (id,command,args) in [
            (1,"ui.tree",serde_json::json!({"viewport":"compose-1"})),
            (2,"ui.menu.list",serde_json::json!({"viewport":"compose-2"})),
            (3,"ui.menu.open",serde_json::json!({"viewport":"compose-1","menu":"Attachments"})),
            (4,"ui.menu",serde_json::json!({"viewport":"compose-2"})),
        ] {
            assert!(toolkit::drive::request(&ctx,id,command,&args).is_none());
            frame(&ctx,&windows);
            let answers = toolkit::drive::logic(&ctx);
            assert_eq!(answers.len(),1);
            assert_eq!(answers[0].rc,0,"{:?}",answers[0].body);
            match id {
                1 => assert!(answers[0].body.to_string().contains("Subject")),
                2 => assert_eq!(answers[0].body["menus"].as_array().unwrap().iter().map(|m| m["title"].as_str().unwrap()).collect::<Vec<_>>(), ["File","Edit","Attachments","View"]),
                3 => assert!(answers[0].body.to_string().contains("Attachments")),
                4 => assert!(answers[0].body["menu"].is_null(),"a sibling menu stays closed"),
                _ => unreachable!(),
            }
        }
        for w in windows.windows.values() { w.lock().unwrap().pending.clear(); }
        let first = windows.windows["compose-1"].clone();
        let viewport = first.lock().unwrap().viewport;
        let mut input = egui::RawInput {viewport_id:viewport,..Default::default()};
        input.viewports.entry(viewport).or_default().events.push(egui::ViewportEvent::Close);
        ctx.run_ui(input,|ui| first.lock().unwrap().show(ui)).drop_without_applying_deltas();
        assert!(first.lock().unwrap().pending.iter().any(|i| i.events.contains(&UiEvent::CloseCompose)));
        assert!(windows.windows["compose-2"].lock().unwrap().pending.is_empty());
    }

    #[test]
    fn root_composer_has_named_drive_menus_and_tree() {
        let (mut state,mut windows,theme,ctx) = fixture(true);
        state.compose(); windows.adopt(&mut state,editor::Palette::from_theme(&theme));
        frame(&ctx,&windows);
        for command in ["ui.tree","ui.menu.list"] {
            let answer = toolkit::drive::request(&ctx,1,command,&serde_json::json!({"viewport":"compose-1"})).unwrap();
            assert_eq!(answer.rc,0);
            assert!(answer.body.to_string().contains(if command == "ui.tree" {"Subject"} else {"Save As"}));
        }
    }

    #[test]
    fn compose_window_snapshot() {
        let (mut state,_,theme,_) = fixture(true);
        state.compose();
        let palette = editor::Palette::from_theme(&theme);
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(900.0,700.0))
            .build_ui(move |ui| { view(ui,&state,&registry(),&palette); });
        toolkit::install(&h.ctx,&theme);
        h.run(); h.snapshot("compose_window");
    }
}
