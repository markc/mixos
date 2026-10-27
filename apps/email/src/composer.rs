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
        self.windows.insert(name.clone(), Arc::new(Mutex::new(Window {
            name, viewport, view, pending: Vec::new(), draft_due: None,
            focused: Some(std::time::Instant::now()), palette,
            had_compose: false, closed: false,
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
            let mut view = context(state);
            swap(&mut view, &mut window.view);
            window.view = view;
            window.palette = palette.clone();
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
        let mut events = view(ui, &self.view, &registry(), &self.palette);
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
        } else { ui.label(&state.status); }
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
        registry.add(Action { id:a.id,label:a.label,menu:Some(menu),submenu:None,group:a.group,shortcut:a.shortcut,icon:a.icon,enabled:a.enabled,run:a.run });
    }
    registry
}
