---
title: noded — changelog
description: Release notes of the noded component, mirrored from services/noded/CHANGELOG.md.
generated: by docs/_build/gen-site.mix from services/noded/CHANGELOG.md; do not edit
---

# noded changes

> Mirrored from [`services/noded/CHANGELOG.md`](https://github.com/markc/mixos/blob/main/services/noded/CHANGELOG.md), where releases update it. The component's page is [noded](README.md). This page describes the current `main` branch of MixOS.

## 0.18.3

Add a structured rejection body to the different-channel `noded.register`
collision refusal: `{"schema": "noded.registration-rejection.v1",
"error_code": "NAME_TAKEN", "message": …}` alongside the unchanged `rc=10`
reply and `error` header. The body is additive only — ABP framing, return
codes, admission refusals and session transcript bytes are untouched — and
lets clients classify a collision without matching diagnostic text. The bus
client decodes it through a bounded typed envelope whose classification is
pinned by noded and bus tests.

## 0.18.2

Transplant the native broker, session authority and mesh routing to the MixOS
workspace. Preserve ABP verbs, admission domains and session transcript bytes.
Use strict MixOS configuration and owned shutdown signalling.
