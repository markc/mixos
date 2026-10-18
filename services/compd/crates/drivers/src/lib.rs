// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Driver channels between the kernel (DRM topology, session, render loop)
//! and the rim (input and policy): audio, lid, logind power actions, output
//! mode requests and resume/vblank state, all reached by storage token.

#[macro_use]
extern crate model;

pub mod audio;
pub mod lid;
pub mod logind;
pub mod output;
pub mod resume;
pub mod world;
