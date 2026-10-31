// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! §2.7 `sound.*`: PipeWire's devices and levels are live (its own state);
//! only the amplify flags are settings.
use super::{DefaultValue as D, Domain::*, KeyDef as K, Kind::*};

const PAGE: &str = "system/sound";

const fn live(path: &'static str, what: &'static str) -> K {
    K::live(path, what, "audio").page(PAGE)
}

pub(super) const KEYS: &[K] = &[
    live("sound.output.device", "PipeWire node name"),
    live("sound.input.device", "PipeWire node name"),
    live("sound.output.volume", "number"),
    live("sound.output.mute", "bool"),
    live("sound.output.balance", "number"),
    live("sound.input.volume", "number"),
    live("sound.input.mute", "bool"),
    K::new("sound.output.amplify", Bool, D::Bool(true)).on(PAGE, &[System]),
    K::new("sound.input.amplify", Bool, D::Bool(false)).on(PAGE, &[System]),
];
