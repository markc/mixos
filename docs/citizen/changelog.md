---
title: citizen — changelog
description: Release notes of the citizen component, mirrored from libs/citizen/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from libs/citizen/CHANGELOG.md; do not edit
---

> Mirrored from [`libs/citizen/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/libs/citizen/CHANGELOG.md), where releases update it. The component's page is [citizen](README.md). This page describes the current `main` branch of MixOS.

<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-27

- Serve live debug queries and six logging levels in the Bus lane, including
  instance names and canonical discovery names. Record command receipt and
  result at debug without adding application actions.

- Add `call_app` for Handle and SupervisedClient: call an app, launch it once
  only when its service is absent, then retry within a registration wait limit.
  Typed errors distinguish transport uncertainty, launch failure, timeout and
  app refusal. Refusals retain their original reply, code and message.
- Add a scripted sink for testing effect workers without a broker.
