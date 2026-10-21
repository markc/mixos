// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
pub use crate::shortcut;

pub mod key {
    pub use crate::library::input::keyboard::enum_::*;
}
pub mod handler {
    pub use crate::library::input::keyboard::action::ShortcutHandler;
}

#[macro_use]
pub mod action;
pub mod combo;
pub mod enum_;
pub mod format;
