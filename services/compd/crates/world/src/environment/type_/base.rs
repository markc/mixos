// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
#[derive(Clone, Debug)]
pub struct Environment {
    pub gpu: String,
    pub desktop_name: String,
}

/// Thin adapter over the central environment config
/// (`model::environment::config`). Kept so existing consumers
/// that hold an `Environment { gpu, desktop_name }` (e.g. `State::new`) need no
/// signature change. The values come from the single `COMPOSITOR_ENVIRONMENT`
/// JSON, which `init()` has already parsed by the time this is called.
pub fn get() -> Environment {
    let env = model::environment::config::base::get();
    let desktop_name = String::from(env.desktop_name.trim());
    let gpu = String::from(env.render_node.trim());

    Environment { gpu, desktop_name }
}
