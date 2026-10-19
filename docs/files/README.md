---
title: files
description: Rooted reads and the files-as-truth markdown corpus model.
---

# files

Shared filesystem and markdown corpus logic for webd and the files service.
`rooted_read::ReadRoot` pins an operator-provisioned root descriptor and
opens regular files with Linux `openat2`, refusing symlinks, escapes and
unsupported kernels. There is no path-based fallback.

The corpus model includes surgical frontmatter writes, the canonical Bus
header reader, BLAKE3 hashes, UUIDv7 identity, link extraction, reconcile
diffs and generic place-scoped filesystem operations. `atomic` preserves
its streamed verification, mode and no-replace contracts; these differ
from config's descriptor-relative configuration replacement helpers.

Enable `sqlite` for index persistence, ingestion and reconciliation.
Test with `cargo test --locked -p files --features sqlite` and
`cargo test --locked -p files --no-default-features`.

Source: [libs/files](https://github.com/markc/mixos/tree/main/libs/files).

Promoted from Dopus's private crate when webd became the second external
owner. Both consumers use the same API and tests; there is no second
filesystem implementation.
