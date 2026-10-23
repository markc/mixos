// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared GUI glue. The headless `macros` library never depends on toolkit.
use crate::macro_panel::{self, Action, Data, Draft, Row};
use crate::strings::{own, own_with};
use ::macros::{Catalogue, Roots, Runner, Status, Tier};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

type Completion = Result<(Catalogue, Option<String>), String>;

/// The standard Help command, labelled and tipped like the other help items.
pub fn command<S>(enabled: fn(&S) -> bool, run: fn(&mut S)) -> crate::Command<S> {
    crate::Command {
        id: "help.macros",
        label: "toolkit-macros",
        menu: Some("help"),
        submenu: None,
        group: 0,
        shortcut: None,
        icon: Some(crate::Icon::ScrollText),
        enabled,
        run,
    }
}

/// One app's catalogue, runner and panel. All filesystem and launcher work
/// runs off the UI thread; `show` only polls completions and draws.
pub struct State {
    pub open: bool,
    pub catalogue: Catalogue,
    pub runner: Runner,
    app: String,
    roots: Option<Roots>,
    pending: Option<mpsc::Receiver<Completion>>,
    draft: Draft,
    error: Option<String>,
    loaded: bool,
}

impl State {
    pub fn new(app: &str) -> Self {
        Self {
            open: false,
            catalogue: Catalogue::default(),
            runner: Runner::default(),
            app: app.into(),
            roots: Roots::resolve(),
            pending: None,
            draft: Draft::default(),
            error: None,
            loaded: false,
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn reload(&mut self) {
        self.work(|_, _| Ok(()));
    }

    fn work(&mut self, task: impl FnOnce(&Roots, &str) -> io::Result<()> + Send + 'static) {
        if self.pending.is_some() {
            return;
        }
        let Some(roots) = self.roots.clone() else {
            self.error = Some(own("macros-no-home"));
            return;
        };
        let app = self.app.clone();
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let error = task(&roots, &app).err().map(|e| e.to_string());
            // A file may have been created even if the editor could not start.
            let result = Catalogue::scan(&roots, &app)
                .map(|catalogue| (catalogue, error))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    match result {
                        Ok((catalogue, error)) => {
                            self.catalogue = catalogue;
                            self.error = error;
                        }
                        Err(err) => self.error = Some(err),
                    }
                    self.pending = None;
                    self.loaded = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some(own("macros-worker-failed"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !self.open {
            return;
        }
        if !self.loaded && self.pending.is_none() {
            self.reload();
        }
        let status = self.runner.status();
        let output = self.runner.output();
        let mut data = Data {
            rows: self
                .catalogue
                .entries
                .iter()
                .map(|e| Row {
                    id: e.id.clone(),
                    title: e.title.clone(),
                    tier: tier_label(e.tier),
                    path: e.path.display().to_string(),
                    shadows: e
                        .shadows
                        .iter()
                        .map(|&t| tier_label(t))
                        .collect::<Vec<_>>()
                        .join(", "),
                })
                .collect(),
            diagnostics: self
                .catalogue
                .diagnostics
                .iter()
                .map(|d| format!("{}: {}", d.path.display(), d.message))
                .collect(),
            busy: self.pending.is_some(),
            running: status == Status::Running,
            status: match status {
                Status::Idle => String::new(),
                Status::Running => own("macros-running"),
                Status::Exited(code) => own_with(
                    "macros-exited",
                    &[(
                        "code",
                        &code.map_or_else(|| own("macros-signal"), |n| n.to_string()),
                    )],
                ),
                Status::FailedToStart(err) => own_with("macros-failed", &[("error", &err)]),
                Status::Failed(err) => own_with("macros-run-failed", &[("error", &err)]),
            },
            output: output.text,
            dropped: output.dropped,
        };
        if let Some(err) = &self.error {
            data.diagnostics.push(err.clone());
        }
        for action in macro_panel::show(ctx, &mut self.open, &data, &mut self.draft) {
            match action {
                Action::Reload => self.reload(),
                Action::New(app) => {
                    self.draft = Draft {
                        target_app: Some(app),
                        ..Draft::default()
                    }
                }
                Action::Create => {
                    let id = self.draft.id.clone();
                    let title = self.draft.title.clone();
                    let tier = user_tier(self.draft.target_app.unwrap_or(true));
                    self.work(move |roots, app| {
                        let path = roots.create(app, tier, &id, &title)?;
                        launch(&path, false)
                    });
                    self.draft.target_app = None;
                }
                Action::OpenFolder(app) => self.work(move |roots, component| {
                    let folder = roots.ensure_user_folder(component, user_tier(app))?;
                    launch(&folder, true)
                }),
                Action::Edit(index) => {
                    let entry = self.catalogue.entries[index].clone();
                    self.work(move |roots, app| {
                        let path = if entry.tier.is_user() {
                            entry.path
                        } else {
                            roots.copy_to_user(app, &entry)?
                        };
                        if let Some(parent) = path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        launch(&path, false)
                    });
                }
                Action::Run(index) => {
                    if let Some(roots) = &self.roots
                        && let Err(err) =
                            self.runner
                                .start(roots, &self.app, &self.catalogue.entries[index])
                    {
                        self.error = Some(err.to_string());
                    }
                }
                Action::Stop => self.runner.stop(),
            }
        }
        if self.runner.status() == Status::Running || self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn user_tier(app: bool) -> Tier {
    if app { Tier::UserApp } else { Tier::UserGlobal }
}
fn tier_label(tier: Tier) -> String {
    own(&format!("macros-{}", tier.as_str()))
}

fn launch(path: &Path, folder: bool) -> io::Result<()> {
    let native = if folder {
        "/opt/mixos/bin/dopus"
    } else {
        "/opt/mixos/bin/ced"
    };
    let binary = if Path::new(native).is_file() {
        native
    } else {
        "xdg-open"
    };
    let mut child = Command::new(binary)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // The launcher is independent; reaping it never delays the panel's worker.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Keep per-window runtime resources in the egui context; the app's serialisable
/// UI state owns the open flag. Returns true when it must be set to false,
/// independently of the guarded toggle command (including same-frame clicks).
pub fn show_app(ctx: &egui::Context, app: &str, open: bool) -> bool {
    let id = egui::Id::new(("shared-macros", app));
    let state: Option<Arc<Mutex<State>>> = ctx.data(|d| d.get_temp(id));
    if !open && state.is_none() {
        return false;
    }
    let state = state.unwrap_or_else(|| {
        let state = Arc::new(Mutex::new(State::new(app)));
        ctx.data_mut(|d| d.insert_temp(id, state.clone()));
        state
    });
    let mut state = state.lock().unwrap();
    state.open = open;
    state.show(ctx);
    state.open != open
}
