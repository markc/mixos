---
title: Preferences
description: Appearance, Text & Fonts, Windows, app preferences, Accessibility, and Software & Updates in one window.
---

# Preferences

Preferences holds MixOS's preference editors in one window. The flat sidebar
lists Appearance, Text & Fonts, Windows, Apps (Email, Term, Ced),
Accessibility, and Software & Updates. Hairlines separate look, desktop,
apps and system pages. Empty categories are hidden. Each page starts with
an **Affects:** line. Wallpaper opens from Appearance.

Section commands have no default shortcuts. The Apps header is a plain label.

Press `/` outside a text field, or choose the magnifier in the title bar's icon bar, to
focus search. It filters sections by title, category, extra words and
settings keys, ignoring case. Try `terminal.` or `background`. Enter opens
the first match. Escape clears search and releases focus. Ctrl+/ opens
Keyboard shortcuts.

## Software & Updates

Software shows its followed apps, from the [releases
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

The group selector at the top offers MixOS Desktop, MixOS Server and
Creative applications. A catalogue entry's optional `category` selects `mixos-desktop`,
`mixos-server`, `creative` or `other`; application names and publishers remain
catalogue data. The catalogue's top-level `categories` map can label a group,
for example `categories: {creative: "Graphics"}`.
Unrecognised catalogue entries remain accessible in **Other**, which appears
only when it has rows. Changing groups clears the previous selection and its release notes;
selecting an app through the Bus opens its group automatically.

One operation runs at a time; the status bar says what is happening and how
it ended. Installs are per user, verified against the checksums each release
publishes, and the previous version is kept for rolling back: see
[Application releases](../releasesd/README.md).

If the releases service is not running, the editor says so instead of an
empty list. You follow apps by listing them in
`~/.config/mixos/releases.mx`.

## Appearance

**Appearance** sets the session's colour scheme, light or dark mode,
contrast, **Widget style**, and **Density** through settingsd.
Widget style controls how buttons, fields and panels are drawn; a scheme
brings its own style unless you pick one. Density offers Compact (0.85),
Comfortable (1.0), and Spacious (1.2), scaling spacing and control heights.
Off-step values remain stored and show **Custom (x)**. Wallpaper opens
its existing editor from this page.

**Windows** holds title bars (Automatic, Drawn by the app, Drawn by the
system) and window-button side. Automatic shows its current result; button
side is editable when the effective title bar is drawn by the app.

Each change applies to the whole session as you make it, as in GNOME's
settings: the apps that follow the session look (Prefs itself, Workshop,
and the portal's clients) change with it. Only the setting you changed is
written. Changes made quickly one after another are sent in turn, never
lost.

If settingsd refuses a change, Preferences keeps it and says so: **Apply**
(Ctrl+S) tries again and **Revert** drops it. If the appearance is changed
elsewhere at the same moment (another window, or an agent), Preferences follows
it on every setting you did not touch. If it changed one you did, Preferences
names it and applies nothing until you choose **Keep my changes** or
**Revert**. Every change is checked against the revision you were shown, so
it never lands on a change you have not seen. If its answer is
lost, Preferences asks settingsd for the change's receipt before anything else is
applied (**Check again** asks now); if settingsd no longer holds one, the
session's current state decides what the status line says.

## Text & Fonts

**Text & Fonts** shows the interface, heading and monospace faces currently
installed on the window, with previews. Interface and monospace family pickers
list installed families, sorted without duplicates; the monospace picker lists
only families with monospaced faces. The font list is read once in the background.
The first row restores the design default (Inter or JetBrains Mono), stored as
null. A stored family absent from the list stays visible, marked **missing**.
Interface weight and size controls sit beside the family choices.
The size reset restores **Design default**, stored as null; an unset size is
labelled with the design's pixel size. Viewing or tabbing through controls
does not save a value.

Family choices use the same fenced drafts and conflict handling as Appearance.
The toolkit currently installs
bundled Inter and JetBrains Mono regardless of those stored names, including
after a theme reload or restart. Other families take effect when the toolkit
supports loading them; this stage adds no font loader. The faces grid and
previews show the actual egui font stacks, rather than the saved request.

## Accessibility

**Larger text** (50–300%, in 5% steps) multiplies every text size,
including readers, editors and the terminal grid. Reset restores 100%.
**Reduce motion** reduces animation in apps and the compositor.
Related rows open Contrast in Appearance and Text size in Text & Fonts.

## Apps

**Apps → Term** groups font family, size, weight and padding under **Text**;
scrollback, scroll speed, cursor and copy on select under **Behaviour**;
TERM, pane border and focus ring under **Advanced**. An unset family says
**Same as Monospace (…)**, following `appearance.mono_family`.
Reset terminal and existing apply behaviour remain available.

**Apps → Ced** holds the editor's font, size, padding and light content.
Email's **Reading** and **Writing** cards live on **Apps → Email**.
Content sizes remain absolute in this step; no settings keys change.

### Email

**Apps → Email** holds the
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
client can read, and Preferences neither keeps nor shows them. **Set password**
hands the typed password to Email itself (`email.account.password.set`),
which keeps it in its own private file, and empties the field; the field
gets the password back only if Email refuses it. If Email is not running,
Preferences starts it (`apps.launch`) and waits up to about 16 seconds for it to
come onto the Bus. Under the field, Preferences says whether Email holds a
password for the account, as Email reports it (`email.accounts`), or that
this is unknown because Email is not running. **Connect now** asks Email
to connect the account (`email.account.connect`). **Forget password…**
asks first (Cancel is the default), then has Email delete the password it
keeps; it is the only time Preferences sends an empty password, and it starts
Email if needed, as Set password does. **Remove…** goes through Email
when it may be running (`email.account.remove`): Email removes the account
from settingsd and forgets its password itself, and refuses while the
account is still being migrated from its old configuration
(`MIGRATION_PENDING`, shown as Email gives it). When Email is not running,
Preferences removes the account from settingsd directly and, once settingsd shows
it gone, asks a running Email to forget the password; Preferences does not start
Email for this, and if Email is away or refuses, the status line says the
password may still be held. An account still being migrated comes back
when Email next starts, by design.
An Email too old to know these commands is reported as such, not retried.

```mix
-- What Preferences shows: accounts (with has_password), default, mail options.
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
For scripts that do not need Preferences open, call `settings.apply` on the
`email.*` paths directly, and `email.account.password.set` on Email.

## For agents

Preferences answers on the Bus as `prefs`. `prefs.info` reports the whole state;
`prefs.apps` the rows, selection and notes; `prefs.appearance` the session
look, the expected look while a change settles, any held or conflicting
edits and the revision read; `prefs.appearance.set`
changes it at once (`scheme`, `style` with `own` for the scheme's own, `mode`,
`contrast`, `decorations`, `caption_side`, `window_style`, `menu_style`, `menu_side`, `font_family`, `mono_family`,
`text_scale`, `density`, `reduced_motion`). Family values are strings or null;
text scale and density reset with 1.0, and reduced motion takes a boolean.
Desktop → Windows places the window style picker beside caption placement.
Its choices and labels come from the design registry; compd applies the chosen
server-side furniture live. It does not change application-drawn captions.
The same section offers Menus (Menu bar, Menu button, In the title bar) and
Menu button (Left, Right). The side picker is enabled when menus are collapsed.
In the title bar uses compd's button in SSD mode and the app's button in CSD mode.
An offline or aliased app keeps its own menu button in SSD mode too.
The section ids in tree order are `appearance`, `text_and_fonts`, `wallpaper`,
`windows`, `apps_email`, `apps_term`, `apps_ced`, `accessibility`, and `software`.
Navigation actions use `view.section.<id>`; retired section ids have no aliases. Search includes their `ui.*`
keys and both font-family keys. `prefs.appearance` and `prefs.appearance.set`
are version 0.3.0. `prefs.info` version 0.5.0 and `prefs.apps` version 0.3.0
add `release_status` (Software's progress, install result or refusal text) and
`release_failed` (whether that outcome failed). These stay separate from
settings feedback in `status` and `failed`; a quiet release read preserves the
last operation's outcome.
The reply includes `problem` (the unresolved settings failure, or null) and
`problem_source` (its page id, or null). Problems appear on their own page,
with a link from other pages. A refused edit stays until its owner's next
successful write or until the person dismisses it with × on that page.
Reverting or retiring a look draft clears its look problem. A settings-service
outage also clears on a recovered read. Email
account tasks report their outcome inline and never create settings problems.
`prefs.section {name[, group]}` opens a registry section.
`app.describe` lists the section ids and their searchable metadata.
For Software, `group` selects `mixos-desktop`, `mixos-server`, `creative` or
`other`. `prefs.select {app}` opens the app's group and selects its row.
`prefs.info` and `prefs.appearance` include the current section id and category.
`prefs.search {query}` returns `matches`, each with `id` and `category`, in
sidebar order, except that `system` ranks Software first. It uses the same
search as the sidebar and leaves the window alone.
`prefs.email`, `prefs.email.set` and `prefs.email.password` are described
under Apps → Email above.
`prefs.execute {id}` runs any command a button
or menu would (`apps.check`, `apps.install`, `appearance.apply`,
`appearance.revert`, …), and `prefs.actions` lists them with their
enablement. The window is drivable like every MixOS app
(`prefs.ui.*`, `prefs.window`). The commands are registered in
`docs/docs/spec/bus/commands.conf.mix`.

For scripted work, call the services directly (`releases.*`,
`settings.*`): Preferences is their window, not a second implementation.

## Wallpaper

Open **Wallpaper** from **Appearance**, then choose the default or an output, then Theme,
Colour or Image. Colour accepts a picker or `#RRGGBB`. Browse opens the native
image browser at the installed wallpaper folder, or Pictures in your home
folder. It lists PNG/JPEG files and folders without blocking the window.
Changes stay in Preferences until **Apply wallpaper**. **Revert wallpaper** drops
them; **Refresh wallpaper** reads the current state again.

An output's **Use default** removes just that override. Editing the default
preserves every output override. Images currently crop to fill; fit controls
and slideshows will follow when backdrop supports them.

Without Backdrop on the session Bus, Wallpaper shows an unavailable page state
and creates no problem links. Local edit refusals stay on Wallpaper. A refusal
of a submitted apply shows its reason,
keeps the draft and leaves the last good wallpaper in place. Preferences checks for
intervening changes before sending the whole document; conflicts require
Revert and a fresh edit. This check is best effort: backdrop does not yet
provide an atomic compare-and-set operation. A lost reply is checked by a
state read, never automatically resent.

Wallpaper and browser state are included in `prefs.info`, wallpaper state in
`prefs.appearance`. Commands `wallpaper.apply`, `wallpaper.revert`,
`wallpaper.refresh` and `wallpaper.browse` are available through
`prefs.execute`; the normal `prefs.ui.*` commands drive the controls and browser.

## Running and testing

Run with `prefs [--noded-url URL]`; a second launch raises the first window
(`prefs.show`). The Bus connection is [citizen](../citizen/README.md).

The source is [`apps/prefs`](https://github.com/markc/mixos/tree/main/apps/prefs):

| Path | What |
|---|---|
| `crates/preferences` | the headless engine: sections, rows, notes, one operation at a time, the `prefs.*` commands |
| `crates/preferences/src/navigation.rs` | the section registry, page registry, software groups and search |
| `crates/preferences/src/appearance.rs` | Appearance: the look, families, scale, density and motion, with drafts, fenced applies and receipts for lost replies |
| `crates/preferences/src/email.rs` | Apps → Email: account changes planned and checked, fenced applies, passwords handed to Email |
| `src/commands.rs` | every action as a registered command (menus, buttons, shortcuts, `prefs.execute`) |
| `src/view.rs` | the egui rendering; returns what the person did |
| `src/shell.rs` | events in, effects out, in order; drained shutdown |
| `i18n/en/prefs.ftl` | the strings |

Test with `cargo test -p preferences -p prefs`. Regenerate the snapshots
with `UPDATE_SNAPSHOTS=1` and look at the images before committing them.

## Macro Bus commands

The shared [macro Bus surface](/macros/#bus-commands) uses this process's
catalogue and runner. Subscribe to `<service>.macros.finished` for completion, where `service` is
the instance’s registered Bus name (for example `term-1234` for a second Term
instance). Commands keep the app’s canonical prefix, such as `prefs.macros.run`.

| Command | Arguments | Result |
|---|---|---|
| `prefs.macros.list` | `{}` | Catalogue and diagnostics |
| `prefs.macros.reload` | `{}` | Catalogue and diagnostics |
| `prefs.macros.run` | `{id}` | Acceptance and run number |
| `prefs.macros.status` | `{}` | Current or last run |
| `prefs.macros.output` | `{}` | Retained output and dropped flag |
| `prefs.macros.stop` | `{}` | Whether termination was requested |
| `prefs.macros.new` | `{title}` | Create a user per-app macro; `{id,title,path,scanned}`, without opening an editor; scan failures add `warning` and preserve the file |
