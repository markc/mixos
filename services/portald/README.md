# portald

The MixOS desktop portal on the session bus: `org.freedesktop.portal.Settings`, answered from settingsd; `org.freedesktop.portal.FileChooser` version 4, forwarded to `files.choose`; and with `--screencast` `org.freedesktop.portal.ScreenCast`, monitor sources from compd screencopy delivered as PipeWire video nodes.

FileChooser serves `OpenFile`, `SaveFile` and `SaveFiles` on the existing
Desktop object for trusted unsandboxed callers, including rfd. DOpus must
serve `files` (`dopus --choose-service`) on the same native Bus. Missing or
lost providers answer Response 2. Each caller owns its Request; Close or a
disconnect cancels it without a Response. Byte paths keep their exact bytes.
The bridge subscribes before forwarding and recovers missed outcomes using
the contract's temporary five-second list/result check.

`cargo test -p portald` runs mapping, lifecycle and private D-Bus/noded tests,
including early outcomes, timeout, Close, token reuse and provider loss.

Documentation: <https://mixos.dev/portald/> — source [`docs/portald/README.md`](../../docs/portald/README.md).

At startup from a validated appearance cache, and after each accepted settingsd
projection, portald merges two keys into `$XDG_CONFIG_HOME/gtk-3.0/settings.ini`
under `[Settings]`: `gtk-icon-theme-name=mixos` and `gtk-decoration-layout`.
Right captions use `menu:minimize,maximize,close`; left captions use
`close,minimize,maximize:menu`. Caption placement comes from the same atomic
appearance read as the portal values. Without a validated projection it waits
for settingsd.

The export is skipped, with one info log, when `XDG_CONFIG_HOME` is unset,
relative or equal to `$HOME/.config`. Parent segments and symlinks in the
destination path are refused too, so a nested desk cannot write through a link
into the host desktop's config. Other keys, comments and sections are preserved.
Unchanged content is not written; changed content is written to a sibling
temporary file, synced and renamed. Files over 64 KiB are refused. Export errors
are logged and do not stop the portal. `portald.status` includes the last path,
outcome (`written`, `unchanged` or `skipped`) and reason under `gtk_settings`.
