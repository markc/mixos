// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drag preview service and its single demand-driven calloop wakeup.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;
use smithay::reexports::calloop::{LoopHandle, RegistrationToken};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use dispatcher::state::RedrawReason;
use world::state::Loop;

pub use world::comp::clock::{Clock, ManualClock};

pub fn service(lp: &mut Loop) {
    let Some(drag) = lp.inner.comp.drag.as_mut() else { return };
    if drag.tick(lp.inner.comp.clock.now()) {
        lp.state.schedule_redraw(RedrawReason::WindowState);
    }
}

pub fn deadline(comp: &world::comp::CompState) -> Option<Instant> {
    comp.drag.as_ref()?.next_deadline(comp.clock.now())
}

/// Shared with testkit: no registered source while idle. A fired source drops
/// itself; a cancelled grab removes its source at the end of that dispatch.
#[derive(Default)]
pub struct Wakeup {
    armed: Option<(RegistrationToken, Instant, Rc<Cell<bool>>)>,
}

impl Wakeup {
    pub fn armed(&self) -> bool {
        self.armed.as_ref().is_some_and(|(_, _, fired)| !fired.get())
    }

    pub fn rearm<T: 'static>(&mut self, handle: &LoopHandle<'static, T>, deadline: Option<Instant>, fire: fn(&mut T)) {
        if self.armed.as_ref().is_some_and(|(_, at, fired)| !fired.get() && Some(*at) == deadline) {
            return;
        }
        if let Some((token, _, fired)) = self.armed.take() && !fired.get() {
            handle.remove(token);
        }
        let Some(deadline) = deadline else { return };
        let fired = Rc::new(Cell::new(false));
        let flag = Rc::clone(&fired);
        let result = handle.insert_source(Timer::from_deadline(deadline), move |_, _, data| {
            flag.set(true);
            fire(data);
            TimeoutAction::Drop
        });
        match result {
            Ok(token) => self.armed = Some((token, deadline, fired)),
            Err(error) => model::warn!("drag preview timer: {error}"),
        }
    }
}

/// The same write executor is used by the Bus adapter and renderer-free fixtures.
pub fn set(comp: &mut world::comp::CompState, path: &str, value: &serde_json::Value) -> comp_model::ControlReply {
    let (old, new) = match comp.tiling.set(path, value) {
        Ok(values) => values,
        Err(error) => return comp_model::ControlReply::Validation(error),
    };
    if old != new {
        comp.settings_changed("tiling", "props.set");
    }
    let fields = serde_json::Map::from_iter([("tiling".into(), serde_json::to_value(comp.tiling).expect("tiling config"))]);
    let persisted = model::environment::preference::base::save_fields(fields).is_ok();
    comp_model::ControlReply::Set { path: path.into(), old, new, persisted: Some(persisted) }
}
