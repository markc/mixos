// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only chooser around the file manager's pane, listing and commands.
use std::collections::BTreeMap;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use dopus_core::chooser::{Acceptance, End, Mode, OpenRequest, Request};
use dopus_core::{CoreEvent, DOpusConfig, DopusCore, PaneId};
use toolkit::{Action, Registry, action::Origin};

use crate::{app::App, label, label_with, view::UiEvent};

/// The chooser's serialisable UI state, returned alongside toolkit window state.
#[derive(serde::Serialize)]
pub struct State {
    pub request: String,
    pub mode: Mode,
    pub caller: String,
    pub on_behalf_of: Option<String>,
    pub folder: Option<String>,
    pub selected: Vec<String>,
    pub name: String,
    pub current_filter: Option<dopus_core::chooser::Filter>,
    pub choices: BTreeMap<String, String>,
    pub sort: dopus_core::SortColumn,
    pub ascending: bool,
    pub show_hidden: bool,
    pub replacing: bool,
    pub message: Option<String>,
    pub ready: bool,
    pub ending: bool,
}

pub struct Window {
    pub id: String,
    pub caller: String,
    pub request: Request,
    pub app: App,
    pub ready: bool,
    pub ending: bool,
    pub name: String,
    original_name: (String, Vec<u8>),
    pub choices: BTreeMap<String, String>,
    pub replace: Option<Acceptance>,
    pub message: Option<String>,
    pub executed: Vec<serde_json::Value>,
    events: mpsc::Receiver<CoreEvent>,
    end: Option<End>,
    focus_name: bool,
    pub map_allowed: bool,
    pub mapped: bool,
}

pub fn path(bytes: &[u8]) -> PathBuf {
    std::ffi::OsString::from_vec(bytes.to_vec()).into()
}

impl Window {
    pub fn state(&self) -> State {
        let pane = self.app.core.pane(PaneId::Left);
        State {
            request: self.id.clone(),
            mode: self.request.mode,
            caller: self.caller.clone(),
            on_behalf_of: self.request.on_behalf_of.clone(),
            folder: dopus_core::chooser::file_uri(pane.path.as_os_str().as_bytes()).ok(),
            selected: self
                .app
                .core
                .selected_paths(PaneId::Left)
                .iter()
                .filter_map(|p| dopus_core::chooser::file_uri(p.as_os_str().as_bytes()).ok())
                .collect(),
            name: self.name.clone(),
            current_filter: self.request.current_filter.clone(),
            choices: self.choices.clone(),
            sort: pane.sort,
            ascending: pane.ascending,
            show_hidden: pane.show_hidden,
            replacing: self.replace.is_some(),
            message: self.message.clone(),
            ready: self.ready,
            ending: self.ending,
        }
    }

    pub fn new(open: &OpenRequest) -> Self {
        Self::for_request(&open.id, &open.caller, open.request.clone())
    }

    pub fn for_request(id: &str, caller: &str, request: Request) -> Self {
        let config = crate::dirs::AppDirs::resolve("dopus")
            .map(|dirs| dopus_core::ConfigFile::load(&dirs.config_dir()).0)
            .unwrap_or_default();
        Self::with_config(id, caller, request, config)
    }

    fn with_config(id: &str, caller: &str, request: Request, mut config: DOpusConfig) -> Self {
        config.left.path = request
            .current_folder
            .as_deref()
            .map(path)
            .unwrap_or_else(dopus_core::home_directory);
        config.left.previews = false;
        let (mut core, events) = DopusCore::new_chooser(config);
        core.set_chooser_filter(
            request.current_filter.clone(),
            request.directory || request.mode == Mode::SaveFiles,
        );
        let raw = request.current_name.clone().unwrap_or_default();
        let name = dopus_core::sanitise_display_text(&String::from_utf8_lossy(&raw));
        Self {
            id: id.into(),
            caller: caller.into(),
            choices: request.choice_defaults(),
            request,
            app: App::new(core),
            ready: false,
            ending: false,
            original_name: (name.clone(), raw),
            name,
            replace: None,
            message: None,
            executed: Vec::new(),
            events,
            end: None,
            focus_name: true,
            map_allowed: false,
            mapped: false,
        }
    }

    /// Install an event waker without polling; only the UI thread feeds on_event.
    pub fn wake_events(&mut self, ctx: &egui::Context) -> Result<(), std::io::Error> {
        let (tx, rx) = mpsc::channel();
        let source = std::mem::replace(&mut self.events, rx);
        let ctx = ctx.clone();
        let viewport = egui::ViewportId::from_hash_of(("files", &self.id));
        std::thread::Builder::new()
            .name("files-listing".into())
            .spawn(move || {
                while let Ok(event) = source.recv() {
                    if tx.send(event).is_err() {
                        break;
                    }
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    ctx.request_repaint_of(viewport);
                }
            })?;
        Ok(())
    }

    pub fn pump(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            let derived = self.app.core.on_event(event);
            self.absorb(derived);
        }
        let derived = self.app.core.tick(Instant::now());
        self.absorb(derived);
    }

    fn absorb(&mut self, events: Vec<CoreEvent>) {
        // Chooser commands never request file operations. Still discharge every
        // unexpected reservation, and never hand OpenFile to a desktop handler.
        for event in events {
            match event {
                CoreEvent::ConfirmRequested { token, .. } => {
                    self.app.core.confirm(token, dopus_core::ConfirmAnswer::No)
                }
                CoreEvent::PromptRequested { token, .. } => self.app.core.prompt_text(token, None),
                CoreEvent::OpenFile(_) => {
                    self.finish(End::Failed(label("chooser-operation-refused")))
                }
                _ => {}
            }
        }
    }

    pub fn finish(&mut self, end: End) {
        if self.ready && !self.ending {
            self.ending = true;
            self.end = Some(end);
        }
    }

    pub fn take_end(&mut self) -> Option<End> {
        self.end.take()
    }

    pub fn refused(&mut self, message: &str) {
        self.ending = false;
        self.end = None;
        self.message = Some(dopus_core::sanitise_display_text(message));
    }

    fn navigate(&mut self, target: PathBuf) {
        if target.is_dir() {
            self.app.go(target);
            self.message = None;
        } else {
            self.message = Some(label("chooser-folder-missing"));
        }
    }

    fn selected(&mut self, target: PathBuf, ctrl: bool, shift: bool) {
        self.app.click_row(
            PaneId::Left,
            target.clone(),
            ctrl && self.request.multiple,
            shift && self.request.multiple,
        );
        if self.request.mode == Mode::Save && !target.is_dir() {
            let raw = target.file_name().unwrap_or_default().as_bytes().to_vec();
            self.name = dopus_core::sanitise_display_text(&String::from_utf8_lossy(&raw));
            self.original_name = (self.name.clone(), raw);
        }
    }

    fn apply(&mut self, events: Vec<UiEvent>, registry: &Registry<Self>) {
        for event in events {
            match event {
                UiEvent::Click {
                    path, ctrl, shift, ..
                } => self.selected(path, ctrl, shift),
                UiEvent::OpenFile { path, .. } => {
                    self.selected(path, false, false);
                    self.command("chooser.accept");
                }
                UiEvent::Go(target) => self.navigate(target),
                UiEvent::Command(id, origin) => {
                    let _ = registry.execute(id, self, origin);
                }
                UiEvent::Drop { .. } | UiEvent::CommandIn(..) => {}
                other => {
                    crate::view::apply(&mut self.app, &crate::actions::registry(), vec![other])
                }
            }
        }
    }

    fn command(&mut self, id: &str) {
        match id {
            "chooser.cancel" => self.finish(End::Cancel),
            "chooser.replace" => {
                if let Some(accept) = self.replace.take() {
                    self.finish(End::Accept(accept));
                }
            }
            "chooser.keep" => self.replace = None,
            "chooser.accept" => self.accept(),
            "file.open" => {
                let selected = self.app.core.pane(PaneId::Left).selected.clone();
                if let Some(target) = selected.filter(|p| p.is_dir()) {
                    self.navigate(target);
                } else {
                    self.accept();
                }
            }
            _ => self.app.action(id),
        }
    }

    pub fn accept(&mut self) {
        if !self.ready || self.ending || self.replace.is_some() {
            return;
        }
        if self.request.mode == Mode::Save && self.name.contains('/') {
            let (folder, tail) = self.name.rsplit_once('/').expect("slash present");
            let tail = tail.to_owned();
            let target = PathBuf::from(if folder.is_empty() { "/" } else { folder });
            let target = if target.is_absolute() {
                target
            } else {
                self.app.core.pane(PaneId::Left).path.join(target)
            };
            if target.is_dir() {
                self.navigate(target);
                self.name = tail;
                self.original_name = (self.name.clone(), self.name.as_bytes().to_vec());
            } else {
                self.message = Some(label("chooser-folder-missing"));
            }
            return;
        }
        let folder = self.app.core.pane(PaneId::Left).path.clone();
        let paths = match self.request.mode {
            Mode::Open => {
                let selected = self.app.core.selected_paths(PaneId::Left);
                let selected = if selected.is_empty() && self.request.directory {
                    vec![folder.clone()]
                } else {
                    selected
                };
                if selected.is_empty()
                    || selected.iter().any(|p| {
                        if self.request.directory {
                            !p.is_dir()
                        } else {
                            !p.is_file()
                        }
                    })
                {
                    self.message = Some(label("chooser-select-entry"));
                    return;
                }
                if !self.request.multiple && selected.len() != 1 {
                    self.message = Some(label("chooser-select-entry"));
                    return;
                }
                selected
                    .into_iter()
                    .map(|p| {
                        if self.request.directory
                            && p.components().any(|c| {
                                matches!(
                                    c,
                                    std::path::Component::ParentDir | std::path::Component::CurDir
                                )
                            })
                        {
                            p.canonicalize().unwrap_or(p)
                        } else {
                            p
                        }
                    })
                    .collect()
            }
            Mode::Save => {
                let raw: &[u8] = if self.name == self.original_name.0 {
                    &self.original_name.1
                } else {
                    self.name.as_bytes()
                };
                if dopus_core::chooser::validate_name(raw).is_err() {
                    self.message = Some(label("chooser-invalid-name"));
                    return;
                }
                let name = self.request.save_name(raw);
                if dopus_core::chooser::validate_name(&name).is_err() {
                    self.message = Some(label("chooser-invalid-name"));
                    return;
                }
                vec![folder.join(std::ffi::OsString::from_vec(name))]
            }
            Mode::SaveFiles => self
                .request
                .files
                .iter()
                .map(|name| folder.join(std::ffi::OsString::from_vec(name.clone())))
                .collect(),
        };
        if self.request.mode == Mode::Save && paths[0].is_dir() {
            self.navigate(paths[0].clone());
            return;
        }
        if !folder.is_dir()
            || paths
                .iter()
                .any(|p| dopus_core::chooser::file_uri(p.as_os_str().as_bytes()).is_err())
        {
            self.message = Some(label("chooser-folder-missing"));
            return;
        }
        let accept = Acceptance {
            paths: paths
                .iter()
                .map(|p| p.as_os_str().as_bytes().to_vec())
                .collect(),
            folder: Some(folder.as_os_str().as_bytes().to_vec()),
            current_filter: self.request.current_filter.clone(),
            choices: self.choices.clone(),
        };
        if self.request.validate_acceptance(&accept).is_err() {
            self.message = Some(label("chooser-invalid-selection"));
            return;
        }
        if self.request.mode != Mode::Open && paths.iter().any(|p| p.symlink_metadata().is_ok()) {
            self.replace = Some(accept);
        } else {
            self.finish(End::Accept(accept));
        }
    }

    /// The same view is used by native secondary viewports and snapshot tests.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.push_id(ui.ctx().viewport_id(), |ui| self.show_inner(ui));
    }

    fn show_inner(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let registry = registry();
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            let _ = registry.execute("chooser.cancel", self, Origin::Menu);
        }
        if self.replace.is_none()
            && !egui::Popup::is_any_open(&ctx)
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            let _ = registry.execute("chooser.cancel", self, Origin::Shortcut);
        }
        if self.ready && !self.ending && self.replace.is_none() && !ctx.egui_wants_keyboard_input()
        {
            for id in registry.shortcuts(&ctx, self) {
                let _ = registry.execute(id, self, Origin::Shortcut);
            }
        }
        let mut events = Vec::new();
        let mut clicked = None;
        toolkit::chrome::client_side(&ctx);
        for (id, origin) in toolkit::titlebar::show_bar(
            ui,
            &dopus_core::sanitise_display_text(&self.request.title),
            Some(toolkit::Icon::Folder),
            &registry,
            self,
            &crate::strings(),
            &[],
        ) {
            events.push(UiEvent::Command(id, origin));
        }
        egui::Panel::top("files-identity").show(ui, |ui| {
            if !is_portald(&self.caller) {
                ui.label(label_with("chooser-caller", &[("caller", &self.caller)]));
            }
            if let Some(claim) = &self.request.on_behalf_of {
                ui.weak(label_with(
                    "chooser-claim",
                    &[
                        ("app", &dopus_core::sanitise_display_text(claim)),
                        ("caller", &self.caller),
                    ],
                ));
            }
        });
        egui::Panel::bottom("files-buttons").show(ui, |ui| {
            ui.add_enabled_ui(self.ready && !self.ending && self.replace.is_none(), |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(toolkit::button::PushButton::primary(
                            dopus_core::sanitise_display_text(&self.request.accept_label),
                        ))
                        .clicked()
                    {
                        clicked = Some("chooser.accept");
                    }
                    if ui.button(label("cancel")).clicked() {
                        clicked = Some("chooser.cancel");
                    }
                });
            });
        });
        let max_form = ctx.content_rect().height()
            * toolkit::chrome::Chrome::of(&ctx)
                .style
                .dialog_resize_max_share as f32
            / 2.0;
        let form_rows = usize::from(self.request.mode != Mode::Open)
            + usize::from(self.request.filter_menu)
            + self.request.choices.len()
            + 2 * usize::from(self.message.is_some());
        let row_height = toolkit::field::height_of(&ctx) + ui.spacing().item_spacing.y;
        let form_height =
            (form_rows as f32 * row_height + 2.0 * ui.spacing().item_spacing.y).min(max_form);
        if form_rows > 0 {
            egui::Panel::bottom("files-controls")
                .exact_size(form_height)
                .show(ui, |ui| {
                    ui.add_enabled_ui(self.ready && !self.ending && self.replace.is_none(), |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("files-options")
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                                if self.request.mode == Mode::Save {
                                    ui.horizontal(|ui| {
                                        ui.label(label("chooser-name"));
                                        let field = ui
                                            .add(
                                                egui::TextEdit::singleline(&mut self.name)
                                                    .desired_width(f32::INFINITY),
                                            )
                                            .accessible_name(label("chooser-name"));
                                        if self.focus_name && self.ready && !self.ending {
                                            field.request_focus();
                                            if let Some(mut state) =
                                                egui::TextEdit::load_state(&ctx, field.id)
                                            {
                                                let end = self
                                                    .name
                                                    .rfind('.')
                                                    .filter(|i| *i > 0)
                                                    .map(|i| self.name[..i].chars().count())
                                                    .unwrap_or_else(|| self.name.chars().count());
                                                state.cursor.set_char_range(Some(
                                                    egui::text::CCursorRange::two(
                                                        egui::text::CCursor::new(0),
                                                        egui::text::CCursor::new(end),
                                                    ),
                                                ));
                                                state.store(&ctx, field.id);
                                            }
                                            self.focus_name = false;
                                        }
                                        if field.lost_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                        {
                                            clicked = Some("chooser.accept");
                                        }
                                    });
                                }
                                if self.request.mode == Mode::SaveFiles {
                                    ui.label(label_with(
                                        "chooser-file-names",
                                        &[(
                                            "names",
                                            &self
                                                .request
                                                .files
                                                .iter()
                                                .map(|n| {
                                                    dopus_core::sanitise_display_text(
                                                        &String::from_utf8_lossy(n),
                                                    )
                                                })
                                                .collect::<Vec<_>>()
                                                .join(", "),
                                        )],
                                    ));
                                }
                                if self.request.filter_menu {
                                    let mut current = self.request.current_filter.clone();
                                    egui::ComboBox::from_id_salt("files-filter")
                                        .selected_text(
                                            current
                                                .as_ref()
                                                .map(|f| dopus_core::sanitise_display_text(&f.name))
                                                .unwrap_or_default(),
                                        )
                                        .show_ui(ui, |ui| {
                                            for filter in &self.request.filters {
                                                if toolkit::selectable(
                                                    ui,
                                                    current.as_ref() == Some(filter),
                                                    dopus_core::sanitise_display_text(&filter.name),
                                                )
                                                .clicked()
                                                {
                                                    current = Some(filter.clone());
                                                }
                                            }
                                        })
                                        .response
                                        .accessible_name(label("chooser-filter"));
                                    if current != self.request.current_filter {
                                        self.request.current_filter = current.clone();
                                        self.app
                                            .core
                                            .set_chooser_filter(current, self.request.directory);
                                    }
                                }
                                for choice in &self.request.choices {
                                    let value =
                                        self.choices.get_mut(&choice.id).expect("default choice");
                                    if choice.options.is_empty() {
                                        let mut checked = value == "true";
                                        if ui
                                            .checkbox(
                                                &mut checked,
                                                dopus_core::sanitise_display_text(&choice.label),
                                            )
                                            .changed()
                                        {
                                            *value = checked.to_string();
                                        }
                                    } else {
                                        ui.horizontal(|ui| {
                                            ui.label(dopus_core::sanitise_display_text(
                                                &choice.label,
                                            ));
                                            egui::ComboBox::from_id_salt((
                                                "files-choice",
                                                &choice.id,
                                            ))
                                            .selected_text(
                                                choice
                                                    .options
                                                    .iter()
                                                    .find(|o| &o.id == value)
                                                    .map(|o| {
                                                        dopus_core::sanitise_display_text(&o.label)
                                                    })
                                                    .unwrap_or_default(),
                                            )
                                            .show_ui(ui, |ui| {
                                                for option in &choice.options {
                                                    if toolkit::selectable(
                                                        ui,
                                                        *value == option.id,
                                                        dopus_core::sanitise_display_text(
                                                            &option.label,
                                                        ),
                                                    )
                                                    .clicked()
                                                    {
                                                        *value = option.id.clone();
                                                    }
                                                }
                                            })
                                            .response
                                            .accessible_name(dopus_core::sanitise_display_text(
                                                &choice.label,
                                            ));
                                        });
                                    }
                                }
                                if let Some(message) = &self.message {
                                    ui.label(message);
                                }
                            });
                    });
                });
        }
        let places_width = self
            .app
            .core
            .places()
            .iter()
            .map(|(name, _)| {
                ui.painter()
                    .layout_no_wrap(
                        name.clone(),
                        egui::TextStyle::Body.resolve(ui.style()),
                        ui.visuals().text_color(),
                    )
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
            + 2.0 * ui.spacing().interact_size.x
            + 2.0 * ui.spacing().item_spacing.x;
        egui::Panel::left("files-places")
            .default_size(places_width)
            .show(ui, |ui| {
                ui.heading(label("places"));
                ui.add_enabled_ui(self.ready && !self.ending && self.replace.is_none(), |ui| {
                    crate::view::places_panel(ui, &self.app, &mut events)
                });
            });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_enabled_ui(self.ready && !self.ending && self.replace.is_none(), |ui| {
                crate::view::chooser_pane(ui, &self.app, &crate::actions::registry(), &mut events)
            });
        });
        if let Some(accept) = &self.replace {
            let names = accept
                .paths
                .iter()
                .filter(|p| path(p).symlink_metadata().is_ok())
                .map(|p| dopus_core::sanitise_display_path(&path(p)))
                .collect::<Vec<_>>()
                .join("\n");
            let shown = toolkit::dialog::Dialog::new(
                egui::Id::unique((ctx.viewport_id(), "files-replace")),
                replace_text("chooser-replace-title", collision_count(accept)),
            )
            .buttons(vec![
                toolkit::dialog::Choice::new(
                    label("chooser-replace"),
                    toolkit::dialog::Role::Default,
                ),
                toolkit::dialog::Choice::new(label("cancel"), toolkit::dialog::Role::Cancel),
            ])
            .show(&ctx, |ui| {
                ui.label(replace_text(
                    "chooser-replace-body",
                    collision_count(accept),
                ));
                let max_names = ctx.content_rect().height()
                    * toolkit::chrome::Chrome::of(&ctx)
                        .style
                        .dialog_resize_max_share as f32
                    / 2.0;
                egui::ScrollArea::vertical()
                    .id_salt("files-collisions")
                    .max_height(max_names)
                    .show(ui, |ui| {
                        ui.label(&names);
                    });
            });
            if let Some(answer) = shown.chosen {
                clicked = Some(if answer == 0 {
                    "chooser.replace"
                } else {
                    "chooser.keep"
                });
            }
        }
        self.apply(events, &registry);
        if let Some(id) = clicked {
            let _ = registry.execute(id, self, Origin::Menu);
        }
        // Tick after this frame's mutations (law 1). Raw worker replies are
        // still fed through on_event only by the root event loop (law 2).
        let derived = self.app.core.tick(Instant::now());
        self.absorb(derived);
        let (runs, _) = registry.take_executed();
        self.executed.extend(runs.iter().map(|run| {
            let mut body = toolkit::executed::Executed::body(run);
            body["viewport"] = serde_json::json!(self.id);
            body
        }));
        if let Some(deadline) = self.app.core.next_deadline() {
            ctx.request_repaint_after(deadline.saturating_duration_since(Instant::now()));
        }
    }
}

fn is_portald(caller: &str) -> bool {
    caller == "local:portald"
}

fn collision_count(accept: &Acceptance) -> usize {
    accept
        .paths
        .iter()
        .filter(|p| path(p).symlink_metadata().is_ok())
        .count()
}

fn replace_text(id: &str, count: usize) -> String {
    label_with(id, &[("count", &count.to_string())])
}

fn registry() -> Registry<Window> {
    let mut registry = Registry::new();
    macro_rules! add {
        ($id:literal, $label:literal) => {
            registry.add(Action {
                id: $id,
                label: $label,
                menu: None,
                submenu: None,
                group: 0,
                icon: None,
                shortcut: crate::keymap::defaults_for($id).first().copied(),
                enabled: |w: &Window| {
                    w.ready
                        && !w.ending
                        && (w.replace.is_none()
                            || matches!($id, "chooser.cancel" | "chooser.replace" | "chooser.keep"))
                },
                run: |w: &mut Window| w.command($id),
            });
        };
    }
    add!("chooser.accept", "chooser-open-accept");
    add!("chooser.cancel", "cancel");
    add!("chooser.replace", "chooser-replace");
    add!("chooser.keep", "cancel");
    add!("file.open", "file-open");
    add!("location.focus", "go-location");
    add!("nav.back", "go-back");
    add!("nav.forward", "go-forward");
    add!("nav.parent", "go-parent");
    add!("nav.home", "go-home");
    add!("view.refresh", "view-refresh");
    add!("view.toggle-hidden", "view-hidden");
    add!("selection.next", "select-next");
    add!("selection.previous", "select-previous");
    add!("selection.first", "select-first");
    add!("selection.last", "select-last");
    registry.add_shortcuts(
        "nav.parent",
        crate::keymap::defaults_for("nav.parent")
            .into_iter()
            .skip(1),
    );
    registry
}

#[cfg(test)]
mod tests;
