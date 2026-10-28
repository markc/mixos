<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-27

- Add `call_app_with_limit` for a caller-selected command deadline. Registration
  wait limits no longer shorten a delivered call; timeouts remain uncertain.

- Serve live debug queries and six logging levels in the Bus lane, including
  instance names and canonical discovery names. Record command receipt and
  result at debug without adding application actions.

- Add `call_app` for Handle and SupervisedClient: call an app, launch it once
  only when its service is absent, then retry within a registration wait limit.
  Typed errors distinguish transport uncertainty, launch failure, timeout and
  app refusal. Refusals retain their original reply, code and message.
- Add a scripted sink for testing effect workers without a broker.
