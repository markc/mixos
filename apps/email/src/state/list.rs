// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The message list's search, the folder tree's actions and the drag of
//! emails onto folders: the engine half of the list header, the tree's
//! context menus and the drop targets. The view reports, these decide.

use serde::Serialize;
use serde_json::{Value, json};
use toolkit::datatable::verbs::{self as table_verbs, Refusal, Request};
use toolkit::datatable::{SelectionMode, TableEvent};

use super::{AccountView, Form, Node, Phase, State};
use crate::verbs::Write;
use crate::work::{IdsFor, SELECT_CAP, Work};

/// Which side panes the window shows (persisted with its shape).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Panes {
    /// The account tree.
    pub folder: bool,
    /// The reader.
    pub message: bool,
}

impl Default for Panes {
    fn default() -> Self {
        Self {
            folder: true,
            message: true,
        }
    }
}

/// Where the search looks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// The selected mailbox.
    #[default]
    Folder,
    /// Every mailbox of the account.
    All,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Folder => "folder",
            Scope::All => "all",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Scope::Folder, Scope::All]
            .into_iter()
            .find(|s| s.name() == name)
    }
}

/// The list's server-side search.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Search {
    /// What the field holds.
    pub text: String,
    /// What the shown page was asked for (the text once its debounce ran).
    pub applied: String,
    pub scope: Scope,
}

impl Search {
    /// The text the server matches, when there is one.
    pub fn query(&self) -> Option<String> {
        Some(self.applied.trim().to_owned()).filter(|t| !t.is_empty())
    }

    /// Whether the shown page is a search's result.
    pub fn active(&self) -> bool {
        self.query().is_some()
    }

    /// Whether the list spans every folder (a search of all of them).
    pub fn all(&self) -> bool {
        self.scope == Scope::All && self.active()
    }
}

/// A folder dialog's task.
#[derive(Clone, Debug, PartialEq)]
pub enum FolderAction {
    /// A new mailbox under `parent` (top level when `None`).
    New {
        parent: Option<String>,
    },
    Rename {
        id: String,
    },
    /// Its emails move to the Trash, then it goes.
    Delete {
        id: String,
    },
    /// Destroy every email in it.
    Empty {
        id: String,
    },
    /// Mark many read: a whole mailbox (`id`), or a search's results (`ids`).
    MarkRead {
        id: Option<String>,
        ids: Vec<String>,
    },
}

/// Marking more emails read than this asks first.
pub const MARK_CONFIRM_ABOVE: u64 = 500;

/// `n` with its thousands grouped: 2866 is "2,866".
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A folder dialog: a name to give, or a confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderForm {
    pub account: String,
    pub generation: u64,
    pub action: FolderAction,
    /// The name typed (New, Rename), or the folder's name (Delete, Empty).
    pub name: String,
    /// The emails a Delete moves or an Empty destroys.
    pub count: u64,
    pub error: Option<String>,
}

/// The mailbox roles that are the server's, never renamed or deleted.
pub const FIXED_ROLES: [&str; 7] = [
    "inbox", "sent", "drafts", "trash", "junk", "spam", "archive",
];

/// Why `name` cannot be a mailbox named among `siblings` (other mailboxes
/// under the same parent), if it cannot.
pub fn name_refusal(name: &str, siblings: &[&str]) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() {
        Some("folder-name-needed")
    } else if name.contains('/') {
        Some("folder-name-slash")
    } else if siblings.iter().any(|s| s.trim().eq_ignore_ascii_case(name)) {
        Some("folder-name-taken")
    } else {
        None
    }
}

impl AccountView {
    /// Run the search as it stands: its first page, nothing selected.
    pub(super) fn run_search(&mut self, limit: u32, work: &mut Vec<Work>) {
        self.mail.search.applied = self.mail.search.text.trim().to_owned();
        self.mail.everything = None;
        self.mail.table.page = 0;
        self.mail.table.selection.clear();
        self.mail.table.focus = None;
        if self.phase != Phase::Ready {
            return;
        }
        if let Some(mailbox) = self.mail.selected.clone() {
            self.mail.busy = true;
            self.fetch_emails(mailbox, limit, work);
        }
    }

    /// The mailbox `id`'s name, for the Folder column.
    pub fn mailbox_name(&self, id: &str) -> Option<&str> {
        self.mail
            .mailboxes
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.name.as_str())
    }
}

impl State {
    /// A keystroke in `account`'s search field: the query waits for a
    /// pause (the debounce), and only the last keystroke's runs.
    pub fn search_typed(&mut self, account: &str, text: String) {
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        view.mail.search.text = text;
        view.tickets.search += 1;
        let ticket = view.tickets.search;
        self.work.push(Work::SearchDue {
            account: account.to_owned(),
            ticket,
        });
    }

    /// Search `account`'s list for `text` in `scope` at once (the Bus,
    /// a scope change, a clear).
    pub fn set_search(&mut self, account: &str, text: Option<String>, scope: Option<Scope>) {
        let limit = self.settings.mail.page_size;
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        if let Some(text) = text {
            view.mail.search.text = text;
        }
        if let Some(scope) = scope {
            view.mail.search.scope = scope;
        }
        // Any keystroke still waiting is overtaken.
        view.tickets.search += 1;
        view.run_search(limit, &mut self.work);
    }

    /// Empty the current account's search field and show the folder again.
    pub fn clear_search(&mut self) {
        if let Some(account) = self.selected.clone() {
            self.set_search(&account, Some(String::new()), None);
        }
    }

    /// The current account's search looks in `scope`.
    pub fn search_scope(&mut self, scope: Scope) {
        if let Some(account) = self.selected.clone() {
            self.set_search(&account, None, Some(scope));
        }
    }

    /// Whether the current account's search looks in `scope`.
    pub fn searching_in(&self, scope: Scope) -> bool {
        self.current().is_some_and(|v| v.mail.search.scope == scope)
    }

    /// Ask the view to give the search field the keyboard.
    pub fn focus_search(&mut self) {
        self.ui.focus_search = true;
    }

    /// The node the folder commands act on: the one a context menu was
    /// opened on, else the tree's selection.
    pub fn target_node(&self) -> Option<Node> {
        self.ui.menu_node.clone().or_else(|| self.selected_node())
    }

    /// The target mailbox, with its account's view.
    fn target_mailbox(&self) -> Option<(&AccountView, &jmap::Mailbox)> {
        let Some(Node::Mailbox(account, id)) = self.target_node() else {
            return None;
        };
        let view = self.account(&account)?;
        let mailbox = view.mail.mailboxes.iter().find(|m| m.id == id)?;
        Some((view, mailbox))
    }

    /// The target is a mailbox of a connected account.
    pub fn folder_targeted(&self) -> bool {
        self.ui.dialog.is_none()
            && self.form.is_none()
            && self
                .target_mailbox()
                .is_some_and(|(v, _)| v.phase == Phase::Ready)
    }

    /// The target mailbox can be renamed or deleted: it has no role.
    pub fn folder_editable(&self) -> bool {
        self.folder_targeted() && self.target_mailbox().is_some_and(|(_, m)| !fixed(m))
    }

    /// The target mailbox has unread email.
    pub fn folder_unread(&self) -> bool {
        self.folder_targeted()
            && self
                .target_mailbox()
                .is_some_and(|(_, m)| m.unread_emails > 0)
    }

    /// The target mailbox is a Trash or Junk with email in it.
    pub fn folder_emptiable(&self) -> bool {
        self.folder_targeted()
            && self
                .target_mailbox()
                .is_some_and(|(_, m)| emptiable(m) && m.total_emails > 0)
    }

    /// Why the folder command `id` is off for the target, for its tooltip.
    pub fn folder_why(&self, id: &str) -> Option<String> {
        let (_, mailbox) = self.target_mailbox()?;
        match id {
            "folder.rename" | "folder.delete" if fixed(mailbox) => {
                Some(crate::label("folder-role-fixed"))
            }
            "folder.mark_read" if mailbox.unread_emails == 0 => {
                Some(crate::label("folder-all-read"))
            }
            "folder.empty" if mailbox.total_emails == 0 => {
                Some(crate::label("folder-empty-already"))
            }
            _ => None,
        }
    }

    /// The commands a context menu on `node` lists (the registry's
    /// enablement decides which can run).
    pub fn node_menu(&self, node: &Node) -> Vec<&'static str> {
        match node {
            Node::Mailbox(account, id) => {
                let mut ids = vec![
                    "folder.new",
                    "folder.rename",
                    "folder.delete",
                    "folder.mark_read",
                ];
                let empty = self
                    .account(account)
                    .and_then(|v| v.mail.mailboxes.iter().find(|m| m.id == *id))
                    .is_some_and(emptiable);
                if empty {
                    ids.push("folder.empty");
                }
                ids.push("folder.refresh");
                ids
            }
            Node::Account(_) => vec![
                "account.compose",
                "account.reconnect",
                "account.settings",
                "folder.refresh",
            ],
            Node::Calendars(_) | Node::Calendar(..) => {
                vec!["calendar.new_event", "folder.refresh"]
            }
            Node::Books(_) | Node::Book(..) => vec!["contacts.new_contact", "folder.refresh"],
        }
    }

    /// A right-click on `node`: it is selected (unless it already is) and
    /// becomes what the folder commands act on.
    pub fn context_node(&mut self, node: &crate::state::Node) {
        if self.selected_node().as_ref() != Some(node) {
            self.select_node(node);
        }
        self.ui.menu_node = Some(node.clone());
    }

    fn done_with_node(&mut self) {
        self.ui.menu_node = None;
    }

    fn folder_form(&mut self, action: FolderAction, name: String, count: u64) {
        let Some(Node::Mailbox(account, _) | Node::Account(account)) = self.target_node() else {
            return;
        };
        let generation = self.generation_of(&account);
        self.open_form(Form::Folder(FolderForm {
            account,
            generation,
            action,
            name,
            count,
            error: None,
        }));
        self.done_with_node();
    }

    /// Folder › New Subfolder…: name a mailbox under the target.
    pub fn folder_new(&mut self) {
        let parent = self.target_mailbox().map(|(_, m)| m.id.clone());
        self.folder_form(FolderAction::New { parent }, String::new(), 0);
    }

    /// Folder › Rename…
    pub fn folder_rename(&mut self) {
        let Some((_, mailbox)) = self.target_mailbox() else {
            return;
        };
        let (id, name) = (mailbox.id.clone(), mailbox.name.clone());
        self.folder_form(FolderAction::Rename { id }, name, 0);
    }

    /// Folder › Delete Folder…: confirm, naming how many emails go to Trash.
    pub fn folder_delete(&mut self) {
        let Some((_, mailbox)) = self.target_mailbox() else {
            return;
        };
        let (id, name, count) = (
            mailbox.id.clone(),
            mailbox.name.clone(),
            mailbox.total_emails,
        );
        self.folder_form(FolderAction::Delete { id }, name, count);
    }

    /// Folder › Empty…: confirm, naming how many emails are destroyed.
    pub fn folder_empty(&mut self) {
        let Some((_, mailbox)) = self.target_mailbox() else {
            return;
        };
        let (id, name, count) = (
            mailbox.id.clone(),
            mailbox.name.clone(),
            mailbox.total_emails,
        );
        self.folder_form(FolderAction::Empty { id }, name, count);
    }

    /// Folder › Mark Folder as Read: every unread email in it, on the
    /// server, a batch at a time.
    ///
    /// On the folder the list shows while it is searched, it marks the
    /// search's results instead (as Thunderbird does): their unread ids are
    /// fetched first. More than [`MARK_CONFIRM_ABOVE`] asks, Cancel first.
    pub fn folder_mark_read(&mut self) {
        let Some((view, mailbox)) = self.target_mailbox() else {
            self.done_with_node();
            return;
        };
        let (account, id) = (view.entry.id.clone(), mailbox.id.clone());
        let (name, unread) = (mailbox.name.clone(), mailbox.unread_emails);
        let searched = view.mail.selected.as_deref() == Some(id.as_str())
            && view.mail.search.active();
        if searched {
            let (text, all) = (view.mail.search.query(), view.mail.search.all());
            self.query_ids(&account, Some(id).filter(|_| !all), text, true, IdsFor::MarkRead);
        } else if unread > MARK_CONFIRM_ABOVE {
            self.folder_form(
                FolderAction::MarkRead {
                    id: Some(id.clone()),
                    ids: Vec::new(),
                },
                name,
                unread,
            );
        } else {
            self.mark_shown_read(&account, None);
            self.mailbox_write(&account, None, Write::MarkMailboxRead { id });
        }
        self.done_with_node();
    }

    /// The shown rows read at once (`ids`: only those; none: every row), so
    /// the list stops being bold before the server answers.
    fn mark_shown_read(&mut self, account: &str, ids: Option<&[String]>) {
        if let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) {
            let shown: Vec<String> = view
                .mail
                .emails
                .iter()
                .map(|e| e.id.clone())
                .filter(|id| ids.is_none_or(|ids| ids.contains(id)))
                .collect();
            view.set_keyword(&shown, "$seen", true);
        }
    }

    /// Ask for every id the list's query has (`mailbox` none: every
    /// mailbox), for `purpose`; only the newest answer is used.
    fn query_ids(
        &mut self,
        account: &str,
        mailbox: Option<String>,
        text: Option<String>,
        unread: bool,
        purpose: IdsFor,
    ) {
        let Some(view) = self.accounts.iter_mut().find(|v| v.entry.id == account) else {
            return;
        };
        view.tickets.ids += 1;
        let ticket = view.tickets.ids;
        self.work.push(Work::QueryIds {
            account: account.to_owned(),
            ticket,
            mailbox,
            text,
            unread,
            cap: SELECT_CAP,
            purpose,
        });
    }

    /// "Select all N in folder": every id of the list's query, every page.
    pub(super) fn select_everything(&mut self, account: &str) {
        let Some(view) = self.account(account) else {
            return;
        };
        let search = &view.mail.search;
        let mailbox = view.mail.selected.clone().filter(|_| !search.all());
        let text = search.query();
        self.query_ids(account, mailbox, text, false, IdsFor::Select);
    }

    /// An id query answered.
    pub(super) fn ids_arrived(
        &mut self,
        tag: &super::Tag,
        ticket: u64,
        purpose: IdsFor,
        result: Result<Vec<String>, jmap::Error>,
    ) {
        let current = self.accounts.iter_mut().find(|v| {
            v.entry.id == tag.account && v.generation == tag.generation && v.tickets.ids == ticket
        });
        let Some(view) = current else {
            return;
        };
        let ids = match result {
            Ok(ids) => ids,
            Err(error) => {
                self.status = error.to_string();
                return;
            }
        };
        match purpose {
            IdsFor::Select => {
                view.mail.table.selection = view.mail.emails.iter().map(|e| e.id.clone()).collect();
                view.mail.everything = Some(ids);
            }
            IdsFor::MarkRead if ids.is_empty() => {}
            IdsFor::MarkRead if ids.len() as u64 > MARK_CONFIRM_ABOVE => {
                let account = tag.account.clone();
                let generation = view.generation;
                let count = ids.len() as u64;
                let name = view.mail.search.applied.clone();
                self.open_form(Form::Folder(FolderForm {
                    account,
                    generation,
                    action: FolderAction::MarkRead { id: None, ids },
                    name,
                    count,
                    error: None,
                }));
            }
            IdsFor::MarkRead => {
                let account = tag.account.clone();
                self.mark_shown_read(&account, Some(&ids));
                let write = Write::Keywords {
                    ids,
                    read: Some(true),
                    flagged: None,
                };
                self.mailbox_write(&account, None, write);
            }
        }
    }

    /// The emails the message actions act on: every id "Select all N"
    /// fetched, else the list's selection, else the open email.
    pub fn acting_on(&self) -> Option<(String, Vec<String>)> {
        let view = self.current()?;
        let ids = match (&view.mail.everything, &view.mail.open) {
            (Some(all), _) => all.clone(),
            (None, _) if !view.mail.table.selection.is_empty() => view.mail.table.selection.clone(),
            (None, Some(open)) => vec![open.id.clone()],
            (None, None) => return None,
        };
        (!ids.is_empty()).then(|| (view.entry.id.clone(), ids))
    }

    /// How many emails the list's selection holds (every page's, once
    /// "Select all N" has run).
    pub fn selection_count(&self, view: &AccountView) -> usize {
        view.mail
            .everything
            .as_ref()
            .map_or(view.mail.table.selection.len(), Vec::len)
    }

    /// Edit › Select All: every row of the list's page.
    pub fn select_all_mail(&mut self) {
        let Some(view) = self.current() else {
            return;
        };
        let account = view.entry.id.clone();
        let keys: Vec<String> = view.mail.emails.iter().map(|e| e.id.clone()).collect();
        if let Some(event) = view.mail.table.select_all(&keys) {
            self.mail_table(&account, event);
        }
    }

    /// Show or hide the folder pane (`folder`) and the reader (`message`);
    /// the keyboard leaves a pane that goes, for the list.
    pub fn set_panes(&mut self, folder: Option<bool>, message: Option<bool>) {
        let before = self.ui.panes;
        if let Some(folder) = folder {
            self.ui.panes.folder = folder;
        }
        if let Some(message) = message {
            self.ui.panes.message = message;
        }
        if self.ui.panes != before {
            if (before.folder && !self.ui.panes.folder) || (before.message && !self.ui.panes.message) {
                self.ui.focus_list = true;
            }
            self.config_dirty = true;
        }
    }

    /// View › Layout › Folder Pane.
    pub fn toggle_folder_pane(&mut self) {
        self.set_panes(Some(!self.ui.panes.folder), None);
    }

    /// View › Layout › Message Pane.
    pub fn toggle_message_pane(&mut self) {
        self.set_panes(None, Some(!self.ui.panes.message));
    }

    /// Refresh the target's account.
    pub fn folder_refresh(&mut self) {
        if let Some(node) = self.target_node() {
            let account = node.account().to_owned();
            self.refresh_account(&account);
        }
        self.done_with_node();
    }

    /// The target account, from an account-root context menu or the tree.
    fn target_account(&self) -> Option<String> {
        self.target_node().map(|n| n.account().to_owned())
    }

    /// Account › Compose from This Account.
    pub fn account_compose(&mut self) {
        if let Some(account) = self.target_account() {
            self.open_compose(Some(&account), jmap::Composed::default());
        }
        self.done_with_node();
    }

    /// Account › Reconnect.
    pub fn account_reconnect(&mut self) {
        if let Some(account) = self.target_account() {
            self.reconnect(&account);
        }
        self.done_with_node();
    }

    /// Account › Account Settings…: Prefs on this account.
    pub fn account_settings(&mut self) {
        if let Some(account) = self.target_account() {
            self.work.push(Work::OpenPrefsAt { account });
        }
        self.done_with_node();
    }

    /// The target is an account (any phase): its own commands can run.
    pub fn account_targeted(&self) -> bool {
        self.ui.dialog.is_none()
            && self.form.is_none()
            && self
                .target_account()
                .is_some_and(|a| self.account(&a).is_some())
    }

    /// A folder-wide write on `account` (marking read, emptying, deleting
    /// to Trash), answered on the Bus under `reply` when one asked.
    pub fn mailbox_write(&mut self, account: &str, reply: Option<u64>, write: Write) {
        self.write_server(account.to_owned(), reply, write);
    }

    /// Move `ids` of `account` into `mailbox`: the one path `email.move`,
    /// the Move form and a drop onto a folder all take.
    pub fn move_emails(
        &mut self,
        account: &str,
        ids: Vec<String>,
        mailbox: String,
        reply: Option<u64>,
    ) {
        self.write_server(account.to_owned(), reply, Write::Move { ids, mailbox });
    }

    /// Why dropping the list's rows on `target` is refused, or `None` when
    /// it is a drop target: another mailbox of the list's own account.
    pub fn drop_refusal(&self, target: &Node) -> Option<&'static str> {
        let Node::Mailbox(account, id) = target else {
            return Some("drop-not-folder");
        };
        let view = self.current()?;
        if view.entry.id != *account {
            return Some("drop-other-account");
        }
        if !view.mail.search.all() && view.mail.selected.as_deref() == Some(id.as_str()) {
            return Some("drop-same-folder");
        }
        None
    }

    /// The list's rows `keys` were dropped on `target`: move them there.
    pub fn drop_on(&mut self, target: &Node, keys: Vec<String>) -> bool {
        if self.drop_refusal(target).is_some() || keys.is_empty() {
            return false;
        }
        let Node::Mailbox(account, id) = target else {
            return false;
        };
        // A page of an every-page selection carries them all.
        let keys = match self.account(account).and_then(|v| v.mail.everything.clone()) {
            Some(all) if keys.iter().all(|k| all.contains(k)) => all,
            _ => keys,
        };
        self.move_emails(account, keys, id.clone(), None);
        true
    }

    /// Submit a folder dialog: validate a name, or run the confirmed task.
    pub(super) fn submit_folder(&mut self, form: FolderForm) {
        let name = form.name.trim().to_owned();
        let write = match &form.action {
            FolderAction::New { parent } => Write::CreateMailbox {
                name,
                parent: parent.clone(),
            },
            FolderAction::Rename { id } => Write::RenameMailbox {
                id: id.clone(),
                name,
            },
            FolderAction::Delete { id } => Write::TrashMailbox { id: id.clone() },
            FolderAction::Empty { id } => Write::EmptyMailbox { id: id.clone() },
            FolderAction::MarkRead { id: Some(id), .. } => {
                Write::MarkMailboxRead { id: id.clone() }
            }
            FolderAction::MarkRead { id: None, ids } => Write::Keywords {
                ids: ids.clone(),
                read: Some(true),
                flagged: None,
            },
        };
        if let FolderAction::New { .. } | FolderAction::Rename { .. } = &form.action {
            // The other mailboxes under the same parent.
            let own = match &form.action {
                FolderAction::Rename { id } => Some(id.as_str()),
                _ => None,
            };
            let siblings: Vec<&str> = self
                .account(&form.account)
                .map(|view| {
                    let parent = parent_of(&form.action, view);
                    view.mail
                        .mailboxes
                        .iter()
                        .filter(|m| m.parent_id == parent && Some(m.id.as_str()) != own)
                        .map(|m| m.name.as_str())
                        .collect()
                })
                .unwrap_or_default();
            if let Some(key) = name_refusal(&form.name, &siblings) {
                if let Some(Form::Folder(open)) = &mut self.form {
                    open.error = Some(crate::label(key));
                }
                return;
            }
        }
        self.form = None;
        self.write_bound(form.account, None, form.generation, write);
    }

    /// The message list's table verbs (`email.table.*`) on `account`:
    /// the same events a person's clicks raise, then the table's state.
    pub fn table_verb(
        &mut self,
        account: &str,
        verb: &str,
        args: &Value,
    ) -> Result<Value, Refusal> {
        let request = {
            let view = self
                .account(account)
                .ok_or_else(|| Refusal::NotFound(format!("no account {account}")))?;
            let keys: Vec<String> = view.mail.emails.iter().map(|e| e.id.clone()).collect();
            let columns = super::list_columns(0.0, view.mail.search.all());
            let table = self.list_table(view);
            table_verbs::request(
                verb,
                args,
                &table_verbs::Table {
                    columns: &columns,
                    state: &table,
                    total: view.mail.total_rows(),
                    keys: &keys,
                    allow_unsorted: false,
                },
            )?
        };
        if let Request::Events(events) = request {
            for event in events {
                self.mail_table(account, event);
            }
        }
        Ok(self.table_state(account))
    }

    /// The list's table as the window shows it: the view's sort, page and
    /// selection with the remembered widths, hidden columns and density.
    pub fn list_table(&self, view: &AccountView) -> toolkit::datatable::TableState {
        let mut table = toolkit::datatable::TableState {
            widths: self.ui.list.widths.clone(),
            hidden: self.ui.list.hidden.clone(),
            density: self.ui.list.density,
            ..view.mail.table.clone()
        };
        if let Some(open) = &view.mail.open
            && table.selection.is_empty()
        {
            table.selection = vec![open.id.clone()];
        }
        table
    }

    /// `email.table.state`: the list's table and its rows.
    pub fn table_state(&self, account: &str) -> Value {
        let Some(view) = self.account(account) else {
            return Value::Null;
        };
        let all = view.mail.search.all();
        let columns = super::list_columns(0.0, all);
        let mut out = self.list_table(view).describe(
            &columns,
            view.mail.total_rows(),
            view.mail.emails.len(),
        );
        out["rows"] = view
            .mail
            .emails
            .iter()
            .map(|e| {
                let folder = e
                    .mailbox_ids
                    .as_ref()
                    .and_then(|ids| ids.keys().next())
                    .and_then(|id| view.mailbox_name(id));
                json!({
                    "key": e.id,
                    "cells": {
                        super::COLUMN_FROM: e.from_display(),
                        super::COLUMN_SUBJECT: e.subject,
                        super::COLUMN_FOLDER: folder,
                        super::COLUMN_DATE: e.received_at.as_ref().or(e.date.as_ref()),
                    },
                    "strong": !e.is_read(),
                    "flagged": e.is_flagged(),
                    "attachment": e.has_attachment == Some(true),
                })
            })
            .collect();
        out["account"] = json!(account);
        out["selected"] = json!(self.selection_count(view));
        out["selected_all"] = json!(view.mail.everything.is_some());
        out["search"] = json!({
            "text": view.mail.search.text,
            "applied": view.mail.search.applied,
            "scope": view.mail.search.scope.name(),
        });
        out
    }

    /// Apply one event of the list's table that the window keeps rather
    /// than the view: hidden columns and density are remembered.
    pub(super) fn list_preference(&mut self, event: &TableEvent) -> bool {
        match event {
            TableEvent::ColumnShown { .. } | TableEvent::DensityChanged(_) => {
                self.ui.list.apply(event, SelectionMode::Single);
                self.config_dirty = true;
                true
            }
            _ => false,
        }
    }
}

/// The parent a New or Rename lands under.
fn parent_of(action: &FolderAction, view: &AccountView) -> Option<String> {
    match action {
        FolderAction::New { parent } => parent.clone(),
        FolderAction::Rename { id } => view
            .mail
            .mailboxes
            .iter()
            .find(|m| m.id == *id)
            .and_then(|m| m.parent_id.clone()),
        _ => None,
    }
}

/// A mailbox the server names by role.
fn fixed(mailbox: &jmap::Mailbox) -> bool {
    mailbox
        .role
        .as_deref()
        .is_some_and(|role| FIXED_ROLES.contains(&role))
}

/// A Trash or a Junk folder.
fn emptiable(mailbox: &jmap::Mailbox) -> bool {
    matches!(mailbox.role.as_deref(), Some("trash" | "junk" | "spam"))
}
