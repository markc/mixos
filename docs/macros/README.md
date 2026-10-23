---
title: macros
description: Shared Mix macro discovery, user storage and background execution for every MixOS app.
---

# macros

A macro is an ordinary Mix program (`.mix`) that you run from an app's
**Macros** icon or Help menu. It can use the usual Mix facilities, including
application commands and Bus verbs. Listing macros never executes them.
Macros run with your normal session authority.

## Folders and precedence

Every app scans four tiers, from lowest to highest precedence:

| Tier | Folder | Scope |
|---|---|---|
| Built-in global | `/opt/mixos/share/macros/` | Every app |
| Built-in per-app | `/opt/mixos/share/macros/<app>/` | One app |
| User global | `~/.config/mixos/macros/` | Every app |
| User per-app | `~/.config/mixos/macros/<app>/` | One app |

`<app>` is the component name, for example `ced`, `dopus` or `term`.
An absolute `MIXOS_SHARE` replaces `/opt/mixos/share`. An absolute
`XDG_CONFIG_HOME` replaces `~/.config`; otherwise the configuration root is
`$HOME/.config`. Resolution needs an absolute `HOME` and does not create
folders until you request a write or open a user folder.

A macro's id is its file stem: `hello.mix` has id `hello`. Ids contain only
lowercase ASCII letters, digits, hyphens and underscores. Dotfiles,
directories and other extensions are excluded. `.mx` documents, invalid ids
and symlinks that escape a root are reported as diagnostics. A scan is
shallow; it does not traverse another app's folder.

For the same id, **user per-app > user global > built-in per-app > built-in
global**. The catalogue lists the winning entries by id, labels each tier
and marks entries that shadow lower tiers. Missing folders are fine.

## Using the panel

The Macros icon is the first standard icon, immediately before search when
the app provides search. **Run** starts one macro at a time per window.
Output scrolls in a monospace view. The last 64 KiB of combined standard
output and standard error are retained; the panel says when older output
was dropped. **Stop** kills the process group and reaps the child in the
background. Closing the panel leaves a running macro alone; closing the
window requests termination and background reaping.

**Edit** opens a user macro in Ced, or copies a built-in macro to the
matching user tier before opening it. **New global** and **New for this app**
ask for an id and title, create a short template and open it for editing.
Creating and copying refuse to overwrite an existing file in that tier.
Choose **Reload** after changing a file; folder watching is future work.

**Open global folder** and **Open app folder** create the user folder if
needed and open it in Dopus. When the native editor or file manager is
absent, the system's `xdg-open` handles the path. Discovery, storage and
launchers run on background threads.

## Execution context

The runner starts `/opt/mixos/bin/mix <path>` directly, without a shell
wrapper. An absolute `MIXOS_MIX` selects another Mix binary. Its working
directory is your home, and it receives:

| Variable | Value |
|---|---|
| `MIXOS_MACRO_APP` | App component name |
| `MIXOS_MACRO_ID` | Macro file stem |
| `MIXOS_MACRO_TIER` | `builtin-global`, `builtin-app`, `user-global` or `user-app` |
| `MIXOS_MACRO_PATH` | Full macro path |

For example, save this as `~/.config/mixos/macros/hello.mix`:

```mix
-- title: Hello desktop
-- SPDX-License-Identifier: MIT OR Apache-2.0

print("Hello from " .. env("MIXOS_MACRO_APP"))
```

The first `-- title:` in the leading comment header supplies the displayed
title; otherwise the id is shown. Header reads are bounded to 8 KiB.

## Library boundaries

`libs/macros` is a core library with no egui dependency. It resolves roots,
discovers files, creates or copies user macros and owns the background
runner. `toolkit::macro_panel` accepts plain display data and returns
actions. `toolkit::macros::State` joins that widget to the core, with
`toggle`, `reload` and `show` methods. Its per-window adapter keeps runtime
resources in the egui context while each app keeps the panel's open flag
in its UI state. This keeps application engines below the GUI boundary in
the application layering gate.

Bus-specific macro verbs, filesystem watching and per-app context adapters
are outside this first slice.
