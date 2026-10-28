// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::mail::{Headers, Kind, MailState};

impl Tab {
    pub fn dirty(&self) -> bool {
        if let Some(mail) = &self.mail {
            let body = self.mirror.as_ref().map(|m| text_string(m.text())).unwrap_or_else(|| mail.saved.body.clone());
            mail.headers.document(body.replace("\r\n", "\n").trim_end().to_owned()) != mail.saved
        } else {
            self.mirror.as_ref().is_some_and(|m| m.meta().dirty)
        }
    }
}

impl Controller {
    pub fn can_send(&self, tab: TabId) -> bool {
        self.tab(tab).is_some_and(|t| t.mail.as_ref().is_some_and(|mail| !mail.sending && !mail.saving)
            && t.mirror.as_ref().is_some_and(|m| matches!(m.phase(), Phase::Live)))
    }

    /// One field's committed edit is one header undo step. Body undo remains
    /// owned by edit; header history is exhausted before body undo.
    pub fn edit_headers(&mut self, tab: TabId, headers: Headers) -> Result<(), String> {
        let mail = self.tab_mut(tab).and_then(|t| t.mail.as_mut()).ok_or("not a mail document")?;
        if mail.sending || mail.saving { return Err("the mail document is busy".into()); }
        if headers.in_reply_to != mail.headers.in_reply_to || headers.references != mail.headers.references {
            return Err("thread headers are read-only".into());
        }
        if headers != mail.headers {
            mail.undo.push(std::mem::replace(&mut mail.headers, headers));
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
        if t.mirror.as_ref().unwrap().meta().bom { return false; }
        let text = text_string(t.mirror.as_ref().unwrap().text());
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
        let document = mail.headers.document(text_string(m.text()).replace("\r\n", "\n").trim_end().to_owned());
        let text = match maildoc::write(&document) {
            Ok(text) => text,
            Err(error) => { let message = error.to_string(); mail.notice = Some(message.clone()); return Err(message); }
        };
        let path = path.or_else(|| t.path.clone()).ok_or("save the document to a file first")?;
        let path = std::path::absolute(&path).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        let expected = (t.path.as_deref() == Some(&path)).then(|| mail.disk.clone());
        mail.saving = true;
        mail.sending = send;
        mail.notice = send.then(|| "Sending…".to_owned());
        let command = self.op_cmd.take();
        let effect = Effect::SaveMail { tab, path, text, expected, send, command };
        if idle { fx.push(effect); } else { self.x.get_mut(&tab).unwrap().mail_save = Some(effect); }
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
                Err(message) => (10, refusal(code::CONFLICT, message, None)),
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
        let mut queue: VecDeque<_> = fx.into();
        let mut rest = Vec::new();
        let mut steps = 0;
        while let Some(effect) = queue.pop_front() {
            steps += 1;
            assert!(steps < 1000, "controller did not settle");
            if let Effect::Send { req, out } = effect {
                let mut answer = service.handle("local:ced", &out.command, &serde_json::from_str(&out.body).unwrap());
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
        let fx = c.on_mail_saved(tab, path, text, send, command, Ok(()));
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
}
