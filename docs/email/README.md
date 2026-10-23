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
- **The centre**: the selected account's mail page (newest first, as many
  as the page size), its calendar's events, or its book's contacts, with
  a server-side filter over name, email and company. The mail page is a
  table of one compact row per email: **Subject** (with flag and
  attachment markers; drag its edge to widen it), **From** and a compact
  **Date** (the time today, the weekday this week, the day and month this
  year, else the full date). Unread rows are bold. Clicking a row opens
  it in the reader; once the list has the keyboard, **↑/↓** move the
  selection and open each email, and **Enter** opens the selected one.
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
  text. View › Message › **Render Markdown** (a tick) flips one email
  between the rendering and its text, either way. The choice lasts while
  that email is open.

  The reader opens with a toolbar of what to do with the email: **Reply**
  (its ▾ chooses with or without the quoted original), **Reply All**,
  **Forward** and, apart, **Delete**. The subject comes first, as the
  pane's heading, then From, To and the date. A right-click anywhere in
  the reader opens the **Message** menu.
- **Compose** (To, Cc, Bcc, Subject, body, and From when there are
  several accounts), reply, reply all and forward. If the account's login
  (server or address) changes while a draft is open, Send waits: the
  dialog shows the new address and **Use New Address**. Mail is sent through
  the proven recipe: build the message, upload it, create the Email in
  Sent, then submit. **Sending requires maild**, because the create step
  uses maild's extension of a top-level `blobId` on `Email/set`. A strict
  RFC 8621 server builds an Email from uploaded MIME with `Email/import`,
  so it refuses the send; `Email/import` is future work.
- **The Message menu** holds everything that acts on the open email:
  reply (with or without the quote), reply all, forward, star, mark read
  or unread, junk or not junk, move, and delete to Trash (confirmed when
  the settings ask). **Junk** moves the email into the account's Junk
  mailbox and **Not Junk** moves it from Junk back to the inbox; maild
  trains its spam filter on those moves (there is no junk keyword). An
  account without a Junk mailbox shows Junk disabled, saying so. After a
  delete, move or junk the reader opens the next email in the list (the
  previous one when it was the last).
  Labels follow the email (**Star** or **Remove Star**, **Mark as Read** or
  **Mark as Unread**). Marking an open email unread leaves it open and
  unread: only opening an email marks it read.
- **Attachments.** An email with attachments shows a paperclip and its
  count at the end of the reader's toolbar (and Message › Attachments…).
  Its popup lists each one with its type, name and size, and **Save**
  and **Open**; **Save all** saves every one that can be fetched. Saving
  writes into your downloads directory (`$XDG_DOWNLOAD_DIR`, the
  `user-dirs.dirs` entry, else `~/Downloads`) and never replaces a file:
  a taken name becomes `name (1).ext`. Names from mail are cleaned (no
  paths, `..`, control characters or leading dots, 200 bytes at most),
  files are written whole and readable only by you, and the status line
  names where each went. **Open** saves into a private directory first
  and hands the file to the desktop's default application; programs and
  scripts (executables, shell scripts, `.desktop` files and the like)
  are Save only. A part the server could not decode is shown greyed. The
  download runs in the background; nothing is written until the whole
  file has arrived.
- **Printing** (Message › Print…, Ctrl+P) goes through the browser's
  print dialog, which also offers **Save to PDF**: MixOS has no print
  service. The email becomes one self-contained page in a private
  directory (`$XDG_RUNTIME_DIR/email-print/`, readable only by you), shown
  as the reader shows it: Markdown rendered, else the plain text. Headers
  (From, To, Cc, Date, Subject) come first, in the light theme on white.
  Nothing from the email runs or loads: its HTML is printed as text,
  images are their alt text (a remote image would tell the sender), and a
  strict Content-Security-Policy allows only the page's own style and the
  script that opens the dialog. The page opens in Firefox through the
  session launcher; without one, the status line names the file to open.
  Pages older than a day are removed on the next print.
- **Address completion** in To, Cc and Bcc. After two characters of an
  address the field offers up to eight: the account's contacts (from
  every address book) first, then addresses it has seen (recipients of
  loaded Sent pages, senders of other loaded pages, kept in memory only).
  Only the entry being typed is completed, the one after the last `,` or
  `;` outside quotes. **Up**/**Down** move, **Enter** or **Tab** take one
  as `Name <address>, ` (the name quoted when it holds a comma or another
  special character), **Escape** closes the list. Addresses already in the
  field are left out. The contacts are looked up once typing pauses
  (200 ms), and only the newest lookup's answer is shown.
- **Quoted replies.** A reply quotes the original's text body (a Markdown
  email's source; an HTML-only email's text) under an "On …, Ann wrote:"
  line, every line marked `> ` (an already quoted line gains another
  `>`), with the original's signature cut at its `-- ` line and lines
  never re-wrapped. The person's text and signature go above the quote.
  Plain text only.
- **New Event** and **New Contact** forms. Start and end are RFC 3339
  instants.

## Keyboard

| Shortcut | Action |
|---|---|
| Ctrl+T | Refresh the account (Thunderbird's Get Messages) |
| Ctrl+Q | Quit |
| Ctrl+N | Compose |
| Ctrl+R | Reply |
| Ctrl+Shift+R | Reply all |
| Ctrl+L | Forward |
| S | Star or remove the star |
| M | Mark read or unread |
| J / Shift+J | Junk / Not junk |
| Ctrl+Shift+M | Move the open email |
| Delete | Delete the open email |
| Ctrl+P | Print (the browser's print dialog, or save as PDF) |
| Ctrl+Shift+E | New event |
| Ctrl+Shift+K | New contact |
| Ctrl+1 / 2 / 3 | Mail / Calendar / Contacts |
| Ctrl+PageDown / PageUp | Next / previous account |

The shortcuts follow Thunderbird's. S, M, J, Shift+J and Delete act only while no
text field has the keyboard (they type into the search box or a draft
otherwise). Help › Keyboard Shortcuts shows the chords in force.

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
`SERVER`,
`BUSY`, `UNKNOWN_VERB`, `UNKNOWN_COMMAND` and `DISABLED`.

Queries that need the server answer when the server does. The window
never waits on them.

| Verb | Class | Does |
|---|---|---|
| `email.ping`, `email.info`, `email.commands` | query | probe; the whole window state (no secret); the command registry |
| `email.accounts` | query | the accounts with `has_password` and `state`, never a password |
| `email.mailboxes {account?}` | query | mailboxes with roles, parents and counts |
| `email.list {account?, mailbox?, offset?, limit?, query?}` | query | one page, newest first, with `total` |
| `email.search {account?, text, mailbox?, limit?}` | query | emails matching text |
| `email.get {account?, id}` | query | one email's headers and `text` body |
| `email.calendars`, `email.events {calendar?, from?, to?}`, `email.event.get {id}` | query | calendars and events |
| `email.books`, `email.contacts {book?, filter?}`, `email.contact.get {id}` | query | address books and contacts |
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
| `email.attachments {id}` | query | the email's attachments (part, name, type, size, undecodable, save_only), never their content |
| `email.attachment.save {id, part, dir?}` | intent | save one attachment (into `dir`, absolute and existing, else the downloads directory) without replacing a file; answers `{path}` |
| `email.contacts.suggest {text, limit?}` | query | address completions for `text`: contacts first, then addresses the account has seen; at most `limit` (8) |
| `email.print {id?}` | intent | write the print page (the open email by default) and open it in the browser; answers `{path, opened}` |
| `email.junk {ids, junk}` | intent | move to the Junk mailbox (`junk: true`) or back to the inbox; maild trains spam or ham on the move; answers `{ids, mailbox}` |
| `email.send {to, cc?, bcc?, subject, body, in_reply_to?}` | intent | send without the dialog |
| `email.compose {…}`, `email.reply {id, all?}`, `email.forward {id, to?}` | intent | open the compose dialog, filled in |
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

## Macro Bus verbs

The shared [macro Bus surface](/macros/#bus-verbs) uses this process's
catalogue and runner. Subscribe to `email.macros.finished` for completion.

| Verb | Arguments | Result |
|---|---|---|
| `email.macros.list` | `{}` | Catalogue and diagnostics |
| `email.macros.reload` | `{}` | Catalogue and diagnostics |
| `email.macros.run` | `{id}` | Acceptance and run number |
| `email.macros.status` | `{}` | Current or last run |
| `email.macros.output` | `{}` | Retained output and dropped flag |
| `email.macros.stop` | `{}` | Whether termination was requested |
| `email.macros.new` | `{title}` | Not yet available |
