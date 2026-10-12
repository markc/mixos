---
title: Decision — one repository, install profiles
description: Server programs and the desktop stay in one MixOS repository and one Cargo workspace. The split between them is made where it matters, in what gets built and installed, by install profiles that every shipping component declares.
---

# Decision — one repository, install profiles

Status: accepted 2026-10-12. AGENTS.md §2.3 (identity and metadata), §2.5
(Mix components) and §10 (the checklist) are amended in the same change
that adds the `profiles` field and its gates.

## The question

MixOS has two audiences: a desktop (the compositor, the apps and their
settings services) and servers (mail, web, blobs, DNS and the mesh daemons
on headless nodes). The server programs have not moved into this
repository yet. Should they get a repository of their own?

## The decision

- **Everything stays in this repository and its one Cargo workspace.**
  Desktop, server, command-line and library code alike. There is no second
  MixOS source repository for server programs.
- **The split is an install split, not a source split.** Every shipping
  component (every `app`, `service` and `cli`) declares the **install
  profiles** that include it:

  | Profile | Meaning |
  |---|---|
  | `base` | On every MixOS system: the shell and the Bus node |
  | `server` | A headless node's services |
  | `desktop` | The desktop session: compositor, apps, their services |

  A component may be in more than one profile. Libraries and a component's
  private crates declare none: they are installed only as part of what
  needs them.

- **Where it is declared.** A Rust component adds `profiles` to its
  `[package.metadata.mixos]`:

  ```toml
  [package.metadata.mixos]
  component = "webd"
  kind = "service"
  layer = "core"
  contract = "public"
  profiles = ["server"]
  ```

  A Mix component adds the same `profiles` list to its `component.mx`.
  Install profiles are unrelated to Cargo's build profiles
  (`release`, `release-fast`): any install profile can be built with any
  of them.

- **The gates.**
  - The component gate refuses an `app`, `service` or `cli` without a
    `profiles` list, an unknown profile, and `profiles` on a library or a
    private crate.
  - The layering gate refuses a `base` or `server` component whose
    dependency closure contains a `desktop`-layer package. A server never
    builds egui, wgpu or smithay.
  - The component index (`docs/dev/components.md`) shows each component's
    profiles.
- **Building and installing by profile.** A profile resolves to a list of
  packages to build (`cargo build -p …`), the Mix components to install, and
  their units. One tool answers "what is in `server`?" from the metadata, so
  build scripts, images and installers never keep their own lists. The
  installer for Rust binaries and their units, with the record, rollback
  and drift check that `tools/component_install.mix` gives Mix components,
  is separate work that follows this decision.

### The first assignment

| Component | Profiles |
|---|---|
| `mix-shell` (the `mix` binary), `noded` | `base` |
| `releasesd` | `server` |
| `compd`, `settingsd`, `portald`, `editd`, `busviewer`, `ced`, `dopus`, `prefs` | `desktop` |

Server programs moved in later (mail, web, blobs and the rest) join
`server` as they land.

## Why

- **The two sides share their foundation.** The Bus, noded, Mix, the config
  and props libraries and logging sit under both. A second repository would
  have to consume them as published versions, and they change almost daily.
  Every change to them would become a release here and an upgrade there.
- **Changes cut across both.** The verb registry's move to classes changed
  the registry, its gate and every application in one commit. Across two
  repositories that is coordinated pull requests that can drift apart.
- **The separation that matters is already enforced** by the layering and
  dependency gates (AGENTS.md §6): headless crates never depend on smithay,
  egui, wgpu or Bevy. A repository boundary adds nothing to that. The
  profile gate extends it from crates to what a server installs.
- **This was decided once already.** The
  [source layout decision](2026-10-04-source-layout.md) rejected one
  repository per component so that one dependency graph can be gated,
  shared code consolidated, and changes made atomically. A desktop/server
  split is the same proposal in two pieces, and nothing it relied on has
  changed.
- **What people actually need separated is the build and the install.** A
  mail server should never compile or install the compositor. Profiles say
  that precisely, from data, in one place.

## Alternatives

- **A second repository for server programs.** Rejected for the reasons
  above. It would be the right call if the server side ever needed a
  different licence, private code, its own team or its own release cadence.
  None of these applies. Private deployment detail already lives outside
  this repository.
- **Cargo features or a second workspace in this repository.** Rejected. A
  second workspace means two lockfiles and two dependency graphs, which is
  what AGENTS.md §0 rules out. Features select code inside a crate, not
  which programs a system gets.
- **A top-level `server/` directory.** Rejected: the top level is
  classified by kind only (AGENTS.md §1). A web server is a service like
  any other; which systems it belongs on is data.

## Consequences

- One `Cargo.lock`: a dependency bump for the desktop also moves the
  servers' resolved versions. The gates and each component's tests catch a
  break; the alternative was version drift between repositories.
- Building a server set compiles only that set's closure (`cargo build -p`),
  so headless builds stay small.
- AGENTS.md §2.3 and §2.5 gain the `profiles` field, and §10's checklist
  asks for it.
