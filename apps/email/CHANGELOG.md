# Changelog

## Unreleased

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
- MIME size estimates use the same filename-based type as the send worker,
  including long registered types and the octet-stream fallback.
