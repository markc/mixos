# settingsd contract changes

## Unreleased

`settings.get` 0.7.0 includes `ui` in summary reads: text scale, density and
reduced motion on the same revision as appearance. Store format unchanged.

## 0.1.0

Before initial release: sealed profile format 4 re-derives the Workshop app
context and drops retired `apps.busviewer` overrides with a diagnostic after
seal verification, preserving unrelated overrides and history.

Serve describe/get/validate/apply/reset/status through native ABP; durable
changed/no-op receipts, target/revision fencing, exclusive writer, recoverable
replacement and complete scoped retained publication. Package the service unit.
Before initial release: add explicit idempotent first-run session seed, preserving
established missing-primary failure, and session wants/upholds wiring without
making GUI startup wait for authority readiness. Authority commands/schema unchanged.
First creation requires installer-only --allow-create; the automatic unit uses
plain seed and cannot recreate a wholly lost profile directory or mount.

Before initial release: pin package source and verify stored content digest;
bind storage to the held directory/lock; fence validation; add bounded pending
publication retries and preserve missing-store failure visibility.
Fence effective compiler interpretation; distinguish intact unsupported/I/O
failures from corruption; keep pending publication backoff capped at 30 seconds
until success. Inline capacity refusal has a distinct snapshot_too_large status.
