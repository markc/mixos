# ced

The MixOS Editor: a desktop editor over the `edit` Bus service.

Documentation: <https://mixos.dev/ced/> — source [`docs/ced/README.md`](../../docs/ced/README.md).

Markdown tabs offer View › Markdown › Source, Split and Preview, with no
default shortcut. Each tab starts in Source and keeps its choice for this
session. Split gives the editor and read-only, selectable preview half the
document area each. Both panes use the editor's saved content style. Links
open through the desktop's web or mail handler.

`ced.action` accepts `view.markdown.source`, `view.markdown.split` and
`view.markdown.preview`. They are disabled for other languages and when no
tab is open. `ced.tabs` rows and `ced.state` include `preview` (`source`,
`split` or `preview`). Save As to another language shows Source; the tab's
Markdown choice stays dormant. Closing the tab drops its choice and parse
cache. These choices are never saved in the session file.

The headless loop draws no document and has no view state. These actions
need a window and return `UNAVAILABLE` there; the reported view is `source`.
