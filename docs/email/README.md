---
title: email
layout: page
---

# email — the MixOS JMAP mail client

`email` (`dev.mixos.email`) is the reserved MixOS mail client: a **pure
JMAP client** that speaks HTTPS to any RFC 8620/8621-shaped server, whether
that is the mesh's own maild or someone else's server. It holds **any
number of accounts**, shown as a tree in the Thunderbird style. Mail is
listed, read, composed, sent, flagged, moved and deleted. Calendars,
events, address books and contacts are listed, shown and created. Every
action can also be done over the Bus.

## Running it

```
email [--noded-url URL] [--service NAME]
```

A second launch raises the first window.

## Accounts are set up in Prefs

Accounts are configured in **Prefs → Email**, not in the app
([decision](../docs/decisions/2026-10-16-email-accounts-in-settings.md)).
Each account has the fields below.

| Field | What it is |
|---|---|
| Name | The label in the tree. |
| Server URL | The JMAP server base (`https://…`, no trailing slash): maild directly, or a webd vhost that proxies `/jmap`. |
| Email address | The login, the Basic username at the token exchange, and the default From. |
| Password | Kept only in the app's private `secrets.conf.mix` (0600). It never enters settingsd. |
| CA bundle path | Optional. An absolute path to a PEM bundle to trust when the server's certificate is self-issued. |
| Enabled | Off disconnects the account and keeps it listed. |
| Signature | Plain text added to new drafts after a `-- ` line. |

The group also holds the **default account** (the compose From) and the
mail options: **page size** (10–500, default 100), **mark read on open**
and **confirm before delete**. Every change applies at once:

- a new enabled account with a password connects;
- a change to the URL, address or CA path reconnects;
- a rename relabels;
- turning an account off disconnects it;
- a removed account's client and view go.

A settings read never deletes a password. Only **Email** forgets one: an
account removed with `email.account.remove`, or an empty
`email.account.password.set`. An account removed elsewhere (in Prefs, or
by a settings reset) leaves its entry in `secrets.conf.mix`, unused.

settingsd omits the `email` group at its defaults, so a read with no
accounts cannot be told apart from a settingsd that does not carry the
group. When such a read arrives while accounts are shown, Email keeps
them (views, connections, passwords) and says so in the status line. It
stops keeping them at the next read that lists accounts, or when an
account is removed through Email.

If settingsd is not running, the window opens the accounts it last saw
(cached in its config) read-only, and the status line says so.

With no account the window shows **Open Email Settings**. An account that
failed to connect, or has no password yet, shows its error, **Retry** and
the same button.

**Upgrading from the single-account version.** The old `config.conf.mix`
(schema 1, with the password inline) migrates once, losing nothing:

1. The password is written to `secrets.conf.mix`, and an inline CA bundle
   to `ca-<id>.pem` beside it. Nothing else happens until that save has
   landed. A user name and password inside the old server URL are
   removed (the status line says so, without the URL).
2. The account is added to settingsd and made the default. When
   settingsd already holds it under another id, the password is written
   under that id, and that save must land too.
3. Only then is the file rewritten as schema 2: the window shape and the
   accounts cache, with no secret.

Until settingsd has taken the account, the old file stays as it was and
the account works as before.

Files, in `~/.local/state/mixos/apps/email/config/`:

| File | Holds |
|---|---|
| `config.conf.mix` | schema 2: split sizes, mode, selected account, collapsed tree nodes, the last accounts seen |
| `secrets.conf.mix` | the passwords, by account id (0600, written only by email) |
| `ca-<id>.pem` | a migrated account's CA bundle |

A malformed file loads defaults and is never overwritten.

## The window

- **The tree** (left). Each account is a root node, with its connection
  state and its total unread count. Under each account come:
  - its **mailboxes**, nested by parent, with unread counts (the inbox
    first, the trash last);
  - a **Calendars** group;
  - an **Address Books** group.

  Clicking a node selects its account, the matching surface and that
  folder, calendar or book. Nodes collapse and expand, and the state is
  remembered. Once the tree has focus, **↑/↓** move, **←/→** collapse and
  expand (or go to the parent or first child) and **Enter** selects.
- **The centre**: the selected account's mail page, its calendar's events,
  or its book's contacts, with a server-side filter over name, email and
  company. The mail page is a [data table](../toolkit/datatable.md): one
  compact row per email with Subject (with flag and attachment markers; its
  edge drags), From (the name, else the address) and Date (the time today, "Mon 14:02"
  this week, "16 Oct" this year, else "16/10/2025"); unread rows are bold
  and long cells end in "…" with the whole text on hover. A header click
  sorts by that column on the server (newest first by default; maild sorts
  by date, subject and sender), and the pager steps through pages of the
  page size. A new mailbox opens on its first page in the last sort chosen,
  which is remembered with the column widths. Once a row is clicked,
  **↑/↓**, **Page Up/Down**, **Home/End** move through the page and
  **Enter** opens the email. After a delete or a move, the next email
  opens, or the one before it when the last went.

  Above the list is one header row:
  - **Compose**: shrinks to its icon when the pane is narrow.
  - **The count**: "N messages", or "N results" while searching.
  - **The search field**, with **This folder** / **All folders** beside
    it. The search runs on the server about 300 ms after typing stops.
    Sorting and paging work on the result, which starts at its first page.
    All folders adds a Folder column, which cannot be sorted.
  - **Search keys**: **Ctrl+F**, or **/** from the list, focuses the
    field. **Esc** clears it, or lets go when it is empty. **↓** moves to
    the list.

  Choosing a folder in the tree ends the search.

  **Selecting several emails.** Once the list has the keyboard:
  - Ctrl+click adds or removes an email.
  - Shift+click selects a range.
  - Shift+↑/↓ extends the selection.
  - Ctrl+A selects the page. It does this only while the list has the
    keyboard, so in a text field it still selects the text.
  - Esc goes back to one email.

  The footer counts the selection. When the whole page is selected it
  offers **Select all N in folder** (or **in the results** while
  searching), which fetches every id, up to 10,000. Delete, Move, Flag,
  Mark Unread and drag all act on the whole selection, sent to the server
  500 at a time. The reader says "N messages selected".

  **Mark Folder as Read** (Folder menu, and the folder's context menu)
  marks every unread email in the folder. While the folder is searched, it
  marks only the results. Above 500 emails it asks first, with Cancel as
  the default.

  **View › Layout › Folder Pane / Message Pane** hide or show the tree and
  the reader. They have no shortcut. A pane comes back at its old width,
  and the keyboard moves to the list when the pane holding it goes. The
  window remembers the layout.

  Right-clicking a row selects it and opens the message menu. Dragging
  rows onto a folder moves them, the same move `email.move` makes. A
  folder that can take them lights up, and a collapsed one opens after
  0.7 s. Calendars, address books, account roots, the folder the emails
  are already in, and another account's folders refuse the drop. Moves
  between accounts are not supported yet.
- **Tree context menus**: right-clicking a tree node selects it and opens
  its own menu. The same commands are in the **Folder** menu.
  - **Mail folder**: New Subfolder…, Rename…, Delete Folder… (its emails
    move to the Trash first), Mark Folder as Read, Empty… (Trash and Junk
    only), and Refresh. Inbox, Sent, Drafts, Trash, Junk and Archive
    cannot be renamed or deleted.
  - **Account**: Compose from This Account, Reconnect, Account Settings…
    and Refresh.
  - **Calendar and address book**: New Event…, New Contact… and Refresh.

  Names are checked: not empty, no `/`, and not a name a sibling already
  has. Deleting or emptying a folder asks first and says how many emails
  it affects, with Cancel as the default.
- **The reader**: the open email (plain text first, the HTML source when
  there is no plain text), event or contact. A text body written as
  Markdown is rendered: a `text/markdown` part always, and a
  `text/plain` one when the toolkit's conservative detector finds
  Markdown in it (a fence, a table, two headings, or two of a heading,
  a list, bold, inline code, a link and a block quote). Quoted replies,
  signatures, forwarded headers, bare URLs and ASCII art never trigger
  it. Rendered mail shows headings, lists and task boxes, code blocks
  that scroll sideways, tables, quotes and links (http, https and mailto
  open in the desktop's browser or mail handler; the address shows on
  hover). Images are never fetched, because remote images in mail track
  their reader: the alt text is a link to the image. Raw HTML is shown as
  text. **Show Source** beside the date (or View › Message › Render
  Markdown / Show Source) flips one email between the rendering and its
  text, and **Render Markdown** does the reverse for any plain email.
  The choice lasts while that email is open.
- **Compose** (To, Cc, Bcc, Subject, body, and From when there are
  several accounts), reply, reply all and forward. If the account's login
  (server or address) changes while a draft is open, Send waits: the
  dialog shows the new address and **Use New Address**. Mail is sent through
  the proven recipe: build the message, upload it, create the Email in
  Sent, then submit. **Sending requires maild**, because the create step
  uses maild's extension of a top-level `blobId` on `Email/set`. A strict
  RFC 8621 server builds an Email from uploaded MIME with `Email/import`,
  so it refuses the send; `Email/import` is future work.
- **Drafts** are saved to the account's Drafts mailbox as you write, five
  seconds after the last edit. Nothing runs while nobody types. Each save
  is one `Email/set` that creates the new draft (`$draft $seen`, its Bcc
  kept in the bytes) and destroys the one it replaces. Only one save runs
  at a time per compose; edits made during it go when it lands. A quiet
  line under the body says **Saving…**, **Saved hh:mm** or **Not saved:**
  and why (no Drafts mailbox, or the server's refusal). Closing with unsaved
  work asks **Save Draft / Discard / Cancel**, with Cancel the default.
  Discard destroys the saved draft, and so does a send that goes. Opening
  a message in Drafts (or one marked `$draft`) resumes it in compose with
  its To, Cc, Bcc, subject, body and the message it replies to, and later
  saves replace it. Quitting saves the open draft within the close's
  five-second budget. Autosave follows `email.compose.autosave` (Prefs ›
  Email); Ctrl+S saves now either way.
- **Address completion** in To, Cc and Bcc: from two characters, the entry
  being typed (the one after the last `,` or `;` outside quotes) is looked
  up once typing pauses. Contacts across every address book come first,
  one per address on the card. After them come the people the loaded Sent
  pages went to and the loaded inbox pages came from, held in memory only
  and never fetched for this. Addresses already in the field are skipped.
  Up and Down move, Enter or Tab take one (`Name <addr>, `, the name quoted
  when it needs it), and Esc closes the list.
- **Mail actions**: delete to Trash (confirmed when the settings ask),
  move to a mailbox, flag, and mark unread.
- **Print** (Ctrl+P). MixOS has no print service, so Email writes the open
  email as one self-contained page, in `$XDG_RUNTIME_DIR/email-print/`
  (private to you, pages older than a day pruned), and opens it in the
  browser, whose print dialog appears at once. That dialog also offers
  **Save to PDF**. The page carries From, To, Cc, Date and Subject, then
  the body as the reader shows it (rendered Markdown, or the plain text
  wrapped) at the reader's text size, on light paper styles. Nothing from
  the message is passed through as markup: raw HTML is printed as text,
  remote images are left out (their alt text stays), and the page's
  security policy forbids it from fetching anything. If no browser opens
  it, the status line names the page so you can open it yourself.
- **Attachments** (Mail › Attachments…). A popup lists the open email's
  attachments: each with its type's icon, its name (elided, whole on
  hover), its size, and **Save** and **Open**; **Save All** is at the
  bottom. A part the server could not decode is greyed with a note.
  Downloads run in the background, never in the window's thread, and stop
  when Email closes.
  - **Save** writes to your download directory (`XDG_DOWNLOAD_DIR` in
    `~/.config/user-dirs.dirs`, else `~/Downloads`), as a new file only
    you can read. It is written in full, then put in place, so a
    half-written file never appears, and it never replaces a file:
    `report.pdf` becomes `report (1).pdf`. The sender's name is cleaned
    first (no `/`, `\`, `..`, control characters or leading dots, at most
    200 bytes); a part with no name is `attachment-<n>` with its type's
    usual extension. The status line shows where it went.
  - **Open** puts the file in `$XDG_RUNTIME_DIR/email-attach/` (private,
    pruned after a day) and opens it in the app the desktop associates
    with its type (`mimeapps.list`, then `mimeinfo.cache`). Programs and
    scripts (ELF executables and libraries, shell and other scripts,
    `.desktop` launchers, AppImages, Windows executables) are never opened,
    by their type, their name or their first bytes: they can only be saved.
- **New Event** and **New Contact** forms. Start and end are RFC 3339
  instants.

## Keyboard

| Shortcut | Action |
|---|---|
| Ctrl+R | Refresh the account |
| Ctrl+Q | Quit |
| Ctrl+N | Compose |
| Ctrl+S | Save the open compose as a draft |
| Ctrl+Shift+R | Reply |
| Ctrl+Shift+A | Reply all |
| Ctrl+L | Forward |
| Delete | Delete the open email |
| Ctrl+Shift+M | Move the open email |
| Ctrl+Shift+S | Flag or unflag |
| Ctrl+Shift+U | Mark unread |
| Ctrl+P | Print the open email |
| Ctrl+Shift+E | New event |
| Ctrl+Shift+K | New contact |
| Ctrl+1 / 2 / 3 | Mail / Calendar / Contacts |
| Ctrl+PageDown / PageUp | Next / previous account |
| Alt+PageDown / PageUp | Next / previous page of the list |
| Alt+1 / 2 / 3 | Sort by From / Subject / Date (again: reverse) |
| Ctrl+F, or / in the list | Search messages |

## Driving it over the Bus

Service `email`, schema `email.v1`. `HELP` lists the verbs, and
`app.describe` gives each one with its arguments and `read_only` flag.
`account` defaults to the selected account. A refusal is rc 10 with
`{error_code, message}`, and two of the codes are kept distinct:

- `UNKNOWN_ACCOUNT`: no account has that id;
- `NOT_FOUND`: the account's server holds no email, mailbox, event or
  contact with that id.

The other codes are `INVALID_ARGUMENT`, `NOT_CONNECTED`, `UNAVAILABLE`
(settingsd absent), `CONFLICT`, `TIMEOUT` (settingsd did not answer a
write within 60 s), `STALE` (the account reconnected while the request
was pending, so its answer would come from the old connection),
`SUPERSEDED` (a newer `email.open` replaced this one before it loaded), `MIGRATION_PENDING` (the account is still moving
from the old config, so its password cannot change, and it cannot be
removed, until that finishes), `CANCELLED` (Email closed before the
request ran; a write that may have run answers `OUTCOME_UNKNOWN`),
`SERVER`, `FILE` (a file could not be written, or nothing opened it),
`BUSY`, `UNKNOWN_VERB`, `UNKNOWN_COMMAND` and `DISABLED`.

Queries that need the server answer when the server does. The window
never waits on them.

| Verb | Class | Does |
|---|---|---|
| `email.ping`, `email.info`, `email.commands` | query | probe; the whole window state (no secret); the command registry |
| `email.accounts` | query | the accounts with `has_password` and `state`, never a password |
| `email.mailboxes {account?}` | query | mailboxes with roles, parents and counts |
| `email.list {account?, mailbox?, sort?, order?, page?, offset?, limit?, query?}` | query | one page sorted by `sort` (`received`, `subject`, `from`; default `received`) in `order` (`asc`, `desc`; default newest first, else A to Z), from `page` × `limit` or `offset`, with `total`, `page`, `sort` and `order` |
| `email.search {account?, text, mailbox?, limit?, show?}` | intent | emails matching text; with `show: true` the window's list searches too (in `mailbox`, else every folder) |
| `email.search.set {account?, text, scope?}` / `email.search.clear {account?}` | intent | the list's search field and its scope (`folder` or `all`) |
| `email.table.*` (`state`, `sort`, `page`, `page_size`, `select`, `activate`, `context`, `column`, `density`) | query / intent | the message list's [table verbs](../toolkit/datatable.md#the-table-verbs) |
| `email.mailbox.mark_read {account?, id}` / `email.mailbox.empty {account?, id}` | intent | mark a folder read, or empty a Trash or Junk, 500 emails a write |
| `email.panes {folder?, message?}` | intent | show or hide the tree and the reader; answers `panes` (also in `email.info` `ui.panes`) |
| `email.form.field {name, value}` / `email.form.submit {choice?}` / `email.form.cancel` | intent | answer the open form or dialog, such as a folder's name or a delete's confirmation |
| `email.get {account?, id}` | query | one email's headers and `text` body |
| `email.calendars`, `email.events {calendar?, from?, to?}`, `email.event.get {id}` | query | calendars and events |
| `email.books`, `email.contacts {book?, filter?}`, `email.contact.get {id}` | query | address books and contacts |
| `email.contacts.suggest {account?, text, limit?}` | query | address completions: contacts across every book, then recent correspondents (`{suggestions: [{name, email, address, source}]}`) |
| `email.show`, `email.quit` | system, intent | raise, close |
| `email.execute {id}` | intent | run any registry command (published with origin `bus`) |
| `email.mode {name}`, `email.refresh {account?}` | intent | switch surface; fetch again (a failed account reconnects) |
| `email.account.set {id?, name, url, email, password?, ca_path?, enabled?, signature?}` | intent | create or update in settingsd; answers `{id}` once settingsd accepts |
| `email.account.remove {id}`, `email.account.default {id}` | intent | through settingsd, fenced |
| `email.account.password.set {id, password}` | intent | write-only: the secrets file, then reconnect; an empty password forgets it for any account id, even one settingsd no longer holds (`{forgotten}`) |
| `email.account.connect {id}`, `email.account.select {id}` | intent | reconnect; show |
| `email.mailbox.select/create/rename/delete` | intent | `create` answers `{id}` |
| `email.open {id}` | intent | show an email in the reader; answers once loaded |
| `email.mark {ids, read?, flagged?}`, `email.move {ids, mailbox}`, `email.delete {ids, destroy?}` | intent | the list and counts refresh after |
| `email.send {to, cc?, bcc?, subject, body, in_reply_to?}` | intent | send without the dialog |
| `email.compose {…}`, `email.reply {id, all?}`, `email.forward {id, to?}` | intent | open the compose dialog, filled in; answers with its `compose` id |
| `email.print {id?}` | intent | print an email (default the open one) through the browser; answers `{path}` of the page, or `FILE` naming it when no browser opened it |
| `email.attachments {id}` | query | the email's attachments: `part`, `name`, `type`, `size`, `disposition`, `cid`, `downloadable`, `openable` |
| `email.attachment.save {id, part, dir?}` | intent | save one attachment, never over a file; `dir` must be an absolute, existing directory (default the download directory); answers `{path}`. A part that cannot be decoded is `INVALID_ARGUMENT`, an unknown part `NOT_FOUND` |
| `email.draft.save {compose?}` | intent | save the open compose to Drafts now; answers `{id, compose}` once the server confirms (`OUTCOME_UNKNOWN` if Email closes first) |
| `email.calendar.select`, `email.event.create/update/delete` | intent | `create` answers `{id}`; `end` or an ISO 8601 `duration` |
| `email.book.select`, `email.contact.create/update/delete` | intent | `create` answers `{id}` |
| `email.ui.*`, `email.window`, `email.window.state` | input, query | the toolkit [drive verbs](../docs/drive.md) |

Every command the window runs (menu, shortcut, button or `email.execute`)
is published on `email.command.executed` with how it was reached.

`email.info` reports how the open email is shown under each account's
`mail.reader`: `{"format": "markdown" | "plain", "auto": true | false}`,
where `auto` is false once the person (or `email.execute
{"id":"view.reader.markdown"}`) has flipped it. The command is disabled
when the open email has no text body.

Its `compose` object carries the compose `id`, the close prompt
(`closing`), the address dropdown (`suggest`: field, query, cursor,
suggestions), and `draft`: `{id, state, saved_at, error, queued,
autosave, line}`. `state` is `none`, `unsaved`, `saving`, `saved` or
`failed`, and `line` is what the dialog's save line says.

From Mix:

```
send email email.accounts
send email email.list body='{"account":"acct-3f9a1c2b4d5e","mailbox":"m-inbox","limit":20}'
send email email.search body='{"text":"invoice"}'
send email email.mark body='{"ids":["E42"],"read":false}'
send email email.event.create body='{"title":"Standup","start":"2026-10-17T09:00:00+10:00","duration":"PT15M"}'
send email email.account.password.set body='{"id":"acct-3f9a1c2b4d5e","password":"…"}'
```

## How it talks to the server

There is one HTTPS client per account. On connect, the password is
exchanged for a Bearer token (`POST /auth/tokens/issue`) and the Session
is discovered (`GET /.well-known/jmap`). Every method call after that
carries the token. A server without the token endpoint falls back to
Basic auth on every request. Requests batch their methodCalls and chain
`Email/query` into `Email/get` through a ResultReference, so a list is one
round trip. Every answer carries its account id and connection generation,
so a late answer from one account, or from an account's earlier
connection, is never applied to another.

The engine (`apps/email/crates/jmap`) is ported from the retired Dioxus
mail client, with its faults fixed:

- session discovery and URL templates instead of hard-coded paths;
- real pagination;
- Bcc carried through;
- `In-Reply-To` wrapped exactly once;
- CR/LF stripped from header inputs (header injection);
- typed handling of method errors.

## Performance workload

The recorded workload for future profiling is a 10,000-email mailbox in
each of several accounts. The list renders one page (the page size) from
a single query-and-get round trip, the reader fetches one email's bodies
when it opens, and the tree's work grows with the mailbox count, not the
email count. Nothing else scales with mailbox size.
