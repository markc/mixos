# clipd

Clipboard Bus service written in Mix. Run with `mix --serve scripts/clipd.mix
--name clipd`. The [manual](../../docs/clipd/README.md) describes the API,
configuration, external tools and current Wayland limitations.

From the checkout root:

```
mix services/clipd/tests/clipboard_test.mix
mix services/clipd/tests/clipd_test.mix
```
