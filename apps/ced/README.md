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
