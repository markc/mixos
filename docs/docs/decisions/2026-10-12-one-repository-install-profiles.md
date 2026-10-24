---
title: Decision — one repository, install profiles
description: Server programs and the desktop stay in one MixOS repository and one Cargo workspace. The split between them is made where it matters, in what gets built and installed, by install profiles that every shipping component declares.
---

# Decision — one repository, install profiles

Status: accepted 2026-10-12; amended 2026-10-12 after review. AGENTS.md
§2.3 (identity and metadata), §2.5 (Mix components) and §10 (the
checklist) are amended in the same change that adds the `profiles` field
and its gate.

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
  component declares the **install profiles** that include it:

  | Profile | Meaning |
  |---|---|
  | `base` | On every MixOS system: the shell and the Bus node |
  | `server` | What a headless node may run |
  | `desktop` | The desktop session: compositor, apps, their services and the session's own wiring |

- **Composition.** `server` and `desktop` each include `base`: resolving
  either one yields its own components plus `base`'s. A system may install
  both. A resolved set is the union of its profiles, each component once.
- **Who declares.** A component's *principal* package declares `profiles`:
  the package whose name is its component (the directory under `apps/`,
  `services/` or `cli/`), or a Mix component's `component.mx`. The list is
  non-empty, has no duplicates and names only the three profiles. The field
  is absent on libraries, on a component's private crates (whatever `kind`
  they inherit from their owner), and on `tool` and `test` packages.
  Libraries are never installed on their own; they are linked into what
  needs them. The session's targets and wiring (`session/`) belong to
  `desktop`; when the `session` kind lands, it declares `profiles` like any
  other shipping component.
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
- **Naming.** Install profiles are unrelated to Cargo's build profiles
  (`release`, `release-fast`) and to any service's own settings profiles.
  Prose says "install profile"; tools take `--install-profile`, beside
  `--cargo-profile` and `--target`, never a bare `--profile`.
- **The profile gate** is new, workspace-wide work. Today's closure and
  layering gates check compd's own crates only; they do not protect other
  components. The profile gate:
  - refuses the declaration errors above;
  - refuses a `base` or `server` component that itself declares the
    `desktop` layer;
  - resolves the normal and build dependency closure of each `base` and
    `server` principal, for every feature set its profiles build, and
    refuses any `desktop`-layer workspace package in it, and any upstream
    GUI crate (smithay, egui, eframe, wgpu, winit, Bevy and the like) even
    though those carry no MixOS metadata;
  - for a Mix component, which has no Cargo closure, refuses a `desktop`
    layer in a `base` or `server` component; what it calls on the Bus is
    checked at review, as AGENTS.md §2.5 already says.

  The component index ([`/docs/components/`](../components.md)) shows each component's
  profiles.
- **Resolving, installing and activating are separate.**
  - **Resolve:** one tool answers "what is in `server`?" from the metadata,
    deterministically. Its output covers the packages to build, the Mix
    components, and each component's runtime artefacts: units by class
    (system or session), `sysusers` and `tmpfiles` fragments, and assets.
    Build scripts, images and installers never keep their own lists.
  - **Install:** places those files. Rust components go to an installer
    that gives binaries and units the record, rollback and drift check
    `tools/component_install.mix` gives Mix components (separate work that
    follows this decision); Mix components go to that existing installer.
  - **Activate:** which services a particular node runs, with what
    configuration, is that node's decision, not the profile's. Installing
    `server` makes a service available; it does not start every server on
    every node.

### The first assignment

| Component | Profiles |
|---|---|
| `mix-shell` (the `mix` binary), `noded` | `base` |
| `compd`, `settingsd`, `portald`, `editd`, `releasesd`, `busviewer`, `ced`, `dopus`, `prefs` | `desktop` |

`server` starts empty: the server programs join it as they move in (mail,
web, blobs and the rest). `releasesd` is a per-user session service and
`editd` serves the session's editors, so both are `desktop`. A headless
mode for either would be its own decision, with suitable supervision,
before it joins `server`.

## Why

- **The two sides share their foundation.** The Bus, noded, Mix, the config
  and props libraries and logging sit under both, and they change almost
  daily. A second repository could pin them as Git dependencies rather than
  wait for releases, but every foundation change would still mean a
  coordinated upgrade on the other side, and consolidating duplicated code
  across the boundary would mean moving it between repositories.
- **Changes cut across both.** The command registry's move to classes changed
  the registry, its gate and every application in one commit. Across two
  repositories that is coordinated pull requests that can drift apart.
- **The separation that matters can be enforced here.** Headless code
  never needing a GUI stack is a property of a dependency closure, and the
  profile gate checks exactly that for every `base` and `server` component.
  A repository boundary would enforce it less precisely: it says nothing
  about which features or which programs a node gets.
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

- **A second repository for server programs.** The strongest case for it:
  an independent lockfile, toolchain and release pipeline, so desktop
  dependency churn never touches a server build and a server fix never
  waits on the desktop. That isolation is real; it loses to the shared,
  fast-moving foundation and the consolidation still to do. A split would
  be the right call if the server side ever needed a different licence,
  private code, its own team or its own release cadence. None of these
  applies. Private deployment detail already lives outside this repository.
- **Cargo features or a second workspace in this repository.** Rejected. A
  second workspace means two lockfiles and two dependency graphs, which is
  what AGENTS.md §0 rules out. Features select code inside a crate, not
  which programs a system gets.
- **A top-level `server/` directory.** Rejected: the top level is
  classified by kind only (AGENTS.md §1). A web server is a service like
  any other; which systems it belongs on is data.

## Consequences

- One `Cargo.lock`: a dependency bump for the desktop also moves the
  servers' resolved versions, and a server fix ships from the same tree as
  that day's desktop work. Each component's tests and the gates catch a
  break; the alternative was version drift between repositories.
- Building a server set compiles only that set's closure (`cargo build -p`),
  so headless builds stay small. A server build host still checks out the
  whole tree, desktop vendor forks included.
- AGENTS.md §2.3 and §2.5 gain the `profiles` field, and §10's checklist
  asks for it.
