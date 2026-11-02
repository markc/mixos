// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lab extension ABI: shared connected sockets, bounded event queues and leases.
//! Tables and stream variants remain private. This API is intentionally provisional.
use crate::builtins::tcp_client as client;
use crate::{MixResult, value::Value};
use std::sync::{Arc, atomic::Ordering};

pub use client::{TcpOwner, TcpPermit, Ownership, TCP_PERMITS, NEXT_ID};
pub use crate::builtins::socket_sources::{ClientKey, SocketRecord, SocketSource, accept_timeout, deadline_after, client_id_of, number_of};
pub use crate::native_events::{Queue, refusal};
pub use crate::numeric::{as_duration, as_exact_integer};
pub use crate::numeric::{InputPolicy, extract_number};
pub use crate::builtins::{expect_args, expect_args_between, number_arg, bool_option, required_number_value};
pub use crate::udp::{ip, map, ttl, io, option, interface};

pub fn has_tcp(id: u64) -> bool { client::MAP.lock().unwrap().contains_key(&id) }
pub fn remove_tcp(id: u64) { drop(client::MAP.lock().unwrap().remove(&id)); }
pub fn retire_tcp(owner: &TcpOwner) { crate::builtins::socket_sources::close_owned(owner); }
pub fn owner_ids(owner: &TcpOwner) -> Vec<u64> { owner.ids.lock().unwrap().iter().copied().collect() }
pub fn adopt_udp(sock: std::net::UdpSocket, permit: TcpPermit) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    crate::udp::handles::MAP.lock().unwrap().insert(id, Arc::new(crate::udp::handles::Socket {
        sock, subscribed: false.into(), _permit: permit,
    }));
    id
}

pub fn adopt_accepted(stream: std::net::TcpStream, permit: TcpPermit, owner: Option<&Arc<TcpOwner>>) -> (u64, std::net::SocketAddr) {
    let peer = stream.peer_addr().unwrap_or_else(|_| ([0,0,0,0], 0).into());
    stream.set_nonblocking(false).ok();
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(30))).ok();
    stream.set_write_timeout(Some(std::time::Duration::from_secs(30))).ok();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut conn = client::Conn::new(client::Stream::Plain(stream), Some(permit));
    conn.ownership = owner.map(|o| Ownership::new(o.clone(), id));
    client::MAP.lock().unwrap().insert(id, conn);
    (id, peer)
}

pub fn markdown(args: Vec<Value>) -> MixResult<Option<Value>> { crate::builtins::call_builtin("markdown", args) }

pub use crate::builtins::socket_sources::{is_subscribed, unmark_subscribed, wait_ready, Wait};

#[cfg(feature = "test-support")]
pub mod testing {
    pub use crate::native_events::{NativeEvents, json_value};
    pub use crate::builtins::hex_encode;
    pub use crate::evaluator::incoming_event_to_mix_value;
    pub use crate::udp::{record, handles::get as get_udp};
    pub use crate::builtins::socket_sources::MAX_SOCKET_FRAMES;
}
