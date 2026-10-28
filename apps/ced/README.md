# ced

The MixOS Editor: a desktop editor over the `edit` Bus service.

Documentation: <https://mixos.dev/ced/> — source [`docs/ced/README.md`](../../docs/ced/README.md).

Markdown tabs offer View › Markdown › Source, Split and Preview, with no
default shortcut. Each tab starts in Source and keeps its choice for this
session. Split gives the editor and read-only, selectable preview half the
document area each. Both panes use the editor's saved content style. Links
open through the desktop's web or mail handler.
The preview shares the editor palette's field fill and gutter inset. In
Split, the source's first row reserves an opening heading's extra ascent
so their baselines line up.

`ced.execute` accepts `view.markdown.source`, `view.markdown.split` and
`view.markdown.preview`. They are disabled for other languages and when no
tab is open. `ced.tabs` rows and `ced.state` include `preview` (`source`,
`split` or `preview`). Save As to another language shows Source; the tab's
Markdown choice stays dormant. Closing the tab drops its choice and parse
cache. These choices are never saved in the session file.

Run actions with `{id, params?}`. Optional `tab` (or `buffer`) and `args`
belong inside `params`; without a selector, the active tab is used. Unknown
action ids return rc 10 `UNKNOWN_ACTION`.

Preview caches use the tab id and mirror revision. A repaint reads no buffer
text; a language change clears the cache. Save As detects the interim language
once, and a fresh open's language metadata takes precedence. View hints use
static values and are refreshed only when their inputs change. `ced.actions`
echoes window enablement for the three Markdown actions; all older actions
keep reporting `enabled: true`.

The headless loop draws no document and has no view state. These actions
need a window and return `UNAVAILABLE` there; the reported view is `source`.

## Mail documents

Ced recognises an opening `---` block containing `mail-*` headers. Valid
[mail documents](../../docs/maildoc/README.md) open with a compact header form above
the body editor. From is a plain address; Email checks identities when you
send. Cc, Bcc and Reply-To buttons reveal optional rows. Thread headers are
read-only. Attach… chooses multiple paths; each attachment chip can be removed.
Each header field change is one undo step. Header undo history is separate
from the edit service's body history: Undo exhausts header edits first, then
undoes body edits. Redo restores header edits before body edits.

The native edit service holds the body in a scratch buffer, seeded at revision
zero, so Undo cannot remove the initial body. Ced keeps the headers with the
tab. Save and Save As assemble the document with `maildoc::write`: LF line
ends, sorted headers, omitted empty headers and canonical trailing body
whitespace. An invalid field refuses the save without touching the file.
A file changed on disk also refuses the save; use Save As or reopen it.
Save As retains the mail kind. Reload from Disk requires closing and reopening
the mail tab, so it cannot silently replace separately edited headers.

File ▸ Send saves first, then asks Email to send that absolute document path.
The tab shows Sending… until Email replies. A second Send is refused while
one is in flight. Email's refusal appears verbatim. An uncertain transport
outcome says the mail may have been sent; ced never retries it. Opening a
document never sends anything. Ced knows no accounts, JMAP or sending limits.

Agents use `ced.execute {id:"file.send", params:{tab:1}}`; the reply waits for
the save and Email's outcome. `ced.state` returns body text and, on mail tabs,
`mail:{headers,sending,saving,notice}`. Update one field by changing the returned
headers and calling `ced.execute {id:"mail.headers", params:{tab:1,
args:{headers:HEADERS}}}`. Thread headers cannot be changed through that action.
Send is unavailable on ordinary text and Bus-message documents (`to` plus
`command`), which remain plain text for Workshop. A refused mail candidate
also opens as unchanged text, with maildoc's exact refusal beside it for repair.
Mail tabs keep header edits in this process. Exit refuses while a mail tab
is dirty or a save/Send is in flight; save or explicitly close the tab first.