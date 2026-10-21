// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::state::DispatchWire;
use protocols::popup::state::PopupState;
use smithay::desktop::PopupManager;

pub fn new<I: DispatchWire>() -> PopupState {
    let popup = PopupManager::default();

    PopupState { state: popup }
}
