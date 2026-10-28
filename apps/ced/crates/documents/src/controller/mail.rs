// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::mail::{Headers, Kind, MailState};

pub(super) struct SaveIntent {
    path: String,
    send: bool,
    command: Option<(u64, String)>,
}

pub(super) fn error_code(message: &str) -> &'static str {
    if matches!(message, "the mail document is busy" | "a mail save or Send is already in flight" | "the body buffer is reconnecting")
        || message.starts_with("the parked mail save was cancelled")
        || message.starts_with("the mail document changed on disk;") {
        code::CONFLICT
    } else if message.starts_with("invalid ") || message.starts_with("mail document ")
        || message.starts_with("empty mail-") || message.starts_with("malformed mail ")
        || message.starts_with("header not allowed") || message.starts_with("duplicate mail ")
        || matches!(message, "not a mail document" | "thread headers are read-only" | "path is not UTF-8" | "save the document to a file first" | "Send is only available on a mail document") {
        code::INVALID_ARGUMENT
    } else { code::INTERNAL }
}

fn canonical_body(body: &str) -> String {
    body.replace("\r\n", "\n").trim_end().to_owned()
}

impl Tab {
    pub fn dirty(&self) -> bool {
        if let Some(mail) = &self.mail {
            if mail.headers != mail.saved_headers { return true; }
            let Some(mirror) = &self.mirror else { return false; };
            if let Some((generation, dirty)) = mail.dirty_body.get() && generation == mirror.view_gen() {
                return dirty;
            }
            let dirty = canonical_body(&text_string(mirror.text())) != canonical_body(&mail.saved_body);
            mail.dirty_body.set(Some((mirror.view_gen(), dirty)));
            dirty
        } else {
            self.mirror.as_ref().is_some_and(|m| m.meta().dirty)
        }
    }
}

impl Controller {
    pub fn mail_save_parked(&self, tab: TabId) -> bool {
        self.x.get(&tab).is_some_and(|x| x.mail_save.is_some())
    }

    pub(super) fn cancel_mail_save(&mut self, tab: TabId, notice: &str, fx: &mut Vec<Effect>) {
        if let Some(park) = self.x.get_mut(&tab).and_then(|x| x.mail_save.take()) {
            self.x.get_mut(&tab).unwrap().close_after_save = None;
            if let Some(mail) = self.tab_mut(tab).and_then(|t| t.mail.as_mut()) {
                mail.saving = false;
                mail.sending = false;
                mail.saving_body = None;
                mail.notice = Some(notice.into());
            }
            Self::mail_answer(park.command, Err(notice.into()), fx);
        }
    }

    pub(super) fn cancel_parked_mail_saves(&mut self, fx: &mut Vec<Effect>) {
        for tab in self.tab_ids() {
            self.cancel_mail_save(tab, "the parked mail save was cancelled by Exit", fx);
        }
    }

    pub(super) fn close_mail_buffers(&mut self, fx: &mut Vec<Effect>) {
        let buffers: Vec<_> = self.tabs.iter().filter(|t| t.mail.is_some())
            .filter_map(|t| t.mirror.as_ref().map(|m| m.buffer().to_string())).collect();
        for buffer in buffers {
            self.send(Outgoing { command: "edit.close".into(), body: json!({"buffer":buffer,"force":true,"origin":UI_ORIGIN}).to_string(), op_id: None, deadline_ms: DEADLINE_MS }, Req::Discard, fx);
        }
    }

    pub(super) fn drain_mail_save(&mut self, tab: TabId, fx: &mut Vec<Effect>) {
        if let Some(park) = self.x.get_mut(&tab).and_then(|x| x.mail_save.take()) {
            let mail = self.tab_mut(tab).unwrap().mail.as_mut().unwrap();
            mail.saving = false;
            mail.sending = false;
            self.op_cmd = park.command.clone();
            if let Err(message) = self.save_mail(tab, Some(park.path), park.send, fx) {
                self.op_cmd = None;
                Self::mail_answer(park.command, Err(message), fx);
            }
        }
    }

    pub(super) fn mail_exit_refusal(&self) -> Option<&'static str> {
        if self
            .tabs
            .iter()
            .filter_map(|tab| tab.mail.as_ref())
            .any(|mail| mail.saving || mail.sending)
        {
            return Some("wait for the mail save or Send to finish before exiting");
        }
        self.tabs
            .iter()
            .any(|tab| tab.mail.is_some() && tab.dirty())
            .then_some("save or close mail documents with unsaved changes before exiting")
    }

    pub fn can_send(&self, tab: TabId) -> bool {
        self.tab(tab).is_some_and(|t| t.mail.as_ref().is_some_and(|mail| !mail.sending && !mail.saving)
            && t.mirror.as_ref().is_some_and(|m| matches!(m.phase(), Phase::Live)))
    }

    /// One field's committed edit is one header undo step. Body undo remains
    /// owned by edit; header history is exhausted before body undo.
    pub fn edit_headers(&mut self, tab: TabId, headers: Headers) -> Result<(), String> {
        self.update_headers(tab, headers, false)
    }

    /// Text fields preview every frame; one focus/edit session is one undo step.
    pub fn preview_headers(&mut self, tab: TabId, headers: Headers) -> Result<(), String> {
        self.update_headers(tab, headers, true)
    }

    pub fn commit_headers(&mut self, tab: TabId) -> bool {
        self.tab_mut(tab).and_then(|t| t.mail.as_mut()).is_some_and(|mail| mail.editing_field.take().is_some())
    }

    fn update_headers(&mut self, tab: TabId, headers: Headers, preview: bool) -> Result<(), String> {
        let mail = self.tab_mut(tab).and_then(|t| t.mail.as_mut()).ok_or("not a mail document")?;
        if mail.sending || mail.saving { return Err("the mail document is busy".into()); }
        if headers.attachments.iter().any(|path| path.to_str().is_none()) {
            let message = maildoc::Refusal::InvalidValue { header: "mail-attachments".into(), reason: "path is not UTF-8".into() }.to_string();
            mail.notice = Some(message.clone());
            return Err(message);
        }
        if headers.in_reply_to != mail.headers.in_reply_to || headers.references != mail.headers.references {
            return Err("thread headers are read-only".into());
        }
        if headers != mail.headers {
            // More than one field can change in a single UI frame or a Bus
            // request. Keep each field individually undoable even then.
            macro_rules! update {
                ($($field:ident),*) => { $(
                    if mail.headers.$field != headers.$field {
                        if !preview || mail.editing_field != Some(stringify!($field)) {
                            mail.undo.push(mail.headers.clone());
                            if mail.undo.len() > 256 { mail.undo.remove(0); }
                        }
                        mail.editing_field = preview.then_some(stringify!($field));
                        mail.headers.$field = headers.$field;
                    }
                )* };
            }
            update!(from, to, cc, bcc, reply_to, subject, attachments);
            mail.redo.clear();
        }
        Ok(())
    }

    pub fn show_mail_row(&mut self, tab: TabId, field: &str) {
        if let Some(mail) = self.tab_mut(tab).and_then(|t| t.mail.as_mut()) {
            match field { "cc" => mail.show_cc = !mail.show_cc, "bcc" => mail.show_bcc = !mail.show_bcc,
                "reply-to" => mail.show_reply_to = !mail.show_reply_to, _ => {} }
        }
    }

    pub(super) fn detect_document(&mut self, tab: TabId, fx: &mut Vec<Effect>) -> bool {
        let Some(t) = self.tab_mut(tab) else { return false; };
        if !t.mirror.as_ref().is_some_and(|m| matches!(m.phase(), Phase::Live)) { return false; }
        if let Some(mail) = t.mail.as_mut() && mail.seeding {
            mail.seeding = false;
            if text_string(t.mirror.as_ref().unwrap().text()) != mail.saved.body {
                // An older daemon may ignore the new optional open argument.
                // Reopen the original file as text; never save an empty body.
                let buffer = t.mirror.as_ref().unwrap().buffer().to_string();
                let path = t.path.clone();
                t.mail = None;
                t.mirror = None;
                t.document_notice = Some("the edit service could not seed the mail body; editd 0.3.0 or later is required".into());
                self.send(Outgoing { command: "edit.close".into(), body: json!({"buffer":buffer,"force":true,"origin":UI_ORIGIN}).to_string(), op_id: None, deadline_ms: DEADLINE_MS }, Req::Discard, fx);
                self.send_open(tab, path, false, fx);
                return true;
            }
        }
        if t.classified { return false; }
        t.classified = true;
        let text = text_string(t.mirror.as_ref().unwrap().text());
        if t.mirror.as_ref().unwrap().meta().bom {
            if matches!(crate::mail::classify(text.trim_start_matches('\u{feff}')), Kind::Mail(_) | Kind::Refused(_)) {
                t.document_notice = Some("mail documents with a UTF-8 BOM open as text; remove the BOM to open the mail header form".into());
            }
            return false;
        }
        match crate::mail::classify(&text) {
            Kind::Mail(document) => {
                let buffer = t.mirror.as_ref().unwrap().buffer().to_string();
                t.mail = Some(MailState::new(document.clone(), text));
                t.mirror = None;
                self.send(Outgoing { command: "edit.close".into(), body: json!({"buffer": buffer, "origin": UI_ORIGIN}).to_string(), op_id: None, deadline_ms: DEADLINE_MS }, Req::Discard, fx);
                self.send_open(tab, None, false, fx);
                true
            }
            Kind::Refused(message) => { t.document_notice = Some(message); false }
            Kind::Text | Kind::BusMessage => false,
        }
    }

    pub(super) fn save_mail(&mut self, tab: TabId, path: Option<String>, send: bool, fx: &mut Vec<Effect>) -> Result<(), String> {
        let t = self.tab_mut(tab).ok_or("no tab is open")?;
        let m = t.mirror.as_ref().ok_or("the buffer is not open")?;
        let mail = t.mail.as_mut().ok_or("Send is only available on a mail document")?;
        if mail.sending || mail.saving { return Err("a mail save or Send is already in flight".into()); }
        if !matches!(m.phase(), Phase::Live) { return Err("the body buffer is reconnecting".into()); }
        let idle = m.is_idle();
        let path = path.or_else(|| t.path.clone()).ok_or("save the document to a file first")?;
        if !idle {
            mail.saving = true;
            mail.sending = send;
            mail.saving_body = None;
            self.x.get_mut(&tab).unwrap().mail_save = Some(SaveIntent { path, send, command: self.op_cmd.take() });
            return Ok(());
        }
        let body = canonical_body(&text_string(m.text()));
        let document = mail.headers.document(body.clone());
        let text = match maildoc::write(&document) {
            Ok(text) => text,
            Err(error) => { let message = error.to_string(); mail.notice = Some(message.clone()); return Err(message); }
        };
        let normal_path = |p: &str| std::fs::canonicalize(p).or_else(|_| std::path::absolute(p));
        let path = normal_path(&path).map_err(|e| e.to_string())?;
        let expected = t.path.as_ref().and_then(|p| normal_path(p).ok()).filter(|p| p == &path).map(|_| mail.disk.clone());
        let path = path.to_str().ok_or("path is not UTF-8")?.to_owned();
        mail.saving = true;
        mail.saving_body = Some(body);
        mail.sending = send;
        mail.notice = send.then(|| "Sending…".to_owned());
        let command = self.op_cmd.take();
        let effect = Effect::SaveMail { tab, path, text, expected, send, command };
        fx.push(effect);
        Ok(())
    }

    pub fn on_mail_saved(&mut self, tab: TabId, path: String, text: String, send: bool, command: Option<(u64, String)>, result: Result<(), String>) -> Vec<Effect> {
        let mut fx = Vec::new();
        let Some(t) = self.tab_mut(tab) else { return fx; };
        let Some(mail) = t.mail.as_mut() else { return fx; };
        mail.saving = false;
        match result {
            Ok(()) => {
                mail.saved = maildoc::parse(&text).expect("validated save");
                mail.saved_headers = mail.headers.clone();
                mail.saved_body = mail.saving_body.take().expect("save body captured");
                mail.dirty_body.set(None);
                mail.disk = text;
                t.path = Some(path.clone());
                if send { fx.push(Effect::SendMail { tab, path, command }); }
                else {
                    mail.notice = Some("Saved".into());
                    Self::mail_answer(command, Ok("Saved".into()), &mut fx);
                    if let Some(intent) = self.x.get_mut(&tab).and_then(|x| x.close_after_save.take()) { self.close(tab, true, None, intent, &mut fx); }
                }
            }
            Err(message) => {
                mail.sending = false;
                mail.saving_body = None;
                mail.notice = Some(message.clone());
                Self::mail_answer(command, Err(message), &mut fx);
                if let Some(x) = self.x.get_mut(&tab) { x.close_after_save = None; }
            }
        }
        self.session_changed(&mut fx);
        fx
    }

    pub fn on_mail_sent(&mut self, tab: TabId, command: Option<(u64, String)>, result: Result<String, String>) -> Vec<Effect> {
        if let Some(mail) = self.tab_mut(tab).and_then(|t| t.mail.as_mut()) {
            mail.sending = false;
            mail.notice = Some(result.as_ref().unwrap_or_else(|message| message).clone());
        }
        let mut fx = Vec::new();
        Self::mail_answer(command, result, &mut fx);
        fx
    }

    fn mail_answer(command: Option<(u64, String)>, result: Result<String, String>, fx: &mut Vec<Effect>) {
        if let Some((id, action)) = command {
            let (rc, body) = match result {
                Ok(message) => (0, json!({"id": action, "ok": true, "result": {"message": message}}).to_string()),
                Err(message) => (10, refusal(error_code(&message), message, None)),
            };
            fx.push(Effect::Respond { id, rc, body });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_model::fake::FakeEditd;
    use std::collections::VecDeque;

    const PATH: &str = "/home/user/letter.md";
    const LETTER: &str = "---\nmail-from: ann@example.org\nmail-subject: Hello\nmail-to: bob@example.org\n---\nBody";

    fn settle(c: &mut Controller, service: &mut FakeEditd, fx: Vec<Effect>) -> Vec<Effect> {
        settle_with_seed(c, service, fx, true)
    }

    fn settle_with_seed(c: &mut Controller, service: &mut FakeEditd, fx: Vec<Effect>, seed: bool) -> Vec<Effect> {
        let mut queue: VecDeque<_> = fx.into();
        let mut rest = Vec::new();
        let mut steps = 0;
        while let Some(effect) = queue.pop_front() {
            steps += 1;
            assert!(steps < 1000, "controller did not settle");
            if let Effect::Send { req, out } = effect {
                let mut args: Value = serde_json::from_str(&out.body).unwrap();
                if !seed && out.command == "edit.open" { args.as_object_mut().unwrap().remove("text"); }
                let mut answer = service.handle("local:ced", &out.command, &args);
                if out.command == "edit.close" { answer.rc = 0; answer.body = json!({}); }
                queue.extend(c.on_incoming(Incoming::Reply { req, rc: answer.rc, body: answer.body.to_string() }));
                for event in service.take_events() {
                    queue.extend(c.on_incoming(Incoming::Topic { topic: "edit.changed".into(), body: serde_json::to_string(&event).unwrap() }));
                }
            } else { rest.push(effect); }
        }
        rest
    }

    fn opened(text: &str) -> (Controller, FakeEditd, TabId) {
        let mut c = Controller::new(Config::default(), 1, true);
        let mut service = FakeEditd::new("0000e1e1");
        service.create(Some(PATH), text);
        let fx = c.open_paths(&[PATH.into()], Intent::ui(0));
        settle(&mut c, &mut service, fx);
        let tab = c.active().unwrap();
        (c, service, tab)
    }

    fn execute(c: &mut Controller, tab: TabId, action: &str) -> Vec<Effect> {
        c.on_bus_command(BusCommand { id: 77, command: "ced.execute".into(), body: json!({"id":action,"params":{"tab":tab}}).to_string(), caller_key: "local:test".into() })
    }

    #[test]
    fn refused_mail_is_ordinary_text_with_the_exact_notice() {
        let text = "---\nmail-to: invalid\n---\nFix this";
        let (c, _, tab) = opened(text);
        let t = c.tab(tab).unwrap();
        assert!(t.mail.is_none());
        assert_eq!(text_string(t.mirror.as_ref().unwrap().text()), text);
        assert_eq!(t.document_notice.as_deref(), Some(maildoc::parse(text).unwrap_err().to_string().as_str()));
        assert!(!t.dirty());
    }

    #[test]
    fn body_only_headers_dirty_undo_and_round_trip_save_as() {
        let (mut c, _, tab) = opened(LETTER);
        let t = c.tab(tab).unwrap();
        assert_eq!(text_string(t.mirror.as_ref().unwrap().text()), "Body");
        assert!(!t.dirty());
        let mut headers = t.mail.as_ref().unwrap().headers.clone();
        headers.subject = "Edited".into();
        c.edit_headers(tab, headers).unwrap();
        assert!(c.tab(tab).unwrap().dirty());
        execute(&mut c, tab, "edit.undo");
        assert!(!c.tab(tab).unwrap().dirty());
        execute(&mut c, tab, "edit.redo");
        assert!(c.tab(tab).unwrap().dirty());
        let fx = c.on_action_args(Some(tab), ActionId::FileSaveAs, Some(json!({"path":"/home/user/copy.md"})), Intent::ui(tab));
        let Effect::SaveMail { path, text, send, command, .. } = fx.into_iter().find(|e| matches!(e, Effect::SaveMail { .. })).unwrap() else { unreachable!() };
        let saved = maildoc::parse(&text).unwrap();
        assert_eq!(saved.subject, "Edited");
        c.on_mail_saved(tab, path.clone(), text.clone(), send, command, Ok(()));
        assert!(!c.tab(tab).unwrap().dirty());
        assert_eq!(c.tab(tab).unwrap().path.as_deref(), Some(path.as_str()));
        let (mut reopened, _, other) = opened(&text);
        assert_eq!(reopened.tab(other).unwrap().mail.as_ref().unwrap().saved, saved);
        let fx = execute(&mut reopened, other, "file.save");
        assert!(fx.iter().any(|e| matches!(e, Effect::SaveMail { text: saved_text, .. } if saved_text == &text)));
    }

    #[test]
    fn send_waits_for_save_then_emits_once_and_refuses_a_second_send() {
        let (mut c, _, tab) = opened(LETTER);
        let fx = execute(&mut c, tab, "file.send");
        assert!(!fx.iter().any(|e| matches!(e, Effect::SendMail { .. } | Effect::Respond { .. })));
        let Effect::SaveMail { path, text, send, command, .. } = fx.into_iter().find(|e| matches!(e, Effect::SaveMail { .. })).unwrap() else { unreachable!() };
        assert!(send);
        assert!(matches!(execute(&mut c, tab, "file.send").as_slice(), [Effect::Respond { rc: 10, .. }]));
        let fx = c.on_mail_saved(tab, path, text, send, command.clone(), Ok(()));
        assert_eq!(fx.iter().filter(|e| matches!(e, Effect::SendMail { .. })).count(), 1);
        assert!(!c.can_send(tab));
        assert!(!execute(&mut c, tab, "file.send").iter().any(|e| matches!(e, Effect::SaveMail { .. } | Effect::SendMail { .. })));
        let message = "This sender is not one of your identities.";
        let fx = c.on_mail_sent(tab, command, Err(message.into()));
        assert_eq!(c.tab(tab).unwrap().mail.as_ref().unwrap().notice.as_deref(), Some(message));
        assert!(fx.iter().any(|e| matches!(e, Effect::Respond { rc:10, body, .. } if serde_json::from_str::<Value>(body).unwrap()["message"] == message)));
    }

    #[test]
    fn failed_save_or_invalid_headers_never_send() {
        let (mut c, _, tab) = opened(LETTER);
        let fx = execute(&mut c, tab, "file.send");
        let Effect::SaveMail { path, text, send, command, .. } = fx.into_iter().find(|e| matches!(e, Effect::SaveMail { .. })).unwrap() else { unreachable!() };
        let fx = c.on_mail_saved(tab, path, text, send, command, Err("disk full".into()));
        assert!(!fx.iter().any(|e| matches!(e, Effect::SendMail { .. })));
        assert!(c.can_send(tab));
        let mut headers = c.tab(tab).unwrap().mail.as_ref().unwrap().headers.clone();
        headers.to = "invalid".into();
        c.edit_headers(tab, headers).unwrap();
        let fx = execute(&mut c, tab, "file.send");
        assert!(!fx.iter().any(|e| matches!(e, Effect::SaveMail { .. } | Effect::SendMail { .. })));
        assert!(c.tab(tab).unwrap().dirty());
    }

    #[test]
    fn send_is_unavailable_on_plain_and_bus_message_tabs() {
        for text in ["plain text", "---\nto: edit\ncommand: edit.info\n---\n"] {
            let (mut c, _, tab) = opened(text);
            assert!(!c.can_send(tab));
            let fx = execute(&mut c, tab, "file.send");
            assert!(fx.iter().any(|e| matches!(e, Effect::Respond { rc: 10, .. })));
            assert!(!fx.iter().any(|e| matches!(e, Effect::SaveMail { .. } | Effect::SendMail { .. })));
        }
    }

    #[test]
    fn send_queues_behind_body_edits_and_undo_cannot_remove_the_opened_body() {
        let (mut c, mut service, tab) = opened(LETTER);
        let fx = execute(&mut c, tab, "edit.undo");
        settle(&mut c, &mut service, fx);
        assert_eq!(text_string(c.tab(tab).unwrap().mirror.as_ref().unwrap().text()), "Body");
        let fx = c.on_editor(tab, EditorMsg::Command(EditCommand::Insert("New ".into())));
        assert!(!c.tab(tab).unwrap().mirror.as_ref().unwrap().is_idle());
        let send = execute(&mut c, tab, "file.send");
        assert!(!send.iter().any(|e| matches!(e, Effect::SaveMail { .. } | Effect::SendMail { .. } | Effect::Respond { .. })));
        let completed = settle(&mut c, &mut service, fx);
        assert!(completed.iter().any(|e| matches!(e, Effect::SaveMail { text, .. } if maildoc::parse(text).unwrap().body == "New Body")));
    }

    #[test]
    fn header_edits_in_one_tab_leave_another_mail_tab_alone() {
        let (mut c, mut service, first) = opened(LETTER);
        let second_path = "/home/user/second.md";
        service.create(Some(second_path), LETTER);
        let fx = c.open_paths(&[second_path.into()], Intent::ui(0));
        settle(&mut c, &mut service, fx);
        let second = c.active().unwrap();
        assert_ne!(first, second);
        let mut headers = c.tab(first).unwrap().mail.as_ref().unwrap().headers.clone();
        headers.subject = "Only first".into();
        c.edit_headers(first, headers).unwrap();
        assert_eq!(c.tab(second).unwrap().mail.as_ref().unwrap().headers.subject, "Hello");
        assert!(!c.tab(second).unwrap().dirty());
    }

    #[test]
    fn unchanged_open_save_and_reopen_is_an_equal_document() {
        let (mut c, _, tab) = opened(LETTER);
        let original = c.tab(tab).unwrap().mail.as_ref().unwrap().saved.clone();
        let fx = execute(&mut c, tab, "file.save");
        let text = fx.iter().find_map(|effect| match effect { Effect::SaveMail { text, .. } => Some(text), _ => None }).unwrap();
        assert_eq!(text.as_bytes(), maildoc::write(&original).unwrap().as_bytes());
        let directory = tempfile::tempdir().unwrap();
        let written = directory.path().join("letter.md");
        std::fs::write(&written, text).unwrap();
        assert_eq!(std::fs::read(&written).unwrap(), maildoc::write(&original).unwrap().into_bytes());
        let text = std::fs::read_to_string(written).unwrap();
        let (reopened, _, tab) = opened(&text);
        assert_eq!(reopened.tab(tab).unwrap().mail.as_ref().unwrap().saved, original);
    }

    #[test]
    fn a_bulk_header_update_still_undoes_one_field_at_a_time() {
        let (mut c, _, tab) = opened(LETTER);
        let mut headers = c.tab(tab).unwrap().mail.as_ref().unwrap().headers.clone();
        headers.to = "carol@example.org".into();
        headers.subject = "Changed subject".into();
        c.edit_headers(tab, headers).unwrap();
        execute(&mut c, tab, "edit.undo");
        let mail = c.tab(tab).unwrap().mail.as_ref().unwrap();
        assert_eq!(mail.headers.subject, "Hello");
        assert_eq!(mail.headers.to, "carol@example.org");
        execute(&mut c, tab, "edit.undo");
        assert!(!c.tab(tab).unwrap().dirty());
    }

    #[test]
    fn whitespace_only_header_edits_are_dirty_until_saved() {
        let (mut c, _, tab) = opened(LETTER);
        let mut headers = c.tab(tab).unwrap().mail.as_ref().unwrap().headers.clone();
        headers.to.push(' ');
        c.edit_headers(tab, headers).unwrap();
        assert!(c.tab(tab).unwrap().dirty());
        let fx = execute(&mut c, tab, "file.save");
        let Effect::SaveMail { path, text, send, command, .. } = fx.into_iter().find(|e| matches!(e, Effect::SaveMail { .. })).unwrap() else { unreachable!() };
        assert_eq!(maildoc::parse(&text).unwrap().to, vec!["bob@example.org"]);
        c.on_mail_saved(tab, path, text, send, command, Ok(()));
        assert!(!c.tab(tab).unwrap().dirty());
        let mail = c.tab(tab).unwrap().mail.as_ref().unwrap();
        assert_eq!(mail.saved_headers.to, "bob@example.org ");
        assert_eq!(mail.saved.to, vec!["bob@example.org"]);
        let mut headers = mail.headers.clone();
        headers.to.push(' ');
        c.edit_headers(tab, headers).unwrap();
        assert!(c.tab(tab).unwrap().dirty());
        execute(&mut c, tab, "edit.undo");
        assert!(!c.tab(tab).unwrap().dirty());
    }

    #[test]
    fn an_old_daemon_cannot_silently_drop_the_initial_mail_body() {
        let mut c = Controller::new(Config::default(), 1, true);
        let mut service = FakeEditd::new("0000e1e1");
        service.create(Some(PATH), LETTER);
        let fx = c.open_paths(&[PATH.into()], Intent::ui(0));
        settle_with_seed(&mut c, &mut service, fx, false);
        let tab = c.tab(c.active().unwrap()).unwrap();
        assert!(tab.mail.is_none());
        assert!(tab.document_notice.as_deref().unwrap().contains("editd 0.3.0"));
        assert_eq!(text_string(tab.mirror.as_ref().unwrap().text()), LETTER);
        assert!(!c.can_send(tab.id));
    }

    #[cfg(unix)]
    #[test]
    fn an_unrepresentable_attachment_path_is_refused_without_poisoning_state() {
        use std::os::unix::ffi::OsStringExt;
        let (mut c, _, tab) = opened(LETTER);
        let mut headers = c.tab(tab).unwrap().mail.as_ref().unwrap().headers.clone();
        headers.attachments.push(std::ffi::OsString::from_vec(b"/tmp/report-\xff".to_vec()).into());
        let expected = maildoc::write(&headers.document("Body".into())).unwrap_err().to_string();
        assert_eq!(c.edit_headers(tab, headers).unwrap_err(), expected);
        assert_eq!(c.tab(tab).unwrap().mail.as_ref().unwrap().notice.as_deref(), Some(expected.as_str()));
        assert!(!c.tab(tab).unwrap().dirty());
        assert!(serde_json::to_value(c.state_reply(tab, true).unwrap()).is_ok());
    }

    #[test]
    fn quitting_cannot_silently_drop_unsaved_headers() {
        let (mut c, _, tab) = opened(LETTER);
        let mut headers = c.tab(tab).unwrap().mail.as_ref().unwrap().headers.clone();
        headers.subject = "Unsaved".into();
        c.edit_headers(tab, headers).unwrap();
        let fx = execute(&mut c, tab, "file.exit");
        assert!(!fx.iter().any(|effect| matches!(effect, Effect::Quit)));
        assert!(fx.iter().any(|effect| matches!(effect, Effect::Respond { rc: 10, .. })));
        let fx = c.on_bus_command(BusCommand {
            id: 78,
            command: "app.quit".into(),
            body: "{}".into(),
            caller_key: "local:test".into(),
        });
        assert!(!fx.iter().any(|effect| matches!(effect, Effect::Quit)));
        assert!(fx.iter().any(|effect| matches!(effect, Effect::Respond { rc: 10, .. })));
    }

    #[test]
    fn parked_save_drains_the_latest_body_and_has_no_snapshot() {
        let (mut c, mut service, tab) = opened(LETTER);
        let mut fx = c.on_editor(tab, EditorMsg::Command(EditCommand::Insert("First ".into())));
        execute(&mut c, tab, "file.send");
        assert!(c.mail_save_parked(tab));
        assert!(c.tab(tab).unwrap().mail.as_ref().unwrap().saving_body.is_none());
        fx.extend(c.on_editor(tab, EditorMsg::Command(EditCommand::Insert("Last ".into()))));
        let fx = settle(&mut c, &mut service, fx);
        assert!(!c.mail_save_parked(tab));
        assert!(fx.iter().any(|e| matches!(e, Effect::SaveMail { text, .. } if maildoc::parse(text).unwrap().body == "First Last Body")));
    }

    #[test]
    fn close_exit_and_connection_loss_cancel_parked_saves() {
        for action in ["file.close", "file.exit", "app.quit", "disconnect", "suspect"] {
            let (mut c, _, tab) = opened(LETTER);
            c.on_editor(tab, EditorMsg::Command(EditCommand::Insert("Unsaved ".into())));
            execute(&mut c, tab, "file.send");
            assert!(c.x[&tab].mail_save.is_some());
            c.x.get_mut(&tab).unwrap().close_after_save = Some(Intent::ui(tab));
            let fx = match action {
                "app.quit" => c.on_bus_command(BusCommand { id:78, command:"app.quit".into(), body:"{}".into(), caller_key:"local:test".into() }),
                "disconnect" => c.on_incoming(Incoming::Connection { up:false }),
                "suspect" => c.on_incoming(Incoming::Connection { up:true }),
                _ => c.on_action(Some(tab), ActionId::from_id(action).unwrap(), Intent::ui(tab)),
            };
            assert!(c.x[&tab].mail_save.is_none(), "{action}");
            assert!(c.x[&tab].close_after_save.is_none(), "{action}");
            let mail = c.tab(tab).unwrap().mail.as_ref().unwrap();
            assert!(!mail.saving && !mail.sending && mail.saving_body.is_none());
            assert!(mail.notice.as_deref().unwrap().contains("cancelled"));
            assert!(fx.iter().any(|e| matches!(e, Effect::Respond { id:77, rc:10, .. })));
            assert!(!fx.iter().any(|e| matches!(e, Effect::Quit)));
            if action == "file.close" { assert!(fx.iter().any(|e| matches!(e, Effect::Prompt(Prompt::CloseDirty { .. })))); }
        }
    }

    #[test]
    fn clean_quit_force_closes_only_mail_scratch_buffers_before_quit() {
        for action in ["file.exit", "app.quit"] {
            let (mut c, _, tab) = opened(LETTER);
            let buffer = c.tab(tab).unwrap().mirror.as_ref().unwrap().buffer().to_owned();
            let fx = if action == "file.exit" { execute(&mut c, tab, action) } else {
                c.on_bus_command(BusCommand { id:78, command:"app.quit".into(), body:"{}".into(), caller_key:"local:test".into() })
            };
            let close = fx.iter().position(|e| matches!(e, Effect::Send { out, .. } if out.command == "edit.close" && serde_json::from_str::<Value>(&out.body).unwrap() == json!({"buffer":buffer,"force":true,"origin":UI_ORIGIN}))).unwrap();
            assert!(close < fx.iter().position(|e| matches!(e, Effect::Quit)).unwrap());
        }
    }

    #[test]
    fn lost_scratch_reseeds_from_current_or_detached_text() {
        let (mut c, mut service, tab) = opened(LETTER);
        let fx = c.on_editor(tab, EditorMsg::Command(EditCommand::Insert("Unsaved ".into())));
        settle(&mut c, &mut service, fx);
        let mut fx = Vec::new();
        c.send_open(tab, None, true, &mut fx);
        assert!(fx.iter().any(|e| matches!(e, Effect::Send { out, .. } if out.command == "edit.open" && serde_json::from_str::<Value>(&out.body).unwrap()["text"] == "Unsaved Body")));
        settle(&mut c, &mut service, fx);
        assert_eq!(text_string(c.tab(tab).unwrap().mirror.as_ref().unwrap().text()), "Unsaved Body");
        assert!(c.tab(tab).unwrap().dirty());
    }

    #[test]
    fn saved_body_uses_the_same_canonical_form_as_disk_and_reseed() {
        let (mut c, mut service, tab) = opened(LETTER);
        c.tab_mut(tab).unwrap().editor.sel = Selection { anchor:4, head:4 };
        let fx = c.on_editor(tab, EditorMsg::Command(EditCommand::Insert("\r\n \t".into())));
        settle(&mut c, &mut service, fx);
        let fx = execute(&mut c, tab, "file.save");
        let Effect::SaveMail { path, text, send, command, .. } = fx.into_iter().find(|e| matches!(e, Effect::SaveMail { .. })).unwrap() else { unreachable!() };
        let body = maildoc::parse(&text).unwrap().body;
        assert_eq!(body, "Body");
        c.on_mail_saved(tab, path, text, send, command, Ok(()));
        assert_eq!(c.tab(tab).unwrap().mail.as_ref().unwrap().saved_body, body);
        assert!(!c.tab(tab).unwrap().dirty());
        let mut fx = Vec::new();
        c.send_open(tab, None, true, &mut fx);
        settle(&mut c, &mut service, fx);
        assert!(!c.tab(tab).unwrap().dirty());
    }

    #[test]
    fn mail_close_after_save_waits_for_document_write_not_scratch_cleanliness() {
        let (mut c, _, tab) = opened(LETTER);
        c.x.get_mut(&tab).unwrap().close_after_save = Some(Intent::ui(tab));
        let mut fx = Vec::new();
        c.drive(tab, editor_model::mirror::Step::default(), &mut fx);
        assert!(c.x[&tab].close_after_save.is_some());
        assert!(!fx.iter().any(|e| matches!(e, Effect::Send { out, .. } if out.command == "edit.close")));
    }

    #[test]
    fn header_previews_coalesce_per_field_and_committed_history_is_bounded() {
        let (mut c, _, tab) = opened(LETTER);
        for subject in ["H", "He", "Hey"] {
            let mut headers = c.tab(tab).unwrap().mail.as_ref().unwrap().headers.clone();
            headers.subject = subject.into();
            c.preview_headers(tab, headers).unwrap();
        }
        assert_eq!(c.tab(tab).unwrap().mail.as_ref().unwrap().undo.len(), 1);
        assert!(c.commit_headers(tab));
        execute(&mut c, tab, "edit.undo");
        assert_eq!(c.tab(tab).unwrap().mail.as_ref().unwrap().headers.subject, "Hello");
        for n in 0..300 {
            let mut headers = c.tab(tab).unwrap().mail.as_ref().unwrap().headers.clone();
            headers.subject = format!("{n}");
            c.edit_headers(tab, headers).unwrap();
        }
        assert_eq!(c.tab(tab).unwrap().mail.as_ref().unwrap().undo.len(), 256);
    }

    #[test]
    fn bom_mail_is_text_with_an_explanation() {
        // FakeEditd strips a file BOM but always reports bom:false. Supply
        // the real daemon's open metadata before its snapshot arrives.
        let mut c = Controller::new(Config::default(), 1, true);
        let mut service = FakeEditd::new("0000e1e1");
        service.create(Some(PATH), &format!("\u{feff}{LETTER}"));
        let fx = c.open_paths(&[PATH.into()], Intent::ui(0));
        for effect in fx {
            if let Effect::Send { req, out } = effect {
                let mut answer = service.handle("local:ced", &out.command, &serde_json::from_str(&out.body).unwrap());
                answer.body["bom"] = json!(true);
                let fx = c.on_incoming(Incoming::Reply { req, rc:answer.rc, body:answer.body.to_string() });
                settle(&mut c, &mut service, fx);
            }
        }
        let t = c.tab(c.active().unwrap()).unwrap();
        assert!(t.mail.is_none());
        assert!(t.document_notice.as_deref().unwrap().contains("BOM"));
    }

    #[test]
    fn relative_open_path_keeps_disk_conflict_protection() {
        let (mut c, _, tab) = opened(LETTER);
        c.tab_mut(tab).unwrap().path = Some("letter.md".into());
        let fx = execute(&mut c, tab, "file.save");
        assert!(fx.iter().any(|e| matches!(e, Effect::SaveMail { expected:Some(expected), .. } if expected == LETTER)));
    }

    #[test]
    fn validation_refusals_have_argument_codes_and_busy_has_conflict() {
        let (mut c, _, tab) = opened(LETTER);
        let mut headers = c.tab(tab).unwrap().mail.as_ref().unwrap().headers.clone();
        headers.to = "invalid".into();
        c.edit_headers(tab, headers).unwrap();
        assert!(execute(&mut c, tab, "file.send").iter().any(|e| matches!(e, Effect::Respond { body, .. } if serde_json::from_str::<Value>(body).unwrap()["error_code"] == code::INVALID_ARGUMENT)));
        let mut fx = Vec::new();
        Controller::mail_answer(Some((77,"file.send".into())), Err("invalid address".into()), &mut fx);
        Controller::mail_answer(Some((78,"file.send".into())), Err("a mail save or Send is already in flight".into()), &mut fx);
        assert!(fx.iter().any(|e| matches!(e, Effect::Respond { id:77, body, .. } if serde_json::from_str::<Value>(body).unwrap()["error_code"] == code::INVALID_ARGUMENT)));
        assert!(fx.iter().any(|e| matches!(e, Effect::Respond { id:78, body, .. } if serde_json::from_str::<Value>(body).unwrap()["error_code"] == code::CONFLICT)));
        assert_eq!(error_code("disk full"), code::INTERNAL);
    }
}
