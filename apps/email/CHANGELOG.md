# Changelog

## Unreleased — 2026-10-28

- Follow session font size and weight with a custom design package, keeping
  the theme file design beneath profile framing, menus and title band.

## Unreleased

- Share compose label sizing, rows, text fields and attachment chips with ced
  through toolkit::header_form, keeping Email's identity row and completions.

- Give every composer one complete client title row under either session
  decoration preference; compd never supplies the mailbox menu or action bar.
- Open a blank composer with its error after a failed document launch. Merge
  kept-draft offers by id and suppress late offers during document launches.
- Ignore `window` for `compose.new`. Cover same-pump compose deliveries,
  drops during an existing read, closed origins before refresh and Quit Back.

- Keep compose-only restore offers in the owning window across refreshes.
  Later and the final Discard start the requested blank message; file launches
  suppress kept-draft offers for that run without deleting saved drafts.
- Quit waits quietly for closed windows' Save As writes. Cancel revokes prior
  quit permission and deferred owner close; both empty and object Quit bodies
  return the same fields and publish the registry execution.
- Batch mixed mailbox drops into one composer and register document drops by
  the returned read ticket. Test the existing closed-composer label on a late
  drop failure and check literal source label keys against the English catalogue.
- Refuse `window` on compose, reply and forward, which always create windows;
  stamp opened window names only on compose-opening command replies.

- Fix mailbox Quit with simultaneous composer input, retain pending Save As
  writes through Close and Quit, and carry completed send/save outcomes to
  the mailbox. Only Cancel on a Quit question revokes a deferred owner close.
- Return the opened `window` from Bus compose, reply, forward, document opens
  and `compose.new`. Default targeting follows the latest creation or focus;
  refuse unknown and non-string window targets. Report draft questions from
  `email.quit` and paint Bus-created composers immediately.
- Attach ordinary Markdown to its drop target, open composers for ordinary
  mailbox file drops, and retire failed document-loading children. Offer kept
  drafts in compose-only startup and set child app IDs explicitly.

- Replace the modal composer with one window per message, with File, Edit,
  Attachments and View menus in its chrome. Add `email --compose [FILE.md]`
  and single-instance forwarding through the citizen probe.
- Give each composer independent drafts, document operations, attachments,
  Bus targeting (`window`) and saved placement (`compose-<n>`). Toolkit drive
  commands use `viewport`, including compose menus.
- Retain the first native window as process owner while children remain;
  never promote or rename a child into its surface. Close waits for children;
  Quit asks per draft and supports cancellation.
- Create and retire composers through a one-off native paint request when
  the mailbox is parked. Direct document sends retain their engine-owned
  reply state independently of composer lifetimes.

- Open and save inert mail documents from compose's File menu (Ctrl+O,
  Ctrl+Shift+S), with replace questions and background file I/O. Remember the
  Open/Save folder for the session, initially XDG Documents or Home.
- Accept `document` paths on `email.compose` and `email.send`; refuse mixed
  inline fields, unowned From/Reply-To identities and each format error with
  distinct codes. Drops open mail documents and never send them; other
  Markdown stays an attachment while composing. Drafts remain private JSON.
- Send Reply-To and ordered References, including through inline Bus sends.

- Pin received attachments below the reader body, with a remembered expanded
  tile view, Save All/Open All, per-file Open/Save and transfer status.
  Message › Attachments now toggles the bar's expansion.
- Show attached PNG, JPEG, GIF (first frame) and WebP images after the text
  by default. View › Show Attachments Inline is remembered across restarts.
  Previews use capped downloads, two background decode slots, explicit image
  limits and textures kept only for the open email; remote URLs stay unfetched.
- Report attachment bar choices and each preview's state/reason in
  `email.info` under `mail.reader`; both choices work through `email.execute`.
- Add an Attachments menu inside compose, with Attach files… (Ctrl+Shift+A),
  Remove all attachments and a one-click paperclip button. The shared chooser
  runs above compose and remembers the last attachment directory this session.
- Add `email.compose.attachment.add {paths}` and dropped-file attachment entry.
  They share atomic path validation, deduplication, draft revisions, metadata
  refresh and size warnings; sending refuses changes.
- Use citizen's shared app-launch helper when opening Prefs, preserving app
  refusals and avoiding repeated commands after an uncertain outcome.

- `email.execute` refuses `mail.send` when final validation prevents queueing,
  returning the compose error rather than a successful `executed` reply.
- Compose attachment metadata keeps the plain basename in `name` and the
  qualified display text in `label`. Failed sends clear their audit origin.
- Mixed local/drive input passes retain the drive caller and request id in
  send audits, with `mixed_input=true` recording the ambiguity. Other send
  audits use `mixed_input=false`.
- Narrow chip labels retain at least four em of width and truncate their text.
- Attachment chips wrap within the compose form's field width. Scoped chip
  frames no longer expand the row and dialog instead of starting a new row.
- `email.send` and `email.compose` accept an optional `attachments` list of
  absolute file paths. Symlinks are followed; relative paths, non-files,
  unreadable files and oversized messages have distinct refusal codes.
- Send reads files at send time and emits multipart MIME with desktop MIME
  types, base64 and RFC 2231 filenames. The encoded message is limited by
  JMAP `maxSizeUpload`, or 25 MiB when the server advertises no limit.
- Local drafts save and restore attachment paths (never file contents),
  including across restarts. Older drafts still load without attachments.
- Send resolves an earlier uncertain submission before checking local files or
  upload limits. Attachment reads and encoding run off the async runtime;
  command validation uses metadata only, and MIME bytes are rendered once.
- Standalone MIME builders impose no upload limit; Send applies the server's
  limit to the exact encoded bytes.
- Sends with attachments log the caller key and paths in the Email app.
  Person-initiated sends use `window`; `mail.send` through `email.execute`
  retains the Bus caller. Driven sends retain the actual replayed request id
  and caller through compose state and queued work for audit correlation.
- Compose shows attachment chips under Subject with names, sizes, file status,
  removal and a total. Missing, unreadable and invalid files or an oversized
  encoded message disable Send. Chips wrap, preserve text line height and
  disambiguate colliding filenames with their parent directories. Oversize
  warnings appear on the total line, with distinct account/default limits.
  Files are checked on open, removal, send failure and before Send;
  text edits update the estimate from cached sizes without polling files.
- `email.info` and the compose reply include the attachment metadata, total,
  warning and Send enablement. `email.compose.attachment.remove {path}` removes
  a path through the same state mutation as the chip's remove button.
- Attachment-only drafts are kept too. Restoring a vanished file preserves its
  path and shows it as missing; draft schema 1 remains compatible.
- Invalid logging configuration reports its error and lets the GUI start
  without a subscriber.
- MIME size estimates reserve the maximum 255-byte media type per part,
  without loading the MIME database or reading file contents. Send restores
  name/content sniffing on its worker; estimates bound actual MIME size with
  under 300 bytes of headroom per part.
