# Changelog

## Unreleased

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
  Window sends use `window`; toolkit drive requests log their request id and
  caller key so input replayed on later frames can be correlated.
- Compose shows attachment chips under Subject with names, sizes, file status,
  removal and a total. Missing, unreadable and invalid files or an oversized
  encoded message disable Send. Files are checked on open and before Send;
  text edits update the estimate from cached sizes without polling files.
- `email.info` and the compose reply include the attachment metadata, total,
  warning and Send enablement. `email.compose.attachment.remove {path}` removes
  a path through the same state mutation as the chip's remove button.
- Attachment-only drafts are kept too. Restoring a vanished file preserves its
  path and shows it as missing; draft schema 1 remains compatible.
- Invalid logging configuration reports its error and lets the GUI start
  without a subscriber.
