// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Vendored into MixOS (for libs/syntax) from microsoft/edit@826b4c0 crates/stdext/src/collections/mod.rs; see vendor/msedit/README.md.

mod string;
mod vec;

pub use string::{BString, BStringFormatter};
pub use vec::BVec;
