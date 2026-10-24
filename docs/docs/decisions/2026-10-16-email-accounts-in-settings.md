---
title: Decision — email accounts live in settingsd, passwords in the app
description: The email app's accounts and mail options are the session profile's email group, edited in Preferences; their passwords stay in the app's private 0600 file and reach it only through a write-only command.
---

# Decision — email accounts live in settingsd, passwords in the app

Status: accepted 2026-10-16. Applies to the [email app](../../email/README.md)
and the [settings contract](../spec/settings/README.md) (`email.*`, contract
0.8.0).

## The decision

- **Non-secret account settings live in settingsd.**
  - The session profile's `email` group holds every account, in tree
    order: `id`, `name`, `url`, `email`, `ca_path`, `enabled` and
    `signature`. It also holds `default_account` and the mail options
    (`page_size`, `mark_read_on_open`, `confirm_delete`).
  - `email.accounts` is replaced whole by one fenced `settings.apply`.
    A change that removes the default account resets
    `email.default_account` in the same batch, so settingsd never holds
    half a change.
  - The email app follows the group through the session-profile summary
    and reconciles its connections live.
  - Ids are minted once (`acct-` and 12 hex digits) and never reused for
    another account.
- **Passwords never enter settingsd.**
  - settingsd publishes retained snapshots on the Bus, which every
    client can read. The `email` group therefore has no password field,
    and every account record refuses unknown keys, so an apply carrying
    `password` is refused.
  - Each password lives in the email app's own file,
    `<config>/secrets.conf.mix` (`{schema_version: 1, passwords: {id: …}}`).
    The directory is 0700 and the file 0600, it is replaced atomically,
    and the poison-pill law applies: a malformed file is never
    overwritten.
  - Only the email app writes that file, and a password leaves it only
    toward the account's own server.
- **A password is reachable only through a write-only command.**
  - `email.account.password.set {id, password}` stores a password and
    reconnects the account.
  - `email.account.set` may carry `password` too. It is held only once
    settingsd has accepted the account.
  - No command, topic, log line or diagnostic returns a password.
    `email.accounts` and `email.info` report `has_password: bool`.
- **Preferences is the configuration surface.**
  - Accounts are added, edited, removed, reordered and made the default
    in Preferences' Email section, which writes `email.*` through settingsd. It
    sets passwords through `email.account.password.set`, starting Email
    first if it is not running.
  - The email app has no account form of its own. With no account, or
    with an account that failed or has no password, it offers Open Email
    Settings.
- **The app keeps a cache of the last accounts it read** in its own
  config (schema 2, which holds no secret). When settingsd is absent, the
  cached accounts open read-only and the status line says so. Account
  writes are refused with `UNAVAILABLE` until settingsd is back.
- **The single account of an older install migrates once, losing
  nothing.**
  - The account comes from the schema-1 `config.conf.mix` and is given an
    id derived from its URL and address.
  - Its password is written to the secrets file first. An inline CA
    bundle goes to a 0600 file beside it, which the account then names by
    `ca_path`.
  - The account is then applied to settingsd, and made the default when
    there is none. An account settingsd already holds with the same URL
    and address is adopted instead.
  - The schema-1 file is kept, and is written back unchanged, until
    settingsd has the account and its password is durable under that
    account's id. Only then is it rewritten as schema 2.

## Why

- **The Bus must not carry secrets.** The session profile is retained
  and broadcast so that any app can follow it. A password there would be
  readable by every Bus client and would appear in every snapshot and
  backup of the profile.
- **One place to configure.** Terminal preferences already live in
  settingsd and are edited in Preferences. Mail accounts are the same kind of
  thing, and a second account form inside the app would drift from it.
- **Automation needs the accounts as data.** An agent can list, add or
  disable an account through `settings.apply` or the email commands. It
  never handles a secret, unless it is the one supplying the password.

## Consequences

- **A settings read never deletes a password.** Only an explicit action
  through Email does: `email.account.remove`, or
  `email.account.password.set` with an empty password. An account removed
  elsewhere (in Preferences, by a reset) drops the app's client and view, and
  leaves its entry in the 0600 secrets file, unused. That is accepted:
  losing a password to a misread would be worse.
- **An empty account list is not a removal of them all.** settingsd omits
  the group at its defaults, so an older settingsd, a reset and a profile
  without the group cannot be told apart. While accounts are shown, such
  a read keeps them (views, connections, passwords) with a status note,
  until a read lists accounts again or one is removed through Email.
- **Migration steps land in order, each durably.** First the password is
  saved to the secrets file. Then the account is applied to settingsd.
  Then, if settingsd held it under another id, the password is saved
  under that id. Only then is the schema-1 file rewritten. A failure at
  any step, including a poison-pill refusal to write the secrets file,
  leaves the schema-1 file untouched, and the next launch runs the
  migration again.
- A conflicting change (two writers at once) is refused by the fence.
  The email app reads again, retries a few times, and then reports
  `CONFLICT`.
- MixOS has no keyring yet. The secrets file has the same trust as the
  app's other private files. A keyring, when it lands, replaces only that
  file; the settings group and the commands stay as they are.
