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

portald writes no toolkit files. GTK and Qt follow settingsd through
[`services/exportd`](../exportd/gtkqt.md); with
`appearance.export_toolkits` false the portal states no preference.
