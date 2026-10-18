// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use smithay::desktop::PopupManager;

pub struct PopupState {
    // A Smithay utility to track the tree of popups (right-click menus, dropdowns) so they
    // can be dismissed when the user clicks outside of them.
    pub state: PopupManager,
}
