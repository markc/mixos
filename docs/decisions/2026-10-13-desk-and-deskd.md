---
title: Decision — desk and deskd
description: The desktop's applications ship as one multi-call binary, desk, and its session services as another, deskd. Each app and service keeps its name as a link, so nothing that calls them changes, and one copy of egui, wgpu and the rest serves them all.
---

# Decision — desk and deskd

Status: accepted 2026-10-13. Follows the multi-call precedent of
[rec and play](2026-10-11-rec-and-play.md); within the naming rules of
[the source layout](2026-10-04-source-layout.md) (AGENTS.md §3).

## The decision

- **`desk` is the desktop's applications in one binary.** The GUI apps
  (busviewer, ced, dopus, prefs, screenshot, backdrop, and term when it
  lands) build into one executable, `desk`. Each app's name is a link to
  it in `/opt/mixos/bin`, and the binary runs the app it was started as.
  `desk <app> [args]` does the same, and `desk` alone lists the apps.
- **`deskd` is the desktop's session services in one binary**, by the same
  rule: the services a desktop session runs and a headless host does not
  (portald, settingsd, editd). Each keeps its name as a link and its own
  unit.
- **What stays separate:**
  - `mix` (the shell, on every host);
  - `noded` (the broker, on every host, desktop or not);
  - `hostd` (the host's information and privilege service, on every
    host). It runs as root and is the one way to elevate: it never shares
    a binary with `deskd` or anything else that runs as the session
    account;
  - `compd` (the compositor: its robustness and its dependency set are
    its own).
- **Each app and service is still its own process.** A crash ends one app,
  not the others; units, desktop entries and scripts keep calling
  `/opt/mixos/bin/<name>`.
- **Each crate keeps a `run` entry point and its own `main`**, so
  `cargo run -p ced` and every per-app test work as before. The multi-call
  crates only dispatch. Identity (build info, app ID, version) is passed by
  each app's `run`, never taken from the multi-call crate's package.

## Why

Every egui app links its own egui, wgpu, naga, fonts and icons: about
22 MB of each binary is that shared runtime, and an app's own code is 1 to
3 MB. A measured prototype put six apps (backdrop, busviewer, ced, dopus,
prefs, screenshot; release profile, fat LTO) in one binary: 128.7 MB
separately, 29.4 MB together. Link-time optimisation removes the
duplicates, and running apps share one mapped executable, so the runtime
is in memory once.

Rust dynamic libraries were rejected: there is no stable Rust ABI, libstd
would ship as a shared library, LTO stops at the boundary, and generic code
is still copied into each app. A C-ABI wrapper around egui would be a
second UI API.

## The names

- `desk` and `deskd` are plain nouns, a pair in the `<role>d` convention
  (§3.1, §3.4), unique in the dependency graph, and installed by no Arch
  package (checked 2026-10-13; the other distributions' file lists are
  checked before first use).
- The layers read in order:
  - **MixOS** is the system.
  - **mix** is its shell and language.
  - **noded** is the broker on every host.
  - **hostd** is the host's information and privileged actions, as root,
    on every host.
  - **compd**, **desk** and **deskd** are the desktop.

  A headless host runs `mix`, `noded` and `hostd`; a desktop adds the
  other three.
- A machine running the desktop is "a desk"; numbered instances (a desk
  and its number) are names of machines, not of these binaries.

## Not decided here

When each app and service moves in, the install layout of the links, and
how `component_install.mix` and the unit collector treat a multi-call
binary. Those land with the implementation.
