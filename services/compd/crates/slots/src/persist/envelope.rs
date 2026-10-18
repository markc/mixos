// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// The versioned on-disk envelope: a small JSON wrapper around a storage's
// persisted DATA, carrying the framework version + the payload schema version so
// both can evolve. `wrap`/`unwrap` convert between file bytes and (version, data).
pub mod base;
