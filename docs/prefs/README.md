---
title: Prefs
description: MixOS's preferences window — a sidebar of editors: Applications follows, installs, updates, rolls back and removes upstream releases; Appearance sets the session's colour scheme, style, mode, contrast and window frame; App Settings → Email edits the email app's accounts and mail options.
---

# Prefs

Prefs holds MixOS's preference editors in one window: an expandable tree
on the left, the selected section on the right. Appearance contains Themes,
Fonts, Wallpapers and Terminal; Applications contains MixOS Desktop, MixOS Server and
Creative; App Settings contains Email. The sidebar can be resized. The name is a nod to the
Prefs drawer of AmigaOS, where each setting had its own small editor.

## Applications

Each Applications section shows its followed apps, from the [releases
service](../releasesd/README.md): each app's installed version, the latest release,
when it was published, and its status (up to date, update available, not
installed, not checked, or check failed).

- **Check for updates** (Ctrl+Shift+R) asks GitHub about every followed app.
- **Update group** installs newer releases for installed apps in the selected group.
- Select an app to see its latest **release notes**, then:
  - **Install or update** (Ctrl+I) installs it, or updates it if it is
    installed;
  - **Roll back** returns to the previous version;
  - **Remove…** asks first; Cancel is the default, so Enter never removes.
- **Refresh** (Ctrl+R) reads the list again without asking GitHub.

MixOS's `mixos-desktop` and `mixos-server` catalogue entries have their own
sections. A catalogue entry's optional `category` selects `mixos-desktop`,
`mixos-server`, `creative` or `other`; application names and publishers remain
catalogue data. The catalogue's top-level `categories` map can label a group,
for example `categories: {creative: "Graphics"}`.
Unrecognised catalogue entries remain accessible under an additional **Other**
section. Changing groups clears the previous selection and its release notes;
selecting an app through the Bus opens its group automatically.

One operation runs at a time; the status bar says what is happening and how
it ended. Installs are per user, verified against the checksums each release
publishes, and the previous version is kept for rolling back: see
[Application releases](../releasesd/README.md).

If the releases service is not running, the editor says so instead of an
empty list. You follow apps by listing them in
`~/.config/mixos/releases.mx`.

## Appearance

**Appearance → Themes** sets the session's look through the settings service
(settingsd): the **colour scheme** (each shown with its accent), the
**style** (the scheme's own, or Plain, Pro, Studio or Classic), **light or
dark**, **contrast**, and the **window frame**: a title bar drawn by the app
or by the system, and which side the app's window buttons sit on.

Each change applies to the whole session as you make it, as in GNOME's
settings: the apps that follow the session look (Prefs itself, BusViewer,
and the portal's clients) change with it. Only the setting you changed is
written. Changes made quickly one after another are sent in turn, never
lost.

If settingsd refuses a change, Prefs keeps it and says so: **Apply**
(Ctrl+S) tries again and **Revert** drops it. If the appearance is changed
elsewhere at the same moment (another window, or an agent), Prefs follows
it on every setting you did not touch. If it changed one you did, Prefs
names it and applies nothing until you choose **Keep my changes** or
**Revert**. Every change is checked against the revision you were shown, so
it never lands on a change you have not seen. If its answer is
lost, Prefs asks settingsd for the change's receipt before anything else is
applied (**Check again** asks now); if settingsd no longer holds one, the
session's current state decides what the status line says.

**Appearance → Fonts** shows the interface, heading and monospace faces currently
installed on the window, with previews. Font family and size selection is not
available yet; these faces follow the session design.

**Appearance → Terminal** holds the terminal's font, layout, cursor, focus ring,
mouse and environment settings, plus Reset terminal. Each change uses
settingsd and applies as it is made. Themes contains only the session theme
controls. See the [terminal settings](../docs/spec/settings/README.md#terminal) for
the defaults and ranges.

## App Settings → Email

**App Settings** holds settings that belong to one application, kept in
the settings service like the session's look. It is separate from
Applications, which installs apps. Its first section is **Email**, the
[email app](../email/README.md)'s accounts and mail options (`email.*` in
settingsd; see the [email settings](../docs/spec/settings/README.md)).

- **Accounts** lists the accounts in the order Email's folder tree shows
  them, marking the default and any that are disabled. **Add account**
  opens a blank form; **Move up** and **Move down** reorder; **Make
  default** picks the account new mail is sent from (with none, the first
  enabled account); **Remove…** asks first, and Cancel is the default.
- The selected account's form holds its **name**, **JMAP URL** (https,
  no trailing slash), **login email**, an optional **CA bundle** (an
  absolute PEM path; blank trusts the system roots) and a **signature**.
  Typing stays in the form until **Save**; **Revert** drops it. Each field
  is checked by the settings service's own rules first, and a field it
  would refuse is named under the field and nothing is sent. **Enabled**
  applies at once.
- **Mail** holds messages per page (10 to 500), marking messages read when
  opened, and asking before deleting them; each applies at once. **Reset
  email settings…** asks first, then puts every email setting back to its
  default, which means no accounts.

Each change is one write to settingsd carrying the whole account list,
and the default account in the same write when the change touches it
(removing the default account clears it). Every write is checked against
the revision shown, as on the Appearance panel. If settingsd refuses one,
the status line gives its reason and the panel keeps showing what
settingsd holds; your typing stays in the form.

**Passwords never go to the settings service**, whose settings every Bus
client can read, and Prefs neither keeps nor shows them. **Set password**
hands the typed password to Email itself (`email.account.password.set`),
which keeps it in its own private file, and empties the field; the field
gets the password back only if Email refuses it. If Email is not running,
Prefs starts it (`apps.launch`) and waits up to about 16 seconds for it to
come onto the Bus. Under the field, Prefs says whether Email holds a
password for the account, as Email reports it (`email.accounts`), or that
this is unknown because Email is not running. **Connect now** asks Email
to connect the account (`email.account.connect`). **Forget password…**
asks first (Cancel is the default), then has Email delete the password it
keeps; it is the only time Prefs sends an empty password, and it starts
Email if needed, as Set password does. Removing an account also asks Email
to forget its password once settingsd shows the account gone, but only if
Email is already running: Prefs does not start Email for this, and if Email
is away or refuses, the status line says the password may still be held.
An Email too old to know these verbs is reported as such, not retried.

```mix
-- What Prefs shows: accounts (with has_password), default, mail options.
send prefs prefs.email

-- Add an account; the answer carries its new id.
send prefs prefs.email.set body='{"op":"add","name":"Work","url":"https://mail.example.com","email":"admin@example.com"}'

-- Change one field, make it the default, move it, disable it.
send prefs prefs.email.set body='{"op":"update","id":"acct-0f3a2b1c9d8e","signature":"-- Admin"}'
send prefs prefs.email.set body='{"op":"default","id":"acct-0f3a2b1c9d8e"}'
send prefs prefs.email.set body='{"op":"move","id":"acct-0f3a2b1c9d8e","direction":"up"}'
send prefs prefs.email.set body='{"op":"enable","id":"acct-0f3a2b1c9d8e","enabled":false}'

-- A mail option.
send prefs prefs.email.set body='{"path":"email.mail.page_size","value":50}'

-- Hand a password to Email (answered once Email has), or forget it.
send prefs prefs.email.password body='{"id":"acct-0f3a2b1c9d8e","password":"…"}'
send prefs prefs.email.password body='{"id":"acct-0f3a2b1c9d8e","forget":true}'
```

`prefs.email.set` takes `op` `add`, `update`, `remove`, `move`, `default`
or `enable`, or `path` and `value` for a mail option. Over the Bus,
`remove` removes at once: the confirmation belongs to the window. A field
the settings service would refuse is answered with `ARGUMENT`, naming it.
For scripts that do not need Prefs open, call `settings.apply` on the
`email.*` paths directly, and `email.account.password.set` on Email.

## For agents

Prefs answers on the Bus as `prefs`. `prefs.info` reports the whole state;
`prefs.apps` the rows, selection and notes; `prefs.appearance` the session
look, the expected look while a change settles, any held or conflicting
edits and the revision read; `prefs.appearance.set`
changes it at once (`scheme`, `style` with `own` for the scheme's own, `mode`,
`contrast`, `decorations`, `caption_side`); `prefs.select {app}` and
`prefs.section {name}` selects `themes`, `fonts`, `wallpapers`, `terminal`, `mixos-desktop`,
`mixos-server`, `creative`, `other` or `email` and expands its parent. `prefs.panel {name}`
remains compatible, opening the default Themes, Creative or Email section
(`applications`, `appearance`, `app-settings`). `prefs.email`,
`prefs.email.set` and `prefs.email.password` are described under App
Settings → Email above.
`prefs.execute {id}` runs any command a button
or menu would (`apps.check`, `apps.install`, `appearance.apply`,
`appearance.revert`, …), and `prefs.commands` lists them with their
enablement. The window is drivable like every MixOS app
(`prefs.ui.*`, `prefs.window`). The verbs are registered in
`docs/docs/spec/bus/verbs.conf.mix`.

For scripted work, call the services directly (`releases.*`,
`settings.*`): Prefs is their window, not a second implementation.

## Wallpaper

In **Appearance → Wallpapers**, choose the default or an output, then Theme,
Colour or Image. Colour accepts a picker or `#RRGGBB`. Browse opens the native
image browser at the installed wallpaper folder, or Pictures in your home
folder. It lists PNG/JPEG files and folders without blocking the window.
Changes stay in Prefs until **Apply wallpaper**. **Revert wallpaper** drops
them; **Refresh wallpaper** reads the current state again.

An output's **Use default** removes just that override. Editing the default
preserves every output override. Images currently crop to fill; fit controls
and slideshows will follow when backdrop supports them.

Backdrop must be running on the same session Bus. A refusal shows its reason,
keeps the draft and leaves the last good wallpaper in place. Prefs checks for
intervening changes before sending the whole document; conflicts require
Revert and a fresh edit. This check is best effort: backdrop does not yet
provide an atomic compare-and-set operation. A lost reply is checked by a
state read, never automatically resent.

Wallpaper and browser state are included in `prefs.info`, wallpaper state in
`prefs.appearance`. Commands `wallpaper.apply`, `wallpaper.revert`,
`wallpaper.refresh` and `wallpaper.browse` are available through
`prefs.execute`; the normal `prefs.ui.*` verbs drive the controls and browser.

## Running and testing

Run with `prefs [--noded-url URL]`; a second launch raises the first window
(`prefs.show`). The Bus connection is [citizen](../citizen/README.md).

The source is [`apps/prefs`](https://github.com/markc/mixos/tree/main/apps/prefs):

| Path | What |
|---|---|
| `crates/preferences` | the headless engine: panels, rows, notes, one operation at a time, the `prefs.*` verbs |
| `crates/preferences/src/appearance.rs` | the Appearance panel: the look, its draft, fenced applies, receipts for lost replies |
| `crates/preferences/src/email.rs` | App Settings → Email: account changes planned and checked, fenced applies, passwords handed to Email |
| `src/commands.rs` | every action as a registered command (menus, buttons, shortcuts, `prefs.execute`) |
| `src/view.rs` | the egui rendering; returns what the person did |
| `src/shell.rs` | events in, effects out, in order; drained shutdown |
| `i18n/en/prefs.ftl` | the strings |

Test with `cargo test -p preferences -p prefs`. Regenerate the snapshots
with `UPDATE_SNAPSHOTS=1` and look at the images before committing them.
