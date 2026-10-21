// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use dispatcher::state::Dispatch;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use world::state::Loop;

pub fn create<'a>() -> Result<(EventLoop<'a, Loop>, Display<Dispatch>), Box<dyn std::error::Error>>
{
    let event_loop = EventLoop::try_new()?;
    let display = Display::new()?;

    Ok((event_loop, display))
}
