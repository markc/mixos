# files

Shared file model. See [the manual](../../docs/files/README.md).

`chooser` owns the renderer-free `files.choose` request grammar, byte paths,
filters, choices, acceptance validation and provider lifecycle engine.
DOpus re-exports it at `dopus_core::chooser`; toolkit uses the same types for
its three chooser tiers. This code moved from dopus-core so these two owners
share one contract without a dependency from toolkit on the DOpus app.
