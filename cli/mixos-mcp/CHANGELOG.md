# Changelog

## Unreleased

- Replace the unserved application action/control tools with `app_commands`
  and `app_execute`, using the application's registry Bus verbs. Forward optional
  command parameters unchanged and preserve application error messages.
- Error-text pass-through without the old `application rc=` prefix also applies
  to `app_describe`, `desktop_windows` and `desktop_window_wait` through their
  shared native helper.
- The indexd Bus-mode call timeout is now 120 seconds by default, previously
  the Bus library's 60 seconds. SSH bridge requests carry the configured timeout.
- Retry bridge failures once before delivery for every verb; after delivery,
  retry only idempotent verbs. Feedback and timeouts report an unknown outcome.
