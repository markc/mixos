---
title: Decision — site URL layout
description: Every component's documentation lives at the top of mixos.dev under its own name, in HTML and Markdown at fixed addresses. Everything that is not a component lives under one /docs/ section, so the only names a component can never take are docs and v.
---

# Decision — site URL layout

Status: proposed 2026-10-16; amended 2026-10-16 after review by two
independent reviewers.

## The question

mixos.dev serves two readers from the same files. A person in a browser
gets a static HTML page, and nothing needs JavaScript to render. An agent
or script on any machine fetches the same page as Markdown and starts from
`/llms.txt`. MixOS will grow to hundreds of apps, services, libraries and
tools over the next few years. How should the URL space be organised, so
that an address, once published, never has to change?

## The decision

### Two namespaces

- **Each component's documentation is at the top level, under the
  component's name.** Examples: `/mix/`, `/term/`, `/compd/`, `/hostd/`,
  `/noded/`.
- **Everything that is not a component lives under `/docs/`.** That covers
  guides such as `/docs/try/` and `/docs/drive/`, plus
  `/docs/decisions/`, `/docs/spec/`, `/docs/dev/`, `/docs/releases/`, and
  the generated indexes below. The site's global navigation has a Docs
  entry that leads there.

### Component names

- A component's name is the `component` declared by its principal package
  in `[package.metadata.mixos]`, or by its `component.mx`. Component names
  are already plain, unique across the workspace and never reused (see
  [source layout](2026-10-04-source-layout.md)), so an address can be
  built from the name alone.
- A component name starts with a lowercase letter and contains only
  lowercase letters, digits and hyphens.
- **Two names are reserved: `docs` and `v`.** The component metadata gate
  refuses them, so a forbidden name is caught when the component is
  created, not when the site is built.
- Site machinery uses names no component can take. Generator inputs and
  assets live in `_build/` and `_web/` (renamed from `build/` and `web/`),
  and root files (`index.html`, `404.html`, `llms.txt`, `llms-full.txt`,
  `sitemap.xml`, `robots.txt`, `CNAME`, the favicons) all contain a dot or
  start with an underscore.
- **Mix's names.** The `mix` component is the language library, so `/mix/`
  is the Mix language manual and the same pages `mix man TOPIC` reads. The
  `mix` binary's component is `system` (`/system/`), and the shell is
  `mix-shell` (`/mix-shell/`). Each links to the others.

### Where pages come from, and their addresses

The path of a source file relative to the site root (the repository's
`docs/` directory) determines its address. The table gives every
mapping:

| Source | HTML | Markdown |
|---|---|---|
| `docs/<name>/README.md` | `/<name>/` | `/<name>/README.md` |
| `docs/<name>/<page>.md` | `/<name>/<page>/` | `/<name>/<page>.md` |
| `docs/docs/<page>.md` | `/docs/<page>/` | `/docs/<page>.md` |
| `<component dir>/CHANGELOG.md` (mirrored) | `/<name>/changelog/` | `/<name>/changelog.md` |
| Generated index pages | `/docs/<index>/` | `/docs/<index>.md` |

- **The Markdown is published at its source path.** Relative links inside
  it therefore resolve correctly under ordinary URL resolution. An agent
  builds a component's Markdown address as `/<name>/README.md`. The
  generator checks every relative link in both formats.
- **Hand-written component documentation lives in `docs/<name>/`.** That
  is its one canonical copy. The component's own directory keeps a short
  `README.md` that points to the page, so someone browsing the source
  finds it.
- **`CHANGELOG.md` stays beside the code**, where releases update it. Only
  the principal package's changelog is published. The published copy is
  generated and never edited. Relative links in it are rewritten to the
  source on GitHub. A component with no changelog has no changelog page.
  `changelog` is therefore a reserved page name inside every component.
- **The doubled `docs/docs/`** is accepted. It keeps the rule that the
  source path determines the address, without a rename table.
- **Generated pages have a Markdown twin** like every other page.
- **HTML is the canonical address.** Each HTML page names its Markdown as
  `<link rel="alternate" type="text/markdown">`. `sitemap.xml` lists HTML
  only.

### Navigation from metadata

- **The URL never encodes kind, layer or install profile.** These change
  over a component's life: a library is promoted, or a desktop service
  becomes useful headless.
- `/docs/desktop/` and `/docs/server/` are generated index pages listing
  components by their declared install `profiles`. A component in both is
  listed in both, with one page. Components in the `base` profile are
  listed under a reader-facing heading such as "Core components". The word
  `base` stays in the installation docs.
- `/docs/components/` is the one complete, generated component index,
  giving each component's name, kind, profiles, source path and page. It
  replaces the hand-run `docs/dev/components.md`.
- Until `profiles` is declared everywhere, the indexes list undeclared
  components as unclassified. They never guess from `layer`.

### Agents

- `/llms.txt` lists every section, with absolute Markdown URLs: one line
  per component (with its kind and profiles) and one per page under
  `/docs/`. Every published document can be reached from it.
  `llms-full.txt` remains an optional single file.
- Every page states that it describes the current `main` branch.

### Every component has a page

- The principal package of every component (app, service, cli or lib) and
  every Mix component must have `docs/<name>/README.md`. Private crates and
  `tool` and `test` packages are excluded.
- **A component without a page fails the site gate.** While existing
  components are being documented, the ones still missing are named in an
  exception list in `site.conf.mix`. That list may only shrink, and a new
  component cannot be added to it.

### Addresses are permanent

From the day this decision is accepted:

- A profile change or a source move never changes an address.
- An address that goes away keeps working in both formats: an entry in
  `site.conf.mix`'s `redirects` for browsers, plus a short Markdown page at
  the old `.md` address that names the new one, because an agent fetching
  Markdown does not follow an HTML redirect.
- A split leaves a page that links to its successors. A retired component
  keeps a retirement page, which stays listed in `/llms.txt`, and its name
  is never reused.

### Versions

`/v/` is reserved for versioned copies of the site. How they work is
decided before the first stable release.

## Why

- **One namespace for components, one for everything else.** An address
  either names a component or starts with `/docs/`. Only two names are off
  limits for components.
- **Names are the most stable thing a component has.** The source-layout
  decision rejected splitting the source tree by desktop, server or core,
  because a component could plausibly sit in either of two places and
  moves between them. Putting that axis into URLs would have recreated the
  problem where a move is most costly.
- **It matches what already works.** `/mix/` and the existing component
  pages (`/hostd/`, `/noded/`, `/prefs/`, `/backdrop/`, `/screenshot/`,
  `/dopus/`) are already at the top level, and `mix man` already fetches
  `/mix/<topic>.md` and `/mix/README.md`.
- **One canonical copy.** Keeping hand-written docs in `docs/` rather than
  beside the code relaxes the source-layout principle that a component is
  self-contained. That is deliberate: documentation for readers has one
  home, one navigation and one gate. Code, tests and changelogs stay with
  the component.

## Rejected

- **Mirror repository paths** (`/apps/term/`, `/libs/bus/`). The address
  is predictable, but it encodes kind, and it moves when a component moves
  in the source tree.
- **Sections by install profile** (`/desktop/term/`, `/server/maild/`,
  `/base/noded/`). Profiles are deployment data that change, a component
  can be in two, and `server` is empty today. A profile change would move
  a public URL.
- **A component prefix** (`/c/term/` or `/components/term/`). It protects
  the component namespace, but `/docs/` does the same job from the other
  side, with shorter, more readable component URLs.
- **Docs beside the code, mirrored into the site.** That makes two copies,
  needs a freshness gate and a mapping table, and splits navigation.
- **A `/<name>.md` alias for each component page.** Relative links in it
  would resolve against the wrong directory.
- **Mapping `/docs/` to a differently named source directory** (for
  example `docs/guides/`). It avoids `docs/docs/`, but it adds a permanent
  rename table.

## Consequences

- Nobody depends on the current URLs, so this one move keeps no old
  address. The permanence rules apply from acceptance.
- The root pages that are not components move under `docs/docs/`: `try`,
  `drive`, `edit`, `releases`, and `bridge` (until `bridged` owns it), plus
  `decisions/`, `spec/` and `dev/`.
- Root pages that describe a component become its page
  (`docs/hostd.md` becomes `docs/hostd/README.md`, and so on).
- The component READMEs beside the code move into `docs/<name>/README.md`,
  leaving a pointer behind. The CHANGELOGs stay where they are.
- `docs/build/` and `docs/web/` become `docs/_build/` and `docs/_web/`.
- The component metadata gate gains the name grammar and the reserved
  names.
- `gen-site` gains the output-collision check (every HTML, Markdown, asset
  and generated output), the changelog mirror, the generated indexes and
  their Markdown twins, the alternate links, the missing-page gate with its
  exception list, and a grouped `llms.txt`.
- Every link to a moved page is updated, including in AGENTS.md and the
  decision records.
