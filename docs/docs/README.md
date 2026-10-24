---
title: Docs
description: Guides, indexes, decision records and specifications for MixOS; every component's own documentation is at mixos.dev/<component>/.
---

# Docs

Everything on mixos.dev that is not one component's documentation lives
here. A component's page is at `/<component>/`, for example
[`/mix/`](../mix/README.md) for the Mix language or
[`/term/`](../term/README.md) for the terminal. Every page is also published
as Markdown at its source path, and [`/llms.txt`](https://mixos.dev/llms.txt)
lists them all. The [site URL layout](decisions/2026-10-16-site-url-layout.md)
decision fixes these addresses.

## Guides

- [Try it beside your desktop](try.md): run MixOS as a small signed image on
  its own virtual terminal, next to the desktop you already use.
- [Driving apps over the Bus](drive.md): how an agent drives a native MixOS
  app through Bus commands.

## Indexes

- [Components](components.md): every component, with its kind, install
  profiles, source and page.
- [Desktop](desktop.md) and [Server](server.md): the components each install
  profile includes.

## Decisions

Public decision records are the authority for the MixOS tree. The
foundational ones:

- [Source layout and naming](decisions/2026-10-04-source-layout.md)
- [egui applications, engine first](decisions/2026-10-09-egui-applications.md)
- [Commands take parameters and return results](decisions/2026-10-20-commands-take-parameters.md)
- [Bus commands and app actions](decisions/2026-10-24-bus-commands-and-actions.md)
- [One repository, install profiles](decisions/2026-10-12-one-repository-install-profiles.md)
- [One system binary](decisions/2026-10-12-one-system-binary.md)
- [Site URL layout](decisions/2026-10-16-site-url-layout.md)

Every decision record is listed in [`/llms.txt`](https://mixos.dev/llms.txt)
and in the [repository](https://github.com/markc/mixos/tree/main/docs/docs/decisions).

## Specifications

- [Bus command registry](https://github.com/markc/mixos/blob/main/docs/docs/spec/bus/commands.conf.mix)
- [Bridge](spec/bridge/README.md)
- [Portal](spec/portal/README.md)
- [Releases](spec/releases/README.md)
- [Settings](spec/settings/README.md)
- [Mix programs and documents](spec/mix/2026-10-08-programs-and-documents.md)
- [Handler closure capture](spec/handler-closure-capture.md)
