---
title: Decision — service property storage belongs in props
description: Keep service domain records and native Bus routing in props, with an opt-in logging bridge and unchanged persisted contracts.
---

# Decision — service property storage belongs in props

Status: accepted 2026-10-18.

## The decision

`libs/props` owns the shared service property substrate: namespace schemas,
capabilities, hooks, lifecycle, audit, storage, mutation routing and event
dispatch. Its empty default feature set remains pure types and revisions;
`store`, `sqlite` and `router` add the respective facilities.

`libs/logging` owns the reserved `log` namespace and live-filter watcher,
behind its opt-in `props` feature. Services supply domain-table mappings
and use their existing native Bus connection for subscription grants and
publication. No new crate, transport or broker is introduced.

The [condensed contract](../spec/props/README.md) preserves capability
strings, verb envelopes, errors, topic suffixes, audit canonicalisation and
the five `__props_*` table names and DDL. Persisted databases require no
migration. `noded.props.subscribe_grant`, already served by the broker,
is registered as an intent because it installs a subscription for another
peer. Concrete service verbs are registered when their owner adopts them.

## Why

Service domain data and the logging bridge need the same schema, storage,
audit and native routing contracts. Keeping those contracts beside the
existing property read surface avoids duplicate stores and projection
logic; replay and live delivery use the same functions. Optional features
keep database and runtime dependencies out of pure readers. This is the
deliberate shared API boundary for logging and the forthcoming mail service
port, with other domain services able to use the same substrate.

## Authority and secrets

`settingsd` continues to own desktop and session settings. This substrate
owns each service's durable domain records: a mail server's mailbox accounts
are distinct from the email application's `email.accounts` profile group.
It creates no second authority for desktop preferences.

The [email settings decision](2026-10-16-email-accounts-in-settings.md)
continues to govern app passwords. Fields declared secret are redacted on
ordinary reads, public describe and passive events; audit keys never enter
the Bus. An explicit secrets-read capability remains part of the existing
privileged service contract. No application password publication is added.
