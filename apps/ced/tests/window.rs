// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The window driven end to end with no Bus: a transport that answers the
//! controller's edit-service calls from the fake edit service, so the real
//! controller, mirrors and editor run in-process. Offscreen snapshots of the
//! window, and typing that reaches the service. Regenerate the images with
//! `UPDATE_SNAPSHOTS=1` and look at them before committing.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

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
use settings::follow::Read;
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

const MARKDOWN: &str = "\
# Project notes

Read the [guide](https://example.invalid/guide) and try `mix`.

- Write a document
- Preview the changes

```mix
print(\"Hello\")
```

| Mode | View |
| --- | --- |
| Split | Source and preview |
| Preview | Read and select |
";

/// The fake edit service as a transport. Every request is answered at
/// once, followed by the events it caused, in order. Timers wait on a fake
/// clock until the test fires them. Session-look reads, their retries and
/// subscriptions are recorded for the test to answer.
struct Fake {
    edit_available: Rc<Cell<bool>>,
    editd: Rc<RefCell<FakeEditd>>,
    queue: Rc<RefCell<VecDeque<Delivery>>>,
    timers: Rc<RefCell<Vec<u64>>>,
    looks: Rc<RefCell<Vec<Read>>>,
    retries: Rc<RefCell<Vec<(Duration, u64)>>>,
    subscribed: Rc<RefCell<Vec<String>>>,
    published: Rc<RefCell<Vec<(String, Value)>>>,
}

/// The test's side of the fake: the service, and its clock.
struct Service {
    edit_available: Rc<Cell<bool>>,
    editd: Rc<RefCell<FakeEditd>>,
    queue: Rc<RefCell<VecDeque<Delivery>>>,
    timers: Rc<RefCell<Vec<u64>>>,
    looks: Rc<RefCell<Vec<Read>>>,
    retries: Rc<RefCell<Vec<(Duration, u64)>>>,
    subscribed: Rc<RefCell<Vec<String>>>,
    published: Rc<RefCell<Vec<(String, Value)>>>,
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

    /// The look reads performed so far.
    fn reads(&self) -> usize {
        self.looks.borrow().len()
    }

    /// Answer the latest look read with `reply`.
    fn answer_look(&self, reply: Option<(u8, String, Option<String>)>) {
        let ticket = self.looks.borrow().last().expect("a look read").ticket;
        self.queue
            .borrow_mut()
            .push_back(Delivery::Look { ticket, reply });
    }

    fn deliver(&self, delivery: Delivery) {
        self.queue.borrow_mut().push_back(delivery);
    }
}

impl Transport for Fake {
    fn perform(&mut self, effect: &Effect) {
        if let Effect::Timer { id, .. } = effect {
            self.timers.borrow_mut().push(*id);
        }
        if let Effect::Subscribe { topic } = effect {
            self.subscribed.borrow_mut().push(topic.clone());
        }
        if let Effect::Send { req, out } = effect {
            if !self.edit_available.get() {
                self.queue.borrow_mut().push_back(reply(
                    *req,
                    10,
                    r#"{"error_code":"UNAVAILABLE","message":""}"#.into(),
                ));
                return;
            }
            let args: Value = serde_json::from_str(&out.body).unwrap_or_else(|_| json!({}));
            let mut answer = self
                .editd
                .borrow_mut()
                .handle("local:ced", &out.command, &args);
            // The fake answers every document as plain text; the real
            // service detects the language from the path and first line.
            if out.command == "edit.open"
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
    fn publish(&mut self, topic: &str, bodies: Vec<Value>, _lost: u64) {
        for body in bodies {
            self.published.borrow_mut().push((topic.to_owned(), body));
        }
    }
    fn connected(&self) -> bool {
        true
    }
    fn read_look(&mut self, read: Read) {
        self.looks.borrow_mut().push(read);
    }
    fn retry_look(&mut self, delay: Duration, generation: u64) {
        self.retries.borrow_mut().push((delay, generation));
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
    window_with_edit(theme, files, size, true)
}

fn window_with_edit(
    theme: Theme,
    files: &[(&str, &str)],
    size: egui::Vec2,
    available: bool,
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
        edit_available: Rc::new(Cell::new(available)),
        editd: editd.clone(),
        queue: Rc::default(),
        timers: Rc::default(),
        looks: Rc::default(),
        retries: Rc::default(),
        subscribed: Rc::default(),
        published: Rc::default(),
    };
    let transport = Box::new(Fake {
        edit_available: service.edit_available.clone(),
        editd: editd.clone(),
        queue: service.queue.clone(),
        timers: service.timers.clone(),
        looks: service.looks.clone(),
        retries: service.retries.clone(),
        subscribed: service.subscribed.clone(),
        published: service.published.clone(),
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

fn markdown_snapshot(scheme: Scheme, mode: Mode, action: documents::actions::ActionId, name: &str) {
    let (mut h, _) = window(
        theme(scheme, mode),
        &[("/work/notes.md", MARKDOWN)],
        egui::vec2(1000.0, 620.0),
    );
    h.run_steps(6);
    let shell = h.state_mut().shell.as_mut().unwrap();
    assert!(shell.app.enabled(action));
    shell.app.action(action);
    h.run_steps(4);
    h.snapshot(name);
}

#[test]
fn markdown_split_studio_dark() {
    markdown_snapshot(
        Scheme::Studio,
        Mode::Dark,
        documents::actions::ActionId::ViewMarkdownSplit,
        "markdown_split_studio_dark",
    );
}

#[test]
fn markdown_split_pro_light() {
    markdown_snapshot(
        Scheme::Pro,
        Mode::Light,
        documents::actions::ActionId::ViewMarkdownSplit,
        "markdown_split_pro_light",
    );
}

#[test]
fn markdown_preview_studio_dark() {
    markdown_snapshot(
        Scheme::Studio,
        Mode::Dark,
        documents::actions::ActionId::ViewMarkdownPreview,
        "markdown_preview_studio_dark",
    );
}

#[test]
fn markdown_preview_is_read_only_and_stays_above_the_panels() {
    use egui_kittest::kittest::Queryable;

    let (mut h, service) = window(
        theme(Scheme::Studio, Mode::Dark),
        &[("/work/notes.md", MARKDOWN)],
        egui::vec2(1000.0, 620.0),
    );
    h.run_steps(6);
    let shell = h.state_mut().shell.as_mut().unwrap();
    shell
        .app
        .action(documents::actions::ActionId::ViewMarkdownPreview);
    shell.app.action(documents::actions::ActionId::SearchFind);
    shell.app.ui.problems = true;
    shell.app.ui.output = true;
    shell.app.absorb(vec![Effect::Notice {
        tab: shell.app.active(),
        notice: editor_model::types::Notice::Message {
            level: editor_model::types::Level::Warn,
            text: "A document notice".into(),
        },
    }]);
    h.run_steps(4);
    let heading = h.get_by_label("Project notes");
    let rect = heading.rect();
    assert!(h.query_by_label("A document notice").is_none());
    assert!(rect.bottom() < h.get_by_label("No problems.").rect().top());
    assert!(rect.bottom() < h.get_by_label("Nothing has run yet.").rect().top());
    heading.click();
    h.run_steps(2);
    h.input_mut().events.push(Event::Text("changed".into()));
    h.run_steps(4);
    let editd = service.editd.borrow();
    assert_eq!(editd.text(&editd.buffers()[0]), MARKDOWN);
    assert!(h.state().shell.as_ref().unwrap().app.editor_id.is_none());
}

#[test]
fn missing_edit_shows_waiting_state_and_registration_opens_the_document() {
    use egui_kittest::kittest::Queryable;
    let (mut h, service) = window_with_edit(
        theme(Scheme::Studio, Mode::Dark),
        &[("/work/hello.mix", "print(1)\n")],
        egui::vec2(900.0, 500.0),
        false,
    );
    h.run_steps(4);
    assert!(
        h.query_by_label(documents::controller::EDIT_UNAVAILABLE)
            .is_some()
    );
    service.edit_available.set(true);
    service.deliver(Delivery::Incoming(Incoming::Topic {
        topic: "noded.props.changed".into(),
        body: r#"{"path":"services.registered","new":["edit"]}"#.into(),
    }));
    h.run_steps(6);
    assert!(
        h.query_by_label(documents::controller::EDIT_UNAVAILABLE)
            .is_none()
    );
    assert!(
        h.state()
            .shell
            .as_ref()
            .unwrap()
            .app
            .active_tab()
            .unwrap()
            .mirror
            .is_some()
    );
}

#[test]
fn document_decisions_remain_in_the_notice_strip() {
    use editor_model::model::EditCommand;
    use egui_kittest::kittest::Queryable;

    let (mut h, service) = window(
        theme(Scheme::Studio, Mode::Dark),
        &[("/work/a.txt", "hello world\n")],
        egui::vec2(900.0, 500.0),
    );
    h.run_steps(6);
    let shell = h.state_mut().shell.as_mut().unwrap();
    let tab = shell.app.active().unwrap();
    shell.app.editor_event(
        tab,
        editor::Event::Command(EditCommand::SetSelection(edit::anchor::Selection {
            anchor: 2,
            head: 4,
        })),
    );
    shell.app.editor_event(
        tab,
        editor::Event::Command(EditCommand::Insert("XY".into())),
    );
    let buffer = service.editd.borrow().buffers()[0].clone();
    let answer = service.editd.borrow_mut().handle(
        "local:agent",
        "edit.replace",
        &json!({
            "buffer": buffer, "range": [3, 5], "text": "Q", "origin": "agent:test"
        }),
    );
    assert_eq!(answer.rc, 0);
    h.run_steps(6);
    assert!(h.query_by_label("Re-insert at Caret").is_some());
    assert!(h.query_by_label("Dismiss").is_some());

    service
        .editd
        .borrow_mut()
        .restart("e2", Some("service text\n"));
    let buffer = service.editd.borrow().buffers()[0].clone();
    let answer = service.editd.borrow_mut().handle(
        "local:agent",
        "edit.insert",
        &json!({
            "buffer": buffer, "at": 0, "text": "new ", "origin": "agent:test"
        }),
    );
    assert_eq!(answer.rc, 0);
    for event in service.editd.borrow_mut().take_events() {
        service.deliver(Delivery::Incoming(Incoming::Topic {
            topic: "edit.changed".into(),
            body: serde_json::to_string(&event).unwrap(),
        }));
    }
    h.run_steps(8);
    assert!(h.query_by_label("Keep Mine").is_some());
    assert!(h.query_by_label("Take the Service's").is_some());
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

/// The panel fill `scheme` in `mode` installs.
fn panel_of(scheme: Scheme, mode: Mode) -> egui::Color32 {
    toolkit::style::style(&theme(scheme, mode))
        .visuals
        .panel_fill
}

fn panel(h: &Harness<'static, Holder>) -> egui::Color32 {
    h.ctx.global_style().visuals.panel_fill
}

/// A `settings.get` summary answer: the profile at `revision` looks like
/// `scheme`, dark; `custom` names a design source of its own.
fn summary_reply(
    revision: &str,
    scheme: &str,
    custom: bool,
) -> Option<(u8, String, Option<String>)> {
    let body = json!({"status":"current","summary":{"schema":1,
        "binding":{"instance":"host","profile":"default"},
        "incarnation":"i","revision":revision,"design_revision":revision,
        "source_digest":"d","custom_source":custom,
        "appearance":{"scheme":scheme,"mode":"dark","contrast":"normal","source":null}}});
    Some((0, body.to_string(), None))
}

fn look_reply(revision: &str, scheme: &str) -> Option<(u8, String, Option<String>)> {
    summary_reply(revision, scheme, false)
}

/// The broker's answer while settingsd is not on the Bus.
fn missing() -> Option<(u8, String, Option<String>)> {
    Some((
        10,
        String::new(),
        Some("Service 'settingsd' not found".into()),
    ))
}

/// The theme file's session theme, as the shell loads it.
fn file_panel() -> egui::Color32 {
    toolkit::style::style(&Theme::load()).visuals.panel_fill
}

/// A window whose summary subscription has landed, with its start read
/// answered by `reply`.
fn following(reply: Option<(u8, String, Option<String>)>) -> (Harness<'static, Holder>, Service) {
    let (mut h, service) = window(
        theme(Scheme::Studio, Mode::Dark),
        &[("/work/greet.mix", MIX)],
        egui::vec2(640.0, 360.0),
    );
    h.run_steps(4);
    let summary = settings::summary_topic(settings::follow::PROFILE);
    assert!(
        service.subscribed.borrow().contains(&summary),
        "the summary topic: {:?}",
        service.subscribed.borrow()
    );
    assert_eq!(service.reads(), 0, "no read before the subscription lands");
    // Another topic landing is not the one that matters.
    service.deliver(Delivery::Subscribed {
        topic: "edit.changed".into(),
    });
    h.run_steps(2);
    assert_eq!(service.reads(), 0);
    service.deliver(Delivery::Subscribed { topic: summary });
    h.run_steps(2);
    assert_eq!(service.reads(), 1, "one read once hints arrive");
    assert_eq!(service.looks.borrow()[0].command, "settings.get");
    service.answer_look(reply);
    h.run_steps(4);
    (h, service)
}

fn hint(service: &Service) {
    service.deliver(Delivery::Incoming(Incoming::Topic {
        topic: settings::summary_topic(settings::follow::PROFILE),
        body: "{}".into(),
    }));
}

/// settingsd's look is the session theme, under the window's own choice,
/// and read again on each hint, reconnection and service registration.
/// Without settingsd, the theme file's session theme comes back.
#[test]
fn the_session_look_follows_settingsd() {
    let (mut h, service) = following(look_reply("7", "forest"));
    assert_eq!(panel(&h), panel_of(Scheme::Forest, Mode::Dark));

    // The window's own mode lays over the session look.
    h.state_mut().shell.as_mut().unwrap().app.theme = toolkit::theme_menu::Choice {
        mode: Some(Mode::Light),
        ..Default::default()
    };
    h.run_steps(4);
    assert_eq!(panel(&h), panel_of(Scheme::Forest, Mode::Light));
    h.state_mut().shell.as_mut().unwrap().app.theme = Default::default();
    h.run_steps(4);

    // The theme file changing does not override settingsd's look.
    service.deliver(Delivery::Incoming(Incoming::Topic {
        topic: "theme.changed".into(),
        body: "{}".into(),
    }));
    h.run_steps(4);
    assert_eq!(panel(&h), panel_of(Scheme::Forest, Mode::Dark));

    // A hint asks again; settingsd gone: the theme file's theme returns.
    hint(&service);
    h.run_steps(2);
    assert_eq!(service.reads(), 2, "a hint reads again");
    service.answer_look(missing());
    h.run_steps(4);
    assert_eq!(panel(&h), file_panel());

    // Only a registration change reads again; settingsd's restart keeps the
    // revision, and the look comes back.
    service.deliver(Delivery::Incoming(Incoming::Topic {
        topic: "noded.props.changed".into(),
        body: json!({"path":"node.uptime"}).to_string(),
    }));
    h.run_steps(2);
    assert_eq!(service.reads(), 2, "not a registration");
    service.deliver(Delivery::Incoming(Incoming::Topic {
        topic: "noded.props.changed".into(),
        body: json!({"path":"services.registered","value":["settingsd"]}).to_string(),
    }));
    h.run_steps(2);
    assert_eq!(service.reads(), 3, "a registration reads again");
    service.answer_look(look_reply("7", "forest"));
    h.run_steps(4);
    assert_eq!(panel(&h), panel_of(Scheme::Forest, Mode::Dark));

    // A reconnection reads again.
    service.deliver(Delivery::Incoming(Incoming::Connection { up: true }));
    h.run_steps(2);
    assert_eq!(service.reads(), 4, "a reconnection reads again");
    service.answer_look(look_reply("7", "forest"));
    h.run_steps(4);

    // A look this build cannot draw, one naming its own design source, or
    // settingsd gone: the theme file's theme, with the window's choice kept
    // on top through all three.
    let light = toolkit::theme_menu::Choice {
        mode: Some(Mode::Light),
        ..Default::default()
    };
    h.state_mut().shell.as_mut().unwrap().app.theme = light;
    h.run_steps(4);
    assert_eq!(panel(&h), panel_of(Scheme::Forest, Mode::Light));
    let file_light = toolkit::style::style(&Theme::load().with_choice(&light).unwrap())
        .visuals
        .panel_fill;
    // Each case starts from a live, drawable look (a newer revision of
    // Forest), so ignoring the reply would leave Forest on screen, which
    // must differ from the theme file's for the test to prove anything.
    assert_ne!(
        panel_of(Scheme::Forest, Mode::Light),
        file_light,
        "the theme file's session theme must not look like Forest here"
    );
    for (live, case, reply) in [
        ("7", "settingsd gone", missing()),
        ("8", "unknown scheme", look_reply("9", "no-such-scheme")),
        ("10", "custom source", summary_reply("11", "forest", true)),
    ] {
        if live != "7" {
            hint(&service);
            h.run_steps(2);
            service.answer_look(look_reply(live, "forest"));
            h.run_steps(4);
        }
        assert_eq!(
            panel(&h),
            panel_of(Scheme::Forest, Mode::Light),
            "before {case}"
        );
        hint(&service);
        h.run_steps(2);
        service.answer_look(reply);
        h.run_steps(4);
        assert_eq!(panel(&h), file_light, "{case}");
    }
    // A theme-file hint while it is the session theme keeps it, the
    // window's choice still on top. (The file's content can't be varied
    // safely here: tests share one process environment.)
    service.deliver(Delivery::Incoming(Incoming::Topic {
        topic: "theme.changed".into(),
        body: "{}".into(),
    }));
    h.run_steps(4);
    assert_eq!(panel(&h), file_light);
}

/// A read made before the subscription lands (a reconnection) can miss a
/// change; the read the subscription landing makes catches it.
#[test]
fn the_subscription_landing_catches_a_missed_change() {
    let (mut h, service) = window(
        theme(Scheme::Studio, Mode::Dark),
        &[],
        egui::vec2(640.0, 360.0),
    );
    h.run_steps(4);
    service.deliver(Delivery::Incoming(Incoming::Connection { up: true }));
    h.run_steps(2);
    assert_eq!(service.reads(), 1, "a reconnection reads early");
    service.answer_look(look_reply("7", "forest"));
    h.run_steps(4);
    assert_eq!(panel(&h), panel_of(Scheme::Forest, Mode::Dark));
    // The profile changes now; no hint can arrive yet. Then the
    // subscription lands and the read it makes has the change.
    service.deliver(Delivery::Subscribed {
        topic: settings::summary_topic(settings::follow::PROFILE),
    });
    h.run_steps(2);
    assert_eq!(service.reads(), 2, "the subscription landing reads");
    service.answer_look(look_reply("8", "ocean"));
    h.run_steps(4);
    assert_eq!(panel(&h), panel_of(Scheme::Ocean, Mode::Dark));
}

/// A lost read keeps the last look and arms one retry, backing off. A read
/// going out voids it: the voided retry firing later reads nothing, and an
/// answer to a superseded read changes nothing, retries included.
#[test]
fn look_retries_back_off_and_are_voided_by_newer_reads() {
    let (mut h, service) = following(look_reply("7", "forest"));
    service.answer_look(None);
    h.run_steps(4);
    // The start read was answered already: this answer is stale.
    assert!(
        service.retries.borrow().is_empty(),
        "a stale answer arms nothing"
    );

    hint(&service);
    h.run_steps(2);
    service.answer_look(None);
    h.run_steps(4);
    assert_eq!(
        panel(&h),
        panel_of(Scheme::Forest, Mode::Dark),
        "the last look stands"
    );
    let (first_delay, voided) = *service.retries.borrow().last().expect("a retry armed");
    assert_eq!(first_delay, settings::follow::RETRY_FIRST);

    // A hint reads before the retry fires: the retry is void, even while
    // that read is still unanswered.
    hint(&service);
    h.run_steps(2);
    assert_eq!(service.reads(), 3);
    service.deliver(Delivery::LookRetry { generation: voided });
    h.run_steps(2);
    assert_eq!(service.reads(), 3, "the voided retry reads nothing");
    // That read fails too: the next retry backs off.
    service.answer_look(None);
    h.run_steps(4);
    assert_eq!(service.reads(), 3, "no follow-up read was folded in");
    let (second_delay, armed) = *service.retries.borrow().last().unwrap();
    assert_eq!(second_delay, first_delay * 2, "backing off");
    assert_ne!(armed, voided);

    // An answer to an old read changes nothing.
    let old = service.looks.borrow()[0].ticket;
    service.deliver(Delivery::Look {
        ticket: old,
        reply: look_reply("8", "pro"),
    });
    h.run_steps(4);
    assert_eq!(
        service.retries.borrow().len(),
        2,
        "no retry from a stale answer"
    );
    assert_eq!(panel(&h), panel_of(Scheme::Forest, Mode::Dark));

    // The armed retry reads, and its answer is the look.
    service.deliver(Delivery::LookRetry { generation: armed });
    h.run_steps(2);
    assert_eq!(service.reads(), 4, "the armed retry reads");
    service.answer_look(look_reply("8", "ocean"));
    h.run_steps(4);
    assert_eq!(panel(&h), panel_of(Scheme::Ocean, Mode::Dark));
}

/// Closing reads nothing more: hints and reconnections queued behind the
/// quit are not followed.
#[test]
fn closing_reads_no_look() {
    let (mut h, service) = following(look_reply("7", "forest"));
    // A read in flight, and a retry armed, when the window closes.
    hint(&service);
    h.run_steps(2);
    service.answer_look(None);
    h.run_steps(4);
    let (_, armed) = *service.retries.borrow().last().expect("a retry armed");
    hint(&service);
    h.run_steps(2);
    let (reads, retries) = (service.reads(), service.retries.borrow().len());
    let shell = h.state_mut().shell.as_mut().unwrap();
    // A retry of this generation would read but for the quit.
    let current = shell.look_generation();
    shell.app.action(documents::actions::ActionId::FileExit);
    // Everything after the quit: the read failing, retries coming due (a
    // voided one and the current one), a hint, a reconnection, the
    // subscription landing again, a late look.
    service.answer_look(None);
    service.deliver(Delivery::LookRetry { generation: armed });
    service.deliver(Delivery::LookRetry {
        generation: current,
    });
    hint(&service);
    service.deliver(Delivery::Incoming(Incoming::Connection { up: true }));
    service.deliver(Delivery::Subscribed {
        topic: settings::summary_topic(settings::follow::PROFILE),
    });
    service.answer_look(look_reply("9", "ocean"));
    h.run_steps(6);
    assert_eq!(service.reads(), reads, "nothing read after quitting");
    assert_eq!(service.retries.borrow().len(), retries, "no retry armed");
    assert_eq!(
        panel(&h),
        panel_of(Scheme::Forest, Mode::Dark),
        "the look stands"
    );
}

/// Quitting with a retry armed and no read in flight: the retry coming due,
/// a hint, a reconnection and the subscription landing would each read at
/// once (nothing is pending to fold into), so only the closing guard stops
/// them.
#[test]
fn closing_with_an_armed_retry_reads_nothing() {
    let (mut h, service) = following(look_reply("7", "forest"));
    hint(&service);
    h.run_steps(2);
    service.answer_look(None);
    h.run_steps(4);
    let shell = h.state_mut().shell.as_mut().unwrap();
    assert_eq!(shell.follower().pending(), None, "no read in flight");
    let (_, armed) = *service.retries.borrow().last().expect("a retry armed");
    assert_eq!(shell.look_generation(), armed, "the armed retry is live");
    let reads = service.reads();
    shell.app.action(documents::actions::ActionId::FileExit);
    service.deliver(Delivery::LookRetry { generation: armed });
    h.run_steps(4);
    assert_eq!(service.reads(), reads, "the live retry read nothing");
    for delivery in [
        Delivery::Incoming(Incoming::Connection { up: true }),
        Delivery::Subscribed {
            topic: settings::summary_topic(settings::follow::PROFILE),
        },
    ] {
        service.deliver(delivery);
    }
    hint(&service);
    h.run_steps(4);
    assert_eq!(service.reads(), reads, "nothing read after quitting");
}

/// The editor in settingsd's look: palette, chrome and highlighting follow
/// it (inspect the image).
#[test]
fn the_editor_in_the_session_look() {
    let (mut h, service) = following(look_reply("7", "forest"));
    let shell = h.state_mut().shell.as_mut().unwrap();
    let first = shell.app.ctl.tabs()[0].id;
    let fx = shell.app.ctl.select_tab(first);
    shell.app.absorb(fx);
    assert!(
        lex(&mut h, &service),
        "greet.mix is lexed before the snapshot"
    );
    h.snapshot("session_look_forest_dark");
}

/// docs/docs/decisions/2026-10-11-executed-commands.md: a shortcut and a menu
/// pick are published on ced.action.executed with their origin and
/// consecutive seq.
#[test]
fn executed_commands_are_published_with_their_origin() {
    use documents::controller::BusCommand;
    use egui_kittest::kittest::Queryable;
    let (mut h, service) = window(
        theme(Scheme::Pro, Mode::Light),
        &[],
        egui::vec2(820.0, 460.0),
    );
    h.run_steps(4);
    // Refused by the window: there is no document to save.
    service.deliver(Delivery::Command(BusCommand {
        id: 9,
        command: "ced.execute".into(),
        body: json!({ "id": "file.save_as" }).to_string(),
        caller_key: "local:test".into(),
    }));
    h.run_steps(4);
    // A shortcut.
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    h.run_steps(4);
    // A menu pick, which opens a dialog.
    h.get_by_label("Help").click();
    h.run_steps(3);
    h.get_by_label("About MixOS Editor").click();
    h.run_steps(3);
    // Refused: every command is disabled while the dialog is open.
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    h.run_steps(3);
    h.key_press(Key::Escape);
    h.run_steps(3);
    // The Bus: one action accepted, one unknown.
    let bus = |id: u64, action: &str| {
        Delivery::Command(BusCommand {
            id,
            command: "ced.execute".into(),
            body: json!({ "id": action }).to_string(),
            caller_key: "local:test".into(),
        })
    };
    service.deliver(bus(1, "view.zoom_in"));
    service.deliver(bus(2, "nope"));
    h.run_steps(6);
    let published = service.published.borrow();
    let bodies: Vec<Value> = published
        .iter()
        .filter(|(topic, _)| topic == "ced.action.executed")
        .map(|(_, body)| body.clone())
        .collect();
    for body in &bodies {
        assert_eq!(body["v"], 2);
        assert_eq!(body.get("params"), Some(&serde_json::Value::Null));
        assert_eq!(body.get("result"), Some(&serde_json::Value::Null));
    }
    let got: Vec<(&str, &str)> = bodies
        .iter()
        .map(|b| (b["id"].as_str().unwrap(), b["origin"].as_str().unwrap()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("file.new", "shortcut"),
            ("help.about", "menu"),
            ("view.zoom_in", "bus")
        ]
    );
    for pair in bodies.windows(2) {
        assert_eq!(
            pair[1]["seq"].as_u64(),
            pair[0]["seq"].as_u64().map(|s| s + 1)
        );
    }
}
