// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window driven end to end with no Bus: a transport that answers the
//! controller's edit-service calls from the fake edit service, so the real
//! controller, mirrors and editor run in-process. Offscreen snapshots of the
//! window, and typing that reaches the service. Regenerate the images with
//! `UPDATE_SNAPSHOTS=1` and look at them before committing.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use ced::app::App;
use ced::bus::Delivery;
use ced::shell::{Shell, Transport, reply};
use design::{DesignContext, Mode, Scheme};
use documents::config::Config;
use documents::controller::{Controller, Effect};
use documents::session::SessionWriter;
use editor_model::fake::FakeEditd;
use editor_model::types::{Incoming, Intent};
use egui::{Event, Key, Modifiers, PointerButton, pos2};
use egui_kittest::Harness;
use serde_json::{Value, json};
use toolkit::Theme;

const MIX: &str = "\
-- greet.mix: say hello to each name
fn greet($name)
  print(\"hello, \" .. $name)
end

for each $n in [\"ada\", \"grace\", \"linus\"]
  greet($n)
end
";

const NOTES: &str = "\
# Notes

The editor wraps prose at word boundaries, so a paragraph reads as a \
paragraph rather than one long line.
";

/// The fake edit service as a transport. Every request is answered at
/// once, followed by the events it caused, in order. Timers wait on a fake
/// clock until the test fires them.
struct Fake {
    editd: Rc<RefCell<FakeEditd>>,
    queue: Rc<RefCell<VecDeque<Delivery>>>,
    timers: Rc<RefCell<Vec<u64>>>,
}

/// The test's side of the fake: the service, and its clock.
struct Service {
    editd: Rc<RefCell<FakeEditd>>,
    queue: Rc<RefCell<VecDeque<Delivery>>>,
    timers: Rc<RefCell<Vec<u64>>>,
}

impl Service {
    /// Fire every armed timer now.
    fn fire_timers(&self) {
        for id in std::mem::take(&mut *self.timers.borrow_mut()) {
            self.queue
                .borrow_mut()
                .push_back(Delivery::Incoming(Incoming::Timer { id }));
        }
    }
}

impl Transport for Fake {
    fn perform(&mut self, effect: &Effect) {
        if let Effect::Timer { id, .. } = effect {
            self.timers.borrow_mut().push(*id);
        }
        if let Effect::Send { req, out } = effect {
            let args: Value = serde_json::from_str(&out.body).unwrap_or_else(|_| json!({}));
            let mut answer = self
                .editd
                .borrow_mut()
                .handle("local:ced", &out.verb, &args);
            // The fake answers every document as plain text; the real
            // service detects the language from the path and first line.
            if out.verb == "edit.open"
                && answer.rc == 0
                && let Some(path) = answer.body["path"].as_str().map(str::to_owned)
            {
                let bid = answer.body["buffer"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let text = self.editd.borrow().text(&bid);
                let first = text.lines().next().unwrap_or_default();
                answer.body["language"] =
                    json!(edit::lang::detect(Some(std::path::Path::new(&path)), first));
            }
            let mut queue = self.queue.borrow_mut();
            queue.push_back(reply(*req, answer.rc, answer.body.to_string()));
            for event in self.editd.borrow_mut().take_events() {
                queue.push_back(Delivery::Incoming(Incoming::Topic {
                    topic: "edit.changed".into(),
                    body: serde_json::to_string(&event).unwrap(),
                }));
            }
        }
    }
    fn poll(&mut self) -> Option<Delivery> {
        self.queue.borrow_mut().pop_front()
    }
    fn connected(&self) -> bool {
        true
    }
    fn shutdown(&mut self, _session: Option<SessionWriter>) {
        self.queue
            .borrow_mut()
            .push_back(Delivery::Stopped { faults: Vec::new() });
    }
}

/// The window's state until its first frame builds the shell.
struct Holder {
    start: Option<(Theme, Box<dyn Transport>, Vec<String>)>,
    shell: Option<Shell>,
}

fn theme(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

/// A window with `files` (path, text) in the fake service, all opened.
fn window(
    theme: Theme,
    files: &[(&str, &str)],
    size: egui::Vec2,
) -> (Harness<'static, Holder>, Service) {
    let editd = Rc::new(RefCell::new(FakeEditd::new("e1")));
    let paths: Vec<String> = files
        .iter()
        .map(|(path, text)| {
            editd.borrow_mut().create(Some(path), text);
            (*path).to_owned()
        })
        .collect();
    let service = Service {
        editd: editd.clone(),
        queue: Rc::default(),
        timers: Rc::default(),
    };
    let transport = Box::new(Fake {
        editd: editd.clone(),
        queue: service.queue.clone(),
        timers: service.timers.clone(),
    });
    let holder = Holder {
        start: Some((theme.clone(), transport, paths)),
        shell: None,
    };
    let harness = Harness::builder().with_size(size).wgpu().build_ui_state(
        |ui, h: &mut Holder| {
            if let Some((theme, transport, paths)) = h.start.take() {
                let ctl = Controller::new(Config::default(), 1, false);
                let app = App::new(ctl, Config::default());
                let mut shell = Shell::new(ui.ctx().clone(), theme, app, transport, None);
                let fx = shell.app.ctl.open_paths(&paths, Intent::ui(0));
                shell.app.absorb(fx);
                h.shell = Some(shell);
            }
            let shell = h.shell.as_mut().unwrap();
            shell.logic(ui.ctx());
            shell.ui(ui);
        },
        holder,
    );
    toolkit::install(&harness.ctx, &theme);
    (harness, service)
}

#[test]
fn two_documents_studio_dark() {
    let (mut h, service) = window(
        theme(Scheme::Studio, Mode::Dark),
        &[("/work/greet.mix", MIX), ("/work/notes.md", NOTES)],
        egui::vec2(820.0, 460.0),
    );
    h.run_steps(8);
    let app = &h.state().shell.as_ref().unwrap().app;
    assert_eq!(app.ctl.tabs().len(), 2);
    // The Mix document, highlighted by its lexer.
    let first = app.ctl.tabs()[0].id;
    let shell = h.state_mut().shell.as_mut().unwrap();
    let fx = shell.app.ctl.select_tab(first);
    shell.app.absorb(fx);
    assert!(
        lex(&mut h, &service),
        "greet.mix is lexed before the snapshot"
    );
    h.snapshot("two_documents_studio_dark");
}

#[test]
fn an_empty_window_pro_light() {
    let (mut h, _) = window(
        theme(Scheme::Pro, Mode::Light),
        &[],
        egui::vec2(640.0, 360.0),
    );
    h.run_steps(4);
    h.snapshot("empty_pro_light");
}

#[test]
fn typing_reaches_the_edit_service() {
    let (mut h, service) = window(
        theme(Scheme::Studio, Mode::Dark),
        &[("/work/a.txt", "first line\n")],
        egui::vec2(640.0, 360.0),
    );
    h.run_steps(6);
    // Click into the text, then type at the start of the document.
    let at = pos2(320.0, 200.0);
    for pressed in [true, false] {
        let input = h.input_mut();
        input.events.push(Event::PointerMoved(at));
        input.events.push(Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(2);
    }
    h.input_mut().events.push(Event::Key {
        key: Key::Home,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    });
    h.run_steps(2);
    h.input_mut().events.push(Event::Text("> ".into()));
    h.run_steps(4);
    let editd = service.editd.borrow();
    let bid = editd.buffers()[0].clone();
    assert_eq!(editd.text(&bid), "> first line\n");
    let app = &h.state().shell.as_ref().unwrap().app;
    let tab = app.active_tab().unwrap();
    assert_eq!(tab.mirror.as_ref().unwrap().pending(), 0, "acknowledged");
}

/// A Mix document is lexed on a worker thread once its debounce timer
/// fires, and the spans reach the highlighter.
#[test]
fn mix_documents_are_lexed_off_the_ui_thread() {
    let (mut h, service) = window(
        theme(Scheme::Studio, Mode::Dark),
        &[("/work/greet.mix", MIX)],
        egui::vec2(640.0, 360.0),
    );
    h.run_steps(6);
    assert!(
        lex(&mut h, &service),
        "the fn line has spans from the Mix lexer"
    );
}

/// Fire the controller's timers until the active document's third line has
/// spans from the Mix lexer (on its worker thread); whether it did.
fn lex(h: &mut Harness<'static, Holder>, service: &Service) -> bool {
    let lexed = |h: &Harness<'_, Holder>| {
        let app = &h.state().shell.as_ref().unwrap().app;
        let tab = app.active_tab().unwrap();
        let m = tab.mirror.as_ref().unwrap();
        let mut budget = editor_model::highlight::SliceBudget::default();
        tab.highlight.with_spans(m.text(), 2, &mut budget, |s| {
            s.is_some_and(|s| !s.is_empty())
        })
    };
    for _ in 0..100 {
        if lexed(h) {
            h.run_steps(2);
            return true;
        }
        service.fire_timers();
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    lexed(h)
}
