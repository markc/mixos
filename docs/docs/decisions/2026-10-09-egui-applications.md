---
title: Decision — egui applications, engine first
description: MixOS applications and desktop furniture are built on egui and wgpu, with behaviour in engines and every action a named command reachable from the UI, the command line and the Bus.
---

# Decision — egui applications, engine first

Status: accepted 2026-10-09.

## The decision

- **The UI toolkit is egui/eframe on wgpu, from crates.io.** MixOS vendors no UI
  toolkit and tracks egui's current release deliberately.
- **Engine first.** An application's behaviour lives in pure-data engine
  crates that know nothing about the UI. The egui crate is a thin shell that
  renders state and dispatches commands.
- **Every action is a command.** A command has a stable id, a label, a menu
  path, an optional shortcut, an enablement rule and a handler. Menus,
  shortcuts and the command palette are generated from the registry. The UI,
  the `mix` command line and ABP Bus verbs all dispatch commands by id, so
  everything a person can do in an application an agent can do too, locally or
  across the mesh.
- **UI state is one serialisable struct.** Agents can read it, drive it and
  restore it over the Bus.
- **One MixOS theme.** Colours, metrics, radii and type come from design
  tokens. Hard-coded colours are refused by a gate.
- **Tests and offscreen snapshots are the gate** for UI changes, rendered
  without a window and checked as images.
- **Rendering is GPU-first through wgpu.** Specialised CPU renderers (terminal
  and editor text) draw into textures. A software adapter is the fallback.
- **External applications are welcome without being forked.** An application
  that offers a line-delimited JSON control channel can be bridged to the Bus
  as a set of verbs by one generic adapter, and runs inside the MixOS session
  unmodified.

## Why

The approach makes every application agent-operable by construction, keeps
behaviour testable without a display, and rests on a mature toolkit with a
stable release cadence.

## Consequences

- Applications and the desktop furniture enter this tree as they are written
  to this standard.
- Performance claims are measured on real hardware and recorded with their
  workload before and after a change.
