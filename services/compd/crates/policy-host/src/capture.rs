// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Agent-first frame capture: no input-authority or active-VT gate.

use comp_model::capture::CaptureFrameSpec;
use comp_model::reply::ControlReply;
use dispatcher::state::state::RedrawReason;
use dispatcher::wire::trait_::wire_trait::WireTrait;
use serde_json::json;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use std::time::{Duration, Instant};
use world::state::Loop;

pub fn start(
    lp: &mut Loop,
    spec: CaptureFrameSpec,
    admitted: Instant,
    answer: impl FnOnce(ControlReply) + 'static,
) {
    let window_output = if let Some(target) = spec.window {
        let id = match lp
            .inner
            .comp
            .registry
            .resolve_window_target(target.id, Some(target.generation))
        {
            Ok(record) => record.id(),
            Err(error) => {
                answer(ControlReply::WindowTarget {
                    id: target.id,
                    error,
                });
                return;
            }
        };
        let Some(window) = crate::control::window_of(lp, id) else {
            answer(ControlReply::WindowTarget {
                id: target.id,
                error: surfaces::WindowTargetError::NotMapped,
            });
            return;
        };
        // Resolve in its owning world, even if that world is currently parked.
        lp.inner.all_world_spaces().iter().find_map(|space| {
            space.state.element_location(&window)?;
            space.state.outputs_for_element(&window).first().cloned()
        })
    } else {
        None
    };
    // The window's output, else the named one, else the one under the
    // pointer (as `comp.region.select` chooses), else the first.
    let outputs: Vec<_> = lp.inner.space_state().state.outputs().cloned().collect();
    let names: Vec<String> = outputs.iter().map(|o| o.name()).collect();
    let pointer = lp.inner.cursor_output.clone().and_then(|key| {
        outputs
            .iter()
            .position(|o| world::state::state::output_key(o) == key)
    });
    let window_at = window_output
        .as_ref()
        .and_then(|w| outputs.iter().position(|o| o == w));
    let output = pick_output(
        &names,
        window_output.is_some(),
        window_at,
        spec.output.as_deref(),
        pointer,
    )
    .map(|at| outputs[at].clone());
    let Some(output) = output else {
        answer(ControlReply::Refused {
            error: "unknown_output",
            detail: json!({"output": spec.output}),
        });
        return;
    };
    if spec.region.is_some() && output.current_transform() != smithay::utils::Transform::Normal {
        answer(ControlReply::refused(
            "unsupported_transform",
            json!({"output":output.name(),"message":"region capture requires an unrotated output"}),
        ));
        return;
    }
    if spec
        .output_generation
        .is_some_and(|expected| expected != lp.inner.comp.output_generation(&output.name()))
    {
        answer(ControlReply::refused(
            "output_changed",
            json!({"output":output.name()}),
        ));
        return;
    }
    let id = screencopy::file::enqueue(&output, spec, move |result| {
        answer(match result {
            Ok(body) => ControlReply::Body(body),
            Err(reply) => reply,
        });
    });
    // A stopped/dark renderer, a removed output or a failed render must always
    // answer. Timer and render completion race only on this compositor thread;
    // taking the request makes the second completion a no-op.
    if lp
        .loop_handle
        .insert_source(
            Timer::from_deadline(admitted + Duration::from_secs(3)),
            move |_, _, _| {
                screencopy::file::fail(id, "capture deadline passed");
                TimeoutAction::Drop
            },
        )
        .is_err()
    {
        screencopy::file::fail(id, "capture deadline timer unavailable");
        return;
    }
    lp.state.redraw.request_for(RedrawReason::Capture);
    lp.state.redraw.wake();
}

/// Which of `names` a frame comes from: a window capture's own output
/// (`window_at`; none if the window is on no output), else the `named` one,
/// else the output under the pointer, else the first.
fn pick_output(
    names: &[String],
    window: bool,
    window_at: Option<usize>,
    named: Option<&str>,
    pointer: Option<usize>,
) -> Option<usize> {
    if window {
        return window_at;
    }
    match named {
        Some(name) => names.iter().position(|n| n == name),
        None => pointer
            .filter(|at| *at < names.len())
            .or((!names.is_empty()).then_some(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::pick_output;

    #[test]
    fn an_unnamed_frame_comes_from_the_pointer_output() {
        let names = ["DP-1".to_owned(), "HDMI-A-1".to_owned()];
        assert_eq!(pick_output(&names, false, None, None, Some(1)), Some(1));
        assert_eq!(pick_output(&names, false, None, None, None), Some(0));
        assert_eq!(
            pick_output(&names, false, None, Some("DP-1"), Some(1)),
            Some(0)
        );
        assert_eq!(pick_output(&names, false, None, Some("VGA"), Some(1)), None);
        assert_eq!(
            pick_output(&names, true, Some(1), Some("DP-1"), Some(0)),
            Some(1)
        );
        assert_eq!(pick_output(&names, true, None, None, Some(0)), None);
        assert_eq!(pick_output(&[], false, None, None, None), None);
    }
}
