// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Client/surface lookup adapted from Smithay 0.7 wlcs_anvil (MIT),
// Copyright (c) 2017 Victor Berger, Victoria Brekenfeld and contributors.
// Reference and licence: vendor/smithay/wlcs_anvil, vendor/smithay/LICENSE.txt.
//! Test-only wlcs adapter. The inherited socket is private to the owned child;
//! it is never a listening control service or a replacement for the Bus.

use comp_model::request::{InputOp, PointerMoveTarget, PressAction};
use comp_model::snapshot::{PortSnapshot, ReadScopes};
use dispatcher::wire::trait_::surface_event::SurfaceHandle;
use dispatcher::wire::trait_::wire_trait::WireTrait;
use protocols::wayland::connection::record::WaylandClientSession;
use serde_json::{Value, json};
use smithay::wayland::seat::WaylandFocus;
use smithay::{
    reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic},
        wayland_server::{Client, Resource},
    },
    wayland::compositor::CompositorClientState,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::net::{UnixDatagram, UnixStream},
    rc::Rc,
    sync::Arc,
};
use surfaces::SeatKind;
use world::state::Loop;
use world::state::CoordinateTrait;

type Pending = Rc<RefCell<Vec<(Value, Option<UnixStream>)>>>;
pub struct Adapter {
    socket: Rc<UnixDatagram>,
    pending: Pending,
    clients: HashMap<i32, Client>,
    touch_slots: HashSet<u32>,
}

pub fn install(
    state: &mut Loop,
    event_loop: &EventLoop<Loop>,
) -> Result<Option<Adapter>, Box<dyn std::error::Error>> {
    let Some(fd) = std::env::var_os("COMPD_WLCS_FD") else {
        return Ok(None);
    };
    let fd: i32 = fd.to_str().ok_or("wlcs fd not UTF-8")?.parse()?;
    // SAFETY: the test parent explicitly transfers ownership across exec.
    let socket = Rc::new(unsafe { UnixDatagram::from_raw_fd(fd) });
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    socket.set_nonblocking(true)?;
    let pending = Pending::default();
    let queue = pending.clone();
    event_loop.handle().insert_source(
        Generic::new(socket.clone(), Interest::READ, Mode::Level),
        move |_, socket, _| {
            let mut bytes = [0u8; 4096];
            // cmsghdr must be aligned. This buffer is large enough for one fd.
            let mut control = [0usize; 8];
            let mut iov = libc::iovec {
                iov_base: bytes.as_mut_ptr().cast(),
                iov_len: bytes.len(),
            };
            let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = std::mem::size_of_val(&control);
            let n = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            if n == 0 {
                return Ok(PostAction::Remove);
            }
            let mut stream = None;
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&msg);
                if !header.is_null()
                    && (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    stream = Some(UnixStream::from_raw_fd(
                        *libc::CMSG_DATA(header).cast::<i32>(),
                    ));
                }
            }
            let request = serde_json::from_slice(&bytes[..n as usize])?;
            queue.borrow_mut().push((request, stream));
            Ok(PostAction::Continue)
        },
    )?;
    // wlcs injects on the agent seat only. No normal backend changes capability.
    state
        .state
        .seat
        .agent
        .as_mut()
        .ok_or("missing agent seat")?
        .add_touch();
    state.inner.comp.injection.agent_pointer = Some((0.0, 0.0));
    socket.send(b"ready")?;
    Ok(Some(Adapter {
        socket,
        pending,
        clients: HashMap::new(),
        touch_slots: HashSet::new(),
    }))
}

impl Adapter {
    pub fn service(&mut self, state: &mut Loop) {
        let requests = std::mem::take(&mut *self.pending.borrow_mut());
        for (request, stream) in requests {
            let result = self.apply(state, &request, stream);
            // Completion is after the native path ran, before the caller returns.
            let _ = state.state.output.display_handle.flush_clients();
            let _ = self
                .socket
                .send(serde_json::to_string(&result).unwrap().as_bytes());
        }
    }

    fn apply(&mut self, state: &mut Loop, r: &Value, stream: Option<UnixStream>) -> Value {
        let number = |key: &str| r[key].as_f64().unwrap_or_default();
        match r["op"].as_str().unwrap_or_default() {
            "client" => {
                let data = WaylandClientSession {
                    compositor_state: CompositorClientState::default(),
                    proprietary: false,
                };
                let client = state
                    .state
                    .output
                    .display_handle
                    .insert_client(stream.expect("wlcs client fd"), Arc::new(data))
                    .expect("insert wlcs client");
                self.clients
                    .insert(r["client"].as_i64().unwrap() as i32, client);
            }
            "place" => {
                let client = self.clients.get(&(number("client") as i32));
                let surface = state
                    .inner
                    .host_space()
                    .state
                    .elements()
                    .filter_map(|w| w.wl_surface())
                    .find(|s| {
                        s.id().protocol_id() == number("surface") as u32
                            && state
                                .state
                                .output
                                .display_handle
                                .get_client(s.id())
                                .ok()
                                .as_ref()
                                == client
                    })
                    .map(|s| s.into_owned());
                if let Some(surface) = surface
                    && let Some(id) = state
                        .inner
                        .comp
                        .registry
                        .id_for_handle(&SurfaceHandle::resolve(&surface))
                    && let Some(record) = state.inner.comp.registry.get(id)
                {
                    let generation = record.generation();
                    let origin =
                        policy_host::input::wlcs_world_point(state, number("x"), number("y"));
                    policy_host::control::wlcs_place_surface(state, id, origin.to_i32_round());
                    // Read the same window row exposed on the Bus. Check both
                    // the native world origin and its screen projection.
                    let snapshot = policy_host::project::project(
                        state,
                        policy_host::project::Identity {
                            service: "compd".into(),
                            version: env!("CARGO_PKG_VERSION").into(),
                            backend: "headless",
                            engine: "wlcs",
                            instance: "wlcs".into(),
                            binding_profile: "nested",
                            port: PortSnapshot {
                                level: "test",
                                event_seq: 0,
                                lost_count: 0,
                                queue_depth: 0,
                                reply_timeouts: 0,
                                publish_timeouts: 0,
                                slug_collisions: 0,
                                broker: "test",
                            },
                        },
                        &ReadScopes::Paths(vec!["windows".into()]),
                    );
                    let row = snapshot
                        .windows
                        .values()
                        .find(|row| row.id == id.0 && row.generation == generation);
                    let Some(row) = row else {
                        return json!({"error": "placed_window_row_missing"});
                    };
                    let transform: world::camera::transform::translate::transform::Transform = (
                        smithay::utils::Point::<f64, smithay::utils::Logical>::from((
                            f64::from(row.x),
                            f64::from(row.y),
                        )),
                        state.size_ctx_all(),
                    )
                        .into();
                    let screen: smithay::utils::Point<f64, smithay::utils::Logical> =
                        transform.into();
                    if (screen.x - number("x")).abs() > 0.01
                        || (screen.y - number("y")).abs() > 0.01
                    {
                        return json!({"error": "surface_origin_mismatch", "requested": [number("x"), number("y")], "actual": [screen.x, screen.y]});
                    }
                    return json!({"ok": true, "surface_origin": [screen.x, screen.y]});
                }
                return json!({"error": "window_not_mapped"});
            }
            "absolute" | "relative" | "button" => {
                let op = match r["op"].as_str().unwrap() {
                    "absolute" => {
                        let point =
                            policy_host::input::wlcs_world_point(state, number("x"), number("y"));
                        // Absolute motion establishes a position even after
                        // native focus cleanup cleared the previous target.
                        let previous = *state
                            .inner
                            .comp
                            .injection
                            .agent_pointer
                            .get_or_insert((0.0, 0.0));
                        InputOp::PointerMove {
                            target: PointerMoveTarget::Relative {
                                dx: point.x - previous.0,
                                dy: point.y - previous.1,
                            },
                            corners: false,
                        }
                    }
                    "relative" => InputOp::PointerMove {
                        target: PointerMoveTarget::Relative {
                            dx: number("x"),
                            dy: number("y"),
                        },
                        corners: false,
                    },
                    _ => InputOp::PointerButton {
                        button: number("button") as u32,
                        action: if r["down"] == true {
                            PressAction::Press
                        } else {
                            PressAction::Release
                        },
                    },
                };
                let reply = policy_host::input::input(
                    state,
                    &InputOp::OnSeat {
                        seat: SeatKind::Agent,
                        op: Box::new(op),
                    },
                );
                return reply.wire_json();
            }
            "touch_down" | "touch_move" | "touch_up" => {
                let slot = number("slot") as u32;
                let phase = r["op"].as_str().unwrap();
                if self.touch_slots.contains(&slot) == (phase == "touch_down") {
                    return json!({"error": "invalid_touch_slot_state", "slot": slot});
                }
                let result =
                    policy_host::input::agent_touch(state, slot, phase, number("x"), number("y"));
                // An undeliverable release still ends this device's contact.
                // Otherwise destroying the target would strand the slot.
                if phase == "touch_up" {
                    self.touch_slots.remove(&slot);
                }
                if let Err(reply) = result {
                    return json!({"error": format!("{reply:?}")});
                }
                if phase == "touch_down" {
                    self.touch_slots.insert(slot);
                }
            }
            other => return json!({"error": format!("unknown request {other}")}),
        }
        json!({"ok": true})
    }
}
