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
email [--compose [FILE.md]] [--noded-url URL] [--service NAME]
```

A second launch raises the first window.

## Compose

Compose, Reply, Reply All, Forward, Send as New, restored drafts and dropped
mail documents each open a separate window in Email's process. Each composer
has its own draft, attachments, chooser, autosave timer and document operations.
There is no compose window.

The window chrome holds **File** (New, Open…, Save As…, Send, Close, Quit),
**Edit** (body Undo, Redo, Cut, Copy, Paste), **Attachments** (Attach files…,
Remove all attachments) and **View** (theme). Ctrl+N opens another composer;
Ctrl+W closes this one. Close asks **Keep Draft / Discard / Keep Editing**
when there is content. Ctrl+Q asks about each draft in turn; Keep Editing
cancels the quit. Drafts already kept or discarded during that quit remain
closed. Unsaved drafts are also autosaved in the private draft store.

`email --compose` starts with a composer as the root window, without a
mailbox. An optional file loads through the same mail-document path as Open…;
relative CLI paths are resolved before forwarding. If Email is running,
the citizen probe forwards `email.compose` and opens another composer there.
A plain launch raises the existing owner, including a compose-only owner.

**Root ownership on Wayland:** the first native window remains the process
owner for its whole lifetime. Closing the mailbox while composers remain
keeps that owner visible with a notice; it exits after the final composer
closes. Closing a root composer similarly waits for its children, then asks
about its own draft. Keep Editing cancels the pending close. A successful send
can leave an empty root owner until its children finish. A child's draft is
never promoted into the root surface: changing a surface's logical identity
would leave its original Wayland app ID and break placement. No hidden root
or `set_visible(false)` workaround is used.

Windows are named `compose-1`, `compose-2`, and so on for the life of the
process, with app IDs `dev.mixos.email.compose-<n>`. Each has a saved frame
under its own name, including a compose-only root. Names are never reused
within a process. Mailbox and compose frames keep separate placement keys.

`email.info {window:"compose-2"}` returns that composer's state and the
`compose_windows` list. `email.compose.attachment.add/remove` and compose
actions through `email.execute` accept the same `window` argument. Omission
selects the most recently focused composer; an unknown name is refused.
`email.compose` always creates a new composer. Toolkit commands instead use
`viewport`, for example `email.ui.tree {viewport:"compose-2"}` and
`email.ui.menu.list {viewport:"compose-2"}`; menu open and all other drive
commands honour the same target.

Representative headless workload: two native deferred composer viewports,
each with a local editor, attachment metadata and an independent document
operation; drive each tree and menu, close one and quit the other. Account
clients and credentials stay in the root engine. Deferred rendering copies
only the account summaries it needs and queues input back to that engine.

## Accounts are set up in Preferences

Accounts are configured in **Preferences → Apps → Email**, not in the app
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
`email.account.password.set`. An account removed elsewhere (in Preferences, or
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
| `config.conf.mix` | schema 2: split sizes, mode, selected account, collapsed tree nodes, attachment bar expansion and inline display choices, the last accounts seen |
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
  hover). Remote images are never fetched, because they track their reader:
  the alt text is a link to the image. Attached image previews are described
  below. Raw HTML is shown as
  text. View › Message › **Render Markdown** (a tick) flips one email
  between the rendering and its text, either way. The choice lasts while
  that email is open.

  The reader opens with a toolbar of what to do with the email: **Reply**
  (its ▾ chooses with or without the quoted original), **Reply All**,
  **Forward** and, apart, **Delete**. The subject comes first, as the
  pane's heading, then From, To and the date. A right-click anywhere in
  the text opens the **Message** menu; attachment tiles and images have
  their own **Open**/**Save** menu.
- **Compose** (To, Cc, Bcc, Subject, body, and From when there are
  several accounts), reply, reply all and forward. If the account's login
  (server or address) changes while a draft is open, Send waits: the
  composer shows the new address and **Use New Address**. Mail is sent through
  the proven recipe: build the message, upload it, create the Email in
  Drafts, submit it, then file it in Sent. **Sending requires maild**, because the create step
  uses maild's extension of a top-level `blobId` on `Email/set`. A strict
  RFC 8621 server builds an Email from uploaded MIME with `Email/import`,
  so it refuses the send; `Email/import` is future work.
  **Email sends plain text only, by design.** The Markdown body goes out as
  `text/plain`, and no HTML part is ever generated. This is a security
  choice: HTML markup, CSS and script are attack surfaces in transit and at
  the reader. Do not add HTML creation
  ([decision](../docs/decisions/2026-10-27-email-sends-no-html.md)).
  The compose window's **File** menu has **Open…** (Ctrl+O) and
  **Save As…** (Ctrl+Shift+S). They read and write [mail documents](../maildoc/README.md),
  ordinary `.md` files with `mail-*` headers and a Markdown body. Opening
  asks **Replace / Cancel** when the composer has content. Saving asks before
  replacing an existing file, and reports invalid headers in the composer.
  Reads and writes run in the background, bounded to 8 MiB. Both choosers
  remember their last folder this session; initially they use the XDG
  Documents folder, then Home if it is not configured.
  `email.compose {document: "/home/user/message.md"}` opens a composer;
  `email.send {document: "/home/user/message.md"}` deliberately sends it.
  `account` may accompany `document`; inline message fields may not
  (`DOCUMENT_INLINE`). From and Reply-To must match identities in that
  account (`IDENTITY_MISMATCH`); an omitted From uses its default identity.
  Reply-To and ordered References are written into the MIME headers.
  Format refusals are `DOCUMENT_TOO_LARGE`, `DOCUMENT_CARRIAGE_RETURN`,
  `DOCUMENT_FRAMING`, `DOCUMENT_HEADER_NOT_ALLOWED`,
  `DOCUMENT_MALFORMED_HEADER`, `DOCUMENT_DUPLICATE_HEADER`,
  `DOCUMENT_EMPTY_VALUE` and `DOCUMENT_INVALID_VALUE`. File failures use
  `DOCUMENT_PATH` (relative path), `DOCUMENT_IO` or `DOCUMENT_ENCODING`
  (UTF-8 required). A changed account or compose refuses a late read as
  `STALE` or `SUPERSEDED`.
  A dropped `.md` mail document opens a composer and never sends. Invalid mail documents show their format refusal in the new window. Local drafts remain private JSON.
  `email.compose` and `email.send` accept `attachments: ["/home/user/report.pdf"]`.
  Paths must be absolute, readable regular files; symlinks are followed.
  Compose and local drafts keep the paths, never the file contents. Restoring
  a draft restores its attachment paths, and Send reads their current contents
  in the background. Under Subject, a paperclip row shows each file's name,
  human-readable size and an × button to remove it, followed by the total
  known file size. The paperclip button is always available while editing:
  it opens the toolkit's multi-file chooser. The compose window's own
  **Attachments** menu offers **Attach files…** (Ctrl+Shift+A) and
  **Remove all attachments**. The chooser starts beside the last file added
  this session, or at Home; it supports editable paths, hidden files,
  Ctrl/Shift selection and keyboard navigation. Directory reads run in the
  background and show at most 10,000 entries. Files dropped onto the window
  join the open compose too. Additions keep path order and omit duplicates;
  invalid dropped items show an inline error. Missing,
  unreadable and non-regular files stay listed with their state and disable
  Send. Chips wrap onto further rows and long names keep room for removal.
  Files sharing a name include their parent directory in both the chip and
  its accessible remove label. An oversized encoded message disables Send;
  the total line shows its estimated size and the account or default limit.
  Files are checked when compose opens, on removal, after a failed send
  and just before Send, with no polling. Text edits and sender changes update the
  size warning from cached file sizes. A draft whose file has vanished shows
  it as missing on restore, keeping its path.
  Attached files follow the plain text part in `multipart/mixed`, with MIME
  types guessed from names and leading bytes by the shared desktop MIME
  database on the send worker. The estimate loads no MIME database and reads
  no file contents: it reserves the maximum 255-byte media type per part.
  It therefore has a small conservative margin (under 300 bytes per part)
  compared with the exact message. Base64 lines contain at most 76 characters
  and end in CRLF.
  The whole encoded message must fit the session's `maxSizeUpload`, or 25 MiB
  when the server gives none. Paths and estimated size are checked on the Bus
  request; Send checks the files again and checks the exact encoded size.
  The send worker resolves an uncertain earlier submission before reading
  attachments. A confirmed earlier submission is filed in Sent without
  sending another copy. The compose window still requires valid attachments
  before it queues a send.
  Sends with attachments log the caller key and attachment paths in Email's
  application log; these details are never added to the message.
  A person's Send uses `window`; `email.execute {id:"mail.send"}` uses its
  Bus caller. Sends triggered by toolkit input retain the caller key and
  `drive_request_id` of the input replayed on that frame, matching the
  separate drive-request audit line. All compose sends share the same
  attachment check. Passes mixing local and injected input retain the drive
  caller and request id, with `mixed_input=true` recording the ambiguity in
  the send audit. Other send audits use `mixed_input=false`.
  `email.execute` refuses `mail.send` when its final checks
  prevent a send from being queued, reporting the compose error.
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
- **Attachments.** An email with attachments always has a bar pinned below
  its scrolling body: a chevron, paperclip, count, total size and **Save All**.
  Its small menu also offers **Open All** for files Open allows (disabled
  when none qualify). The chevron or Message › **Attachments** expands a
  wrapping set of tiles with type icons, names and sizes. Hover shows the
  whole name; a click opens a file, or saves a Save-only file. Right-click
  offers **Open** and **Save**. Each tile reports saving, opening, success
  with its path, or a failure. The expansion is remembered across emails
  and restarts. **Save All** saves every file that can be fetched. Saving
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

  View › **Show Attachments Inline** is on by default and remembered across
  restarts. In both rendered Markdown and source views, attached PNG, JPEG,
  GIF (first frame only) and WebP images appear after the text, below a
  filename divider. Images fit the reader width without stretching beyond
  their physical pixel size; click opens, right-click offers Open/Save.
  Loading and failed previews show a note; the file remains in the bar.
  Undecodable parts, SVG, other types and encoded files above 16 MiB have no
  preview. Downloads use the existing capped JMAP path, with at most two
  background download/decode jobs, image limits of 8192 × 8192 and 256 MiB
  decoder allocation, and textures kept only for the open email. Switching
  emails or turning the choice off frees the textures and drops stale results.
  Previews come only from attachment parts, never URLs. Received HTML
  continues to be shown as text.
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

Service `email`, schema `email.v1`. `HELP` lists the commands, and
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
`BUSY`, `UNKNOWN_COMMAND`, `UNKNOWN_ACTION` and `DISABLED`.

Outgoing attachment refusals have separate codes: `ATTACHMENT_RELATIVE`
(path is relative), `ATTACHMENT_NOT_FILE` (missing or not a regular file),
`ATTACHMENT_UNREADABLE` (cannot inspect, open or read the file), and
`MESSAGE_TOO_LARGE` (the encoded message exceeds the upload limit).

Queries that need the server answer when the server does. The window
never waits on them.

| Command | Class | Does |
|---|---|---|
| `email.ping`, `email.info`, `email.actions` | query | probe; the whole window state (no secret), including reader attachment choices and preview states; the command registry |
| `email.accounts` | query | the accounts with `has_password` and `state`, never a password |
| `email.mailboxes {account?}` | query | mailboxes with roles, parents and counts |
| `email.list {account?, mailbox?, offset?, limit?, query?}` | query | one page, newest first, with `total` |
| `email.search {account?, text, mailbox?, limit?}` | query | emails matching text |
| `email.get {account?, id}` | query | one email's headers and `text` body |
| `email.calendars`, `email.events {calendar?, from?, to?}`, `email.event.get {id}` | query | calendars and events |
| `email.books`, `email.contacts {book?, filter?}`, `email.contact.get {id}` | query | address books and contacts |
| `email.show`, `email.quit` | system, intent | raise, close |
| `email.execute {id}` | intent | run any registry command (published with origin `bus`), including `mail.attachments` and `view.attachments_inline` |
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
| `email.send {document?, from?, to?, cc?, bcc?, reply_to?, subject?, body?, in_reply_to?, references?, attachments?}` | intent | send a mail document or inline fields (To and subject required); From/Reply-To must be owned identity addresses; references is an ordered string or list; attachments are absolute paths read at send time |
| `email.compose {document?, to?, cc?, bcc?, reply_to?, subject?, body?, in_reply_to?, references?, attachments?}` | intent | open a mail document or inline fields; document excludes inline fields; Send reads attachment paths |
| `email.compose.attachment.add {window?, paths:["/home/user/report.pdf"]}` | intent | add to the open compose in order, omitting duplicate paths; no compose: `NOT_FOUND`, sending: `BUSY`; relative, non-regular or unreadable paths use `ATTACHMENT_RELATIVE`, `ATTACHMENT_NOT_FILE` or `ATTACHMENT_UNREADABLE`; an invalid batch changes nothing; answers `{compose}` |
| `email.compose.attachment.remove {window?, path}` | intent | remove all occurrences of the exact path from the open compose; no compose or unknown path: `NOT_FOUND`, sending: `BUSY`; answers `{compose}` |
| `email.reply {id, all?}`, `email.forward {id, to?}` | intent | open the compose window, filled in |
| `email.calendar.select`, `email.event.create/update/delete` | intent | `create` answers `{id}`; `end` or an ISO 8601 `duration` |
| `email.book.select`, `email.contact.create/update/delete` | intent | `create` answers `{id}` |
| `email.ui.*`, `email.window`, `email.window.state` | input, query | the toolkit [drive commands](../docs/drive.md) |

Every command the window runs (menu, shortcut, button or `email.execute`)
is published on `email.action.executed` with how it was reached.

The `compose` object in `email.info` (also returned by `email.compose` and
attachment removal) lists `attachments: [{path, name, label, size, state}]`.
Sizes are raw bytes, or `null` when unknown; states are `ready`, `missing`, `not_file`, `unreadable`
and `relative`. `name` is the plain basename; `label` is the text shown by the
chip, including a directory qualifier when basenames collide.
It also reports `attachment_size` (the sum of known raw file
sizes), `attachment_error`, `attachments_over_limit` and `sendable`, matching
the dialog, and `send_origin: {caller_key, drive_request_id, mixed_input}` after a send
starts; failed sends clear that origin. Removal matches the supplied path
exactly, preserving symlink paths, and updates the saved draft. UI fields remain drivable through the
toolkit commands.

`email.info` reports how the open email is shown under each account's
`mail.reader`: `{"format": "markdown" | "plain", "auto": true | false,
"attachments_expanded": false, "attachments_inline": true, "attachments": []}`,
where `auto` is false once the person (or `email.execute
{"id":"view.reader.markdown"}`) has flipped it. The command is disabled
when the open email has no text body.

The reader's `attachments` list contains `{part, preview: {state}}`, where
`state` is `none`, `loading`, `shown` or `failed`; failures also carry `reason`.
Non-previewable parts and disabled inline display report `none`. The choices
are available through `email.execute {"id":"mail.attachments"}` and
`email.execute {"id":"view.attachments_inline"}` and their checked state
appears in `email.actions`. The existing `mail.attachments` metadata and
transfer states remain available beside `mail.reader`.

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

## Macro Bus commands

The shared [macro Bus surface](/macros/#bus-commands) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Commands keep the app’s canonical prefix, such as `email.macros.run`.

| Command | Arguments | Result |
|---|---|---|
| `email.macros.list` | `{}` | Catalogue and diagnostics |
| `email.macros.reload` | `{}` | Catalogue and diagnostics |
| `email.macros.run` | `{id}` | Acceptance and run number |
| `email.macros.status` | `{}` | Current or last run |
| `email.macros.output` | `{}` | Retained output and dropped flag |
| `email.macros.stop` | `{}` | Whether termination was requested |
| `email.macros.new` | `{title}` | Create a user per-app macro; `{id,title,path,scanned}`, without opening an editor; scan failures add `warning` and preserve the file |

Logging: `--debug[=LEVEL]` and live Bus controls are described in
[live application logging](../docs/dev/debugging.md).
