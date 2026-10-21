// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Test-only wlcs adapter. The inherited socket is private to the owned child;
//! it is never a listening control service or a replacement for the Bus.

use comp_model::request::{InputOp, PlaceSpec, PointerMoveTarget, PressAction, WindowOp};
use dispatcher::wire::trait_::surface_event::SurfaceHandle;
use dispatcher::wire::trait_::wire_trait::WireTrait;
use protocols::wayland::connection::record::record::WaylandClientSession;
use serde_json::{Value, json};
use smithay::{
    reexports::{
        calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic},
        wayland_server::{Client, Resource},
    },
    wayland::compositor::CompositorClientState,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    io,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::net::{UnixDatagram, UnixStream},
    rc::Rc,
    sync::Arc,
};
use surfaces::SeatKind;
use world::state::Loop;

type Pending = Rc<RefCell<Vec<(Value, Option<UnixStream>)>>>;
pub struct Adapter {
    socket: Rc<UnixDatagram>,
    pending: Pending,
    clients: HashMap<i32, Client>,
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
    socket.send(b"ready")?;
    Ok(Some(Adapter {
        socket,
        pending,
        clients: HashMap::new(),
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
                    let op = WindowOp::Place(PlaceSpec {
                        id: id.0,
                        generation: record.generation(),
                        output: None,
                        x: Some(number("x")),
                        y: Some(number("y")),
                        width: None,
                        height: None,
                    });
                    return json!({"reply": format!("{:?}", policy_host::control::window(state, &op))});
                }
                return json!({"error": "window_not_mapped"});
            }
            "absolute" | "relative" | "button" => {
                let op = match r["op"].as_str().unwrap() {
                    "absolute" => InputOp::PointerMove {
                        target: PointerMoveTarget::Output {
                            output: None,
                            x: number("x"),
                            y: number("y"),
                        },
                        corners: false,
                    },
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
                return json!({"reply": format!("{reply:?}")});
            }
            "touch_down" | "touch_move" | "touch_up" => {
                policy_host::input::agent_touch(
                    state,
                    number("slot") as u32,
                    r["op"].as_str().unwrap(),
                    number("x"),
                    number("y"),
                );
            }
            "stop" => state.inner.loader.loop_signal.stop(),
            other => return json!({"error": format!("unknown request {other}")}),
        }
        json!({"ok": true})
    }
}
