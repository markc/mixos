---
title: Components
description: Every MixOS component with its kind, install profiles, source and page, generated from the component metadata.
generated: by docs/_build/gen-site.mix from tools/component_index.mix --json; do not edit
---

# Components

Every MixOS component, generated from the component metadata: each principal
package's `[package.metadata.mixos]`, or a Mix component's `component.mx`, read by
`tools/component_index.mix`. Install profiles come only from a declared `profiles`
list; a shipping component that has not declared one yet is unclassified. By
profile: [desktop](desktop.md) and [server](server.md). This page describes the current `main` branch of MixOS.

| Component | Kind | Profiles | Source | Description |
|---|---|---|---|---|
| [assets](../assets/README.md) | lib | none (library) | [`libs/assets`](https://github.com/markc/mixos/tree/main/libs/assets) | The resolver for a pinned, immutable set of fonts, icons and emoji on the local disk, which finds, verifies and reads a published set without any downloader or daemon. |
| [backdrop](../backdrop/README.md) | app | unclassified | [`apps/backdrop`](https://github.com/markc/mixos/tree/main/apps/backdrop) | The MixOS desktop background — drawn by its own program on the compositor's background layer, on every output, at native and fractional scale. |
| [bridged](../bridged/README.md) · [changelog](../bridged/changelog.md) | service | unclassified | [`services/bridged`](https://github.com/markc/mixos/tree/main/services/bridged) | How bridged puts an external application with a line-delimited JSON control channel on the Bus, one instance per application, and what that exposes. |
| [buildinfo](../buildinfo/README.md) | lib | none (library) | [`libs/buildinfo`](https://github.com/markc/mixos/tree/main/libs/buildinfo) | Compile-time build provenance for MixOS binaries, and the --version contract every binary answers. |
| [bus](../bus/README.md) | lib | none (library) | [`libs/bus`](https://github.com/markc/mixos/tree/main/libs/bus) | The Bus wire format and the broker client library, with a supervised connection to noded that survives broker restarts. |
| [busviewer](../busviewer/README.md) | app | unclassified | [`apps/busviewer`](https://github.com/markc/mixos/tree/main/apps/busviewer) | BusViewer is the MixOS desktop app that lists the services and verbs on this node's Bus, shows each verb's description and the mesh peers, and calls one verb once with a JSON body. |
| [ced](../ced/README.md) | app | unclassified | [`apps/ced`](https://github.com/markc/mixos/tree/main/apps/ced) | The MixOS Editor, a desktop text editor over the edit Bus service, where people and agents edit the same document at once and see each other's carets, selections and changes live. |
| [citizen](../citizen/README.md) | lib | none (library) | [`libs/citizen`](https://github.com/markc/mixos/tree/main/libs/citizen) | An app's supervised native Bus connection, shared by the MixOS apps for registration, calls, replies, single-instance launch and window restore. |
| [compd](../compd/README.md) · [changelog](../compd/changelog.md) | service | unclassified | [`services/compd`](https://github.com/markc/mixos/tree/main/services/compd) | The MixOS Wayland compositor, which owns scanout, rendering, input and capture, and serves the comp Bus service with its workspaces, verbs, topics and props tree. |
| [config](../config/README.md) | lib | none (library) | [`libs/config`](https://github.com/markc/mixos/tree/main/libs/config) | Where MixOS keeps its files and how a conf.mix file is read, with one directory rule for every component and descriptor-safe atomic file helpers. |
| [davproto](../davproto/README.md) | lib | none (library) | [`libs/davproto`](https://github.com/markc/mixos/tree/main/libs/davproto) | CalDAV and CardDAV codecs that convert between JSCalendar or JSContact and iCalendar or vCard, with strong ETags, and no transport or storage. |
| [design](../design/README.md) | lib | none (library) | [`libs/design`](https://github.com/markc/mixos/tree/main/libs/design) | The MixOS design-token system, which compiles a strict-data theme source into resolved colours, metrics, typography and button tables for one scheme, mode and contrast. |
| [dopus](../dopus/README.md) | app | unclassified | [`apps/dopus`](https://github.com/markc/mixos/tree/main/apps/dopus) | MixOS's twin-pane file manager — two folder trees side by side, Places and Properties sidebars, drag and drop between panes, menus and keys for every action, and a Bus surface for agents that reads… |
| [edit](../edit/README.md) | lib | none (library) | [`libs/edit`](https://github.com/markc/mixos/tree/main/libs/edit) | Shared headless text buffers used by editd and Ced, with revision checks, transactions, operational transforms, per-origin undo, anchors and search. |
| [editd](../editd/README.md) · [changelog](../editd/changelog.md) | service | unclassified | [`services/editd`](https://github.com/markc/mixos/tree/main/services/editd) | The edit service, which holds shared text buffers that people and agents edit together, with revision checks, per-origin undo, anchors, atomic save, external-change detection and recovery. |
| [editor](../editor/README.md) | lib | none (library) | [`libs/editor`](https://github.com/markc/mixos/tree/main/libs/editor) | The shared plain-text editor widget for MixOS egui applications, a virtualised monospace view over the headless editing model that works on a local file or a shared buffer. |
| [editor-model](../editor-model/README.md) | lib | none (library) | [`libs/editor-model`](https://github.com/markc/mixos/tree/main/libs/editor-model) | The headless text-editing model shared by MixOS editors, turning commands and motions into edits and keeping a local mirror of an edit-service buffer in step with the server, plus highlighting and… |
| [email](../email/README.md) | app | unclassified | [`apps/email`](https://github.com/markc/mixos/tree/main/apps/email) |  |
| [expr](../expr/README.md) | lib | none (library) | [`libs/expr`](https://github.com/markc/mixos/tree/main/libs/expr) | The pure expression language of Mix Scenes bindings, which compiles an expression, reports the globals it reads and evaluates it over JSON values under size and time limits. |
| [hostd](../hostd/README.md) · [changelog](../hostd/changelog.md) | service | unclassified | [`services/hostd`](https://github.com/markc/mixos/tree/main/services/hostd) | How hostd answers questions about the host and carries out privileged host changes through closed, typed verbs behind standing grants, receipts and rollback, so agents never need sudo. |
| [inputd](../inputd/README.md) · [changelog](../inputd/changelog.md) | service | unclassified | [`services/inputd`](https://github.com/markc/mixos/tree/main/services/inputd) | How inputd turns keys into Bus verbs below the compositor, keeps the user keys for the person, and holds its keyboard grab only while its own VT is active. |
| [logging](../logging/README.md) | lib | none (library) | [`libs/logging`](https://github.com/markc/mixos/tree/main/libs/logging) | Shared tracing, sink selection, reload handles and process statistics for MixOS services and the Mix shell. |
| [mimedb](../mimedb/README.md) | lib | none (library) | [`libs/mimedb`](https://github.com/markc/mixos/tree/main/libs/mimedb) | A library that reads the freedesktop shared-mime-info database and decides a file's type from its name and content the way GLib does, so MixOS names a file's type as the desktop does. |
| [mix](../mix/README.md) · [changelog](../mix/changelog.md) | lib | none (library) | [`libs/mix`](https://github.com/markc/mixos/tree/main/libs/mix) |  |
| [mix-shell](../mix-shell/README.md) · [changelog](../mix-shell/changelog.md) | cli | unclassified | [`cli/mix-shell`](https://github.com/markc/mixos/tree/main/cli/mix-shell) | The Mix shell library, holding the interactive line editor, the command classifier, the native-session bootstrap and the Bus citizen runtime that the mix binary runs. |
| [monitor](../monitor/README.md) | app | desktop | [`apps/monitor`](https://github.com/markc/mixos/tree/main/apps/monitor) | Monitor is the MixOS task manager, showing the processes and performance that monitord reports for this node or any node on the mesh, with End task and Kill for this node's processes. |
| [monitord](../monitord/README.md) · [changelog](../monitord/changelog.md) | service | base | [`services/monitord`](https://github.com/markc/mixos/tree/main/services/monitord) | How monitord samples a host's CPU, memory, disks, network and processes, keeps history in memory and SQLite, and answers read verbs on the Bus, so any node's load can be read from anywhere on the… |
| [noded](../noded/README.md) · [changelog](../noded/changelog.md) | service | unclassified | [`services/noded`](https://github.com/markc/mixos/tree/main/services/noded) | The MixOS Bus broker, which handles service registration, discovery, routing, subscriptions, authenticated local session grants and mesh peering between nodes. |
| [notifyd](../notifyd/README.md) | service | unclassified | [`services/notifyd`](https://github.com/markc/mixos/tree/main/services/notifyd) | How notifyd keeps the desk's notifications, shows them as toasts and in the Notifications page, and tells an agent or app which button the human pressed. |
| [portald](../portald/README.md) | service | unclassified | [`services/portald`](https://github.com/markc/mixos/tree/main/services/portald) | How portald serves the XDG Settings portal on the session bus, where the appearance values come from, and how applications and agents read them. |
| [prefs](../prefs/README.md) | app | unclassified | [`apps/prefs`](https://github.com/markc/mixos/tree/main/apps/prefs) | MixOS's preferences window — a sidebar of editors: Applications follows, installs, updates, rolls back and removes upstream releases; Appearance sets the session's colour scheme, style, mode,… |
| [props](../props/README.md) | lib | none (library) | [`libs/props`](https://github.com/markc/mixos/tree/main/libs/props) | Shared property read types, revision handling and optional Bus publication builders for noded, Mix and editd. |
| [releasesd](../releasesd/README.md) | service | unclassified | [`services/releasesd`](https://github.com/markc/mixos/tree/main/services/releasesd) | How releasesd follows upstream application releases on GitHub and installs their Linux builds per user, verified, with rollback, from the command line or the Bus. |
| [scene](../scene/README.md) | lib | none (library) | [`libs/scene`](https://github.com/markc/mixos/tree/main/libs/scene) | The Mix Scenes document core, which parses, lints, resolves and diffs scene documents and their bindings without a renderer or interpreter. |
| [screenshot](../screenshot/README.md) | app | unclassified | [`apps/screenshot`](https://github.com/markc/mixos/tree/main/apps/screenshot) | MixOS's screenshot app — Print Screen for a region, Shift+Print for the screen, Alt+Print for a window; arrows, boxes, pens, highlighter, text, numbered markers, crop and redaction that stay… |
| [settings](../settings/README.md) · [changelog](../settings/changelog.md) | lib | none (library) | [`libs/settings`](https://github.com/markc/mixos/tree/main/libs/settings) | Headless desktop settings types, validation, resolution and the shared consumer that apps and the compositor use to follow the settings authority. |
| [settingsd](../settingsd/README.md) · [changelog](../settingsd/changelog.md) | service | unclassified | [`services/settingsd`](https://github.com/markc/mixos/tree/main/services/settingsd) | The headless desktop settings authority, the one writer for a session profile's settings, with durable receipts and retained change publication on the Bus. |
| [strict](../strict/README.md) | lib | none (library) | [`libs/strict`](https://github.com/markc/mixos/tree/main/libs/strict) | The MixOS strict-data format, the grammar of conf.mix and data.mix files, loaded as data and never executed. |
| [syntax](../syntax/README.md) | lib | none (library) | [`libs/syntax`](https://github.com/markc/mixos/tree/main/libs/syntax) | The syntax highlighting library for MixOS text editors, which runs Microsoft Edit's compiled lsh language definitions line by line over any chunked text store. |
| [system](../system/README.md) | cli | unclassified | [`cli/system`](https://github.com/markc/mixos/tree/main/cli/system) | The system binary, mix, which is the Mix shell and every headless MixOS daemon in one file, choosing its personality from the name it is started under. |
| [term](../term/README.md) · [changelog](../term/changelog.md) | app | unclassified | [`apps/term`](https://github.com/markc/mixos/tree/main/apps/term) | The MixOS terminal, with tabs, split panes and a GPU-drawn grid, driven by the session settings and fully scriptable over the Bus. |
| [test-broker](../test-broker/README.md) | lib | none (library) | [`libs/test-broker`](https://github.com/markc/mixos/tree/main/libs/test-broker) | A development fixture that runs the real noded modules in an isolated broker runtime for the Term-to-Mix native session acceptance tests. |
| [thumbs](../thumbs/README.md) | lib | none (library) | [`libs/thumbs`](https://github.com/markc/mixos/tree/main/libs/thumbs) | File thumbnails made the freedesktop way, read from and written to the shared thumbnail cache and generated by the system's installed thumbnailers in their own processes. |
| [toolkit](../toolkit/README.md) | lib | none (library) | [`libs/toolkit`](https://github.com/markc/mixos/tree/main/libs/toolkit) | The shared shell for MixOS egui applications, giving every app the same theme, fonts, icons, title bar, menus, chrome components, command registry, Fluent strings and Bus drive verbs. |

## Packages

Every workspace package and Mix component, with the component that owns it.
Private crates stay under their owning component. Dependency closure is
checked separately by the layering gate.

| Package | Component | Kind | Layer | Contract | Version |
|---|---|---|---|---|---|
| [backdrop](https://github.com/markc/mixos/blob/main/apps/backdrop/Cargo.toml) | backdrop | app | desktop | none | 0.1.1 |
| [wallpaper](https://github.com/markc/mixos/blob/main/apps/backdrop/crates/wallpaper/Cargo.toml) | backdrop | app | core | none | 0.1.1 |
| [busviewer](https://github.com/markc/mixos/blob/main/apps/busviewer/Cargo.toml) | busviewer | app | desktop | public | 0.1.1 |
| [inspector](https://github.com/markc/mixos/blob/main/apps/busviewer/crates/inspector/Cargo.toml) | busviewer | app | core | none | 0.1.1 |
| [ced](https://github.com/markc/mixos/blob/main/apps/ced/Cargo.toml) | ced | app | desktop | public | 0.1.1 |
| [documents](https://github.com/markc/mixos/blob/main/apps/ced/crates/documents/Cargo.toml) | ced | app | core | none | 0.1.1 |
| [dopus](https://github.com/markc/mixos/blob/main/apps/dopus/Cargo.toml) | dopus | app | desktop | public | 0.1.1 |
| [dopus-core](https://github.com/markc/mixos/blob/main/apps/dopus/crates/dopus-core/Cargo.toml) | dopus | app | core | none | 0.1.1 |
| [files](https://github.com/markc/mixos/blob/main/apps/dopus/crates/files/Cargo.toml) | dopus | app | core | none | 0.1.1 |
| [email](https://github.com/markc/mixos/blob/main/apps/email/Cargo.toml) | email | app | desktop | public | 0.1.1 |
| [jmap](https://github.com/markc/mixos/blob/main/apps/email/crates/jmap/Cargo.toml) | email | app | core | none | 0.1.1 |
| [monitor](https://github.com/markc/mixos/blob/main/apps/monitor/Cargo.toml) | monitor | app | desktop | public | 0.1.1 |
| [vitals](https://github.com/markc/mixos/blob/main/apps/monitor/crates/vitals/Cargo.toml) | monitor | app | core | none | 0.1.1 |
| [prefs](https://github.com/markc/mixos/blob/main/apps/prefs/Cargo.toml) | prefs | app | desktop | public | 0.1.1 |
| [preferences](https://github.com/markc/mixos/blob/main/apps/prefs/crates/preferences/Cargo.toml) | prefs | app | core | none | 0.1.1 |
| [screenshot](https://github.com/markc/mixos/blob/main/apps/screenshot/Cargo.toml) | screenshot | app | desktop | public | 0.1.1 |
| [annotation](https://github.com/markc/mixos/blob/main/apps/screenshot/crates/annotation/Cargo.toml) | screenshot | app | core | none | 0.1.1 |
| [term](https://github.com/markc/mixos/blob/main/apps/term/Cargo.toml) | term | app | desktop | public | 0.4.0 |
| [term-core](https://github.com/markc/mixos/blob/main/apps/term/crates/term-core/Cargo.toml) | term | app | core | none | 0.1.1 |
| [term-render](https://github.com/markc/mixos/blob/main/apps/term/crates/term-render/Cargo.toml) | term | app | desktop | none | 0.1.1 |
| [mix-shell](https://github.com/markc/mixos/blob/main/cli/mix-shell/Cargo.toml) | mix-shell | cli | mix | public | 0.116.0 |
| [system](https://github.com/markc/mixos/blob/main/cli/system/Cargo.toml) | system | cli | core | none | 0.1.1 |
| [assets](https://github.com/markc/mixos/blob/main/libs/assets/Cargo.toml) | assets | lib | core | public | 0.1.1 |
| [buildinfo](https://github.com/markc/mixos/blob/main/libs/buildinfo/Cargo.toml) | buildinfo | lib | core | none | 0.1.1 |
| [bus](https://github.com/markc/mixos/blob/main/libs/bus/Cargo.toml) | bus | lib | core | none | 0.1.1 |
| [citizen](https://github.com/markc/mixos/blob/main/libs/citizen/Cargo.toml) | citizen | lib | core | none | 0.1.1 |
| [config](https://github.com/markc/mixos/blob/main/libs/config/Cargo.toml) | config | lib | core | none | 0.1.1 |
| [davproto](https://github.com/markc/mixos/blob/main/libs/davproto/Cargo.toml) | davproto | lib | core | none | 0.1.1 |
| [design](https://github.com/markc/mixos/blob/main/libs/design/Cargo.toml) | design | lib | core | none | 0.1.1 |
| [edit](https://github.com/markc/mixos/blob/main/libs/edit/Cargo.toml) | edit | lib | core | none | 0.1.1 |
| [editor-model](https://github.com/markc/mixos/blob/main/libs/editor-model/Cargo.toml) | editor-model | lib | core | none | 0.1.1 |
| [editor](https://github.com/markc/mixos/blob/main/libs/editor/Cargo.toml) | editor | lib | desktop | none | 0.1.1 |
| [expr](https://github.com/markc/mixos/blob/main/libs/expr/Cargo.toml) | expr | lib | core | none | 0.1.1 |
| [logging](https://github.com/markc/mixos/blob/main/libs/logging/Cargo.toml) | logging | lib | core | none | 0.1.1 |
| [mimedb](https://github.com/markc/mixos/blob/main/libs/mimedb/Cargo.toml) | mimedb | lib | mix | none | 0.1.1 |
| [mix](https://github.com/markc/mixos/blob/main/libs/mix/Cargo.toml) | mix | lib | mix | public | 0.116.0 |
| [props](https://github.com/markc/mixos/blob/main/libs/props/Cargo.toml) | props | lib | core | none | 0.1.1 |
| [scene](https://github.com/markc/mixos/blob/main/libs/scene/Cargo.toml) | scene | lib | core | none | 0.1.1 |
| [settings](https://github.com/markc/mixos/blob/main/libs/settings/Cargo.toml) | settings | lib | core | public | 0.7.0 |
| [strict](https://github.com/markc/mixos/blob/main/libs/strict/Cargo.toml) | strict | lib | core | none | 0.1.1 |
| [syntax](https://github.com/markc/mixos/blob/main/libs/syntax/Cargo.toml) | syntax | lib | core | none | 0.1.1 |
| [test-broker](https://github.com/markc/mixos/blob/main/libs/test-broker/Cargo.toml) | test-broker | lib | core | none | 0.1.1 |
| [thumbs](https://github.com/markc/mixos/blob/main/libs/thumbs/Cargo.toml) | thumbs | lib | core | none | 0.1.1 |
| [toolkit](https://github.com/markc/mixos/blob/main/libs/toolkit/Cargo.toml) | toolkit | lib | desktop | none | 0.1.1 |
| [bridged](https://github.com/markc/mixos/blob/main/services/bridged/component.mx) | bridged | service | desktop | public | 0.2.0 |
| [compd](https://github.com/markc/mixos/blob/main/services/compd/Cargo.toml) | compd | service | desktop | public | 0.2.0 |
| [comp-model](https://github.com/markc/mixos/blob/main/services/compd/crates/comp-model/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [comp-service](https://github.com/markc/mixos/blob/main/services/compd/crates/comp-service/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [decor](https://github.com/markc/mixos/blob/main/services/compd/crates/decor/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [dispatcher](https://github.com/markc/mixos/blob/main/services/compd/crates/dispatcher/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [drivers](https://github.com/markc/mixos/blob/main/services/compd/crates/drivers/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [edges](https://github.com/markc/mixos/blob/main/services/compd/crates/edges/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [effects](https://github.com/markc/mixos/blob/main/services/compd/crates/effects/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [frames](https://github.com/markc/mixos/blob/main/services/compd/crates/frames/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [graphics](https://github.com/markc/mixos/blob/main/services/compd/crates/graphics/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [kms](https://github.com/markc/mixos/blob/main/services/compd/crates/kms/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [ledger](https://github.com/markc/mixos/blob/main/services/compd/crates/ledger/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [model](https://github.com/markc/mixos/blob/main/services/compd/crates/model/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [motion](https://github.com/markc/mixos/blob/main/services/compd/crates/motion/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [native](https://github.com/markc/mixos/blob/main/services/compd/crates/native/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [nested](https://github.com/markc/mixos/blob/main/services/compd/crates/nested/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [outputs](https://github.com/markc/mixos/blob/main/services/compd/crates/outputs/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [policy-host](https://github.com/markc/mixos/blob/main/services/compd/crates/policy-host/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [policy](https://github.com/markc/mixos/blob/main/services/compd/crates/policy/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [protocols](https://github.com/markc/mixos/blob/main/services/compd/crates/protocols/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [recorder](https://github.com/markc/mixos/blob/main/services/compd/crates/recorder/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [render-gles](https://github.com/markc/mixos/blob/main/services/compd/crates/render-gles/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [scene-host](https://github.com/markc/mixos/blob/main/services/compd/crates/scene-host/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [scenegraph](https://github.com/markc/mixos/blob/main/services/compd/crates/scenegraph/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [screencopy](https://github.com/markc/mixos/blob/main/services/compd/crates/screencopy/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [seat](https://github.com/markc/mixos/blob/main/services/compd/crates/seat/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [slots](https://github.com/markc/mixos/blob/main/services/compd/crates/slots/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [surfaces](https://github.com/markc/mixos/blob/main/services/compd/crates/surfaces/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [testkit](https://github.com/markc/mixos/blob/main/services/compd/crates/testkit/Cargo.toml) | compd | test | desktop | none | 0.1.1 |
| [ui](https://github.com/markc/mixos/blob/main/services/compd/crates/ui/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [world](https://github.com/markc/mixos/blob/main/services/compd/crates/world/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [x11-wm](https://github.com/markc/mixos/blob/main/services/compd/crates/x11-wm/Cargo.toml) | compd | service | desktop | none | 0.1.1 |
| [editd](https://github.com/markc/mixos/blob/main/services/editd/Cargo.toml) | editd | service | core | public | 0.2.0 |
| [hostd](https://github.com/markc/mixos/blob/main/services/hostd/Cargo.toml) | hostd | service | core | public | 0.1.0 |
| [inputd](https://github.com/markc/mixos/blob/main/services/inputd/Cargo.toml) | inputd | service | core | public | 0.5.0 |
| [inputd-core](https://github.com/markc/mixos/blob/main/services/inputd/crates/inputd-core/Cargo.toml) | inputd | service | core | none | 0.1.1 |
| [inputd-schema](https://github.com/markc/mixos/blob/main/services/inputd/crates/inputd-schema/Cargo.toml) | inputd | service | core | none | 0.1.1 |
| [monitord](https://github.com/markc/mixos/blob/main/services/monitord/Cargo.toml) | monitord | service | core | public | 0.1.0 |
| [noded](https://github.com/markc/mixos/blob/main/services/noded/Cargo.toml) | noded | service | core | public | 0.18.3 |
| [mesh-trust](https://github.com/markc/mixos/blob/main/services/noded/crates/mesh-trust/Cargo.toml) | noded | service | core | none | 0.1.1 |
| [mesh](https://github.com/markc/mixos/blob/main/services/noded/crates/mesh/Cargo.toml) | noded | service | core | none | 0.1.1 |
| [notifyd](https://github.com/markc/mixos/blob/main/services/notifyd/component.mx) | notifyd | service | desktop | public | 0.1.0 |
| [portald](https://github.com/markc/mixos/blob/main/services/portald/Cargo.toml) | portald | service | core | public | 0.1.0 |
| [releasesd](https://github.com/markc/mixos/blob/main/services/releasesd/component.mx) | releasesd | service | desktop | public | 0.2.0 |
| [settingsd](https://github.com/markc/mixos/blob/main/services/settingsd/Cargo.toml) | settingsd | service | core | public | 0.1.0 |
