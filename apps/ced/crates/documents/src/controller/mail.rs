// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::mail::{Headers, Kind, MailState};

impl Tab {
    pub fn dirty(&self) -> bool {
        if let Some(mail) = &self.mail {
            let body = self.mirror.as_ref().map(|m| text_string(m.text())).unwrap_or_else(|| mail.saved.body.clone());
            mail.headers.document(body) != mail.saved
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
        if t.classified || !t.mirror.as_ref().is_some_and(|m| matches!(m.phase(), Phase::Live)) { return false; }
        t.classified = true;
        let text = text_string(t.mirror.as_ref().unwrap().text());
        match crate::mail::classify(&text) {
            Kind::Mail(document) => {
                let buffer = t.mirror.as_ref().unwrap().buffer().to_string();
                t.mail = Some(MailState::new(document.clone(), text));
                t.mirror = None;
                self.x.get_mut(&tab).unwrap().fill = Some((document.body, Intent::ui(tab)));
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
        if !m.is_idle() || !matches!(m.phase(), Phase::Live) { return Err("wait for the body edits to finish before saving or sending".into()); }
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
        fx.push(Effect::SaveMail { tab, path, text, expected, send, command });
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
