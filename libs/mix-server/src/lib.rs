// SPDX-License-Identifier: MIT OR Apache-2.0
//! Server family installed explicitly by mixd, never by a Cargo feature.
use api::*;
pub use api::{accept_timeout, deadline_after};
use mix::{MixError, MixResult, server_api as api, value::Value};
use std::{
    net::{Ipv4Addr, UdpSocket},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
mod http;
mod subscriptions;
mod tcp;
mod udp;
pub use http::*;
use mix::interrupt;
pub use subscriptions::*;
pub use tcp::*;
pub use udp::*;
mod tcp_client {
    pub use mix::server_api::{NEXT_ID, Ownership, TCP_PERMITS, TcpOwner, TcpPermit};
}
mod socket_sources {
    pub use crate::{accept_timeout, prepare_accept};
}

pub struct Server;
static SERVER: Server = Server;
pub fn register() -> MixResult<()> {
    mix::extensions::register(&SERVER)
}

impl mix::extensions::BuiltinFamily for Server {
    fn name(&self) -> &'static str {
        "server"
    }
    fn entries(&self) -> Vec<&'static mix::builtin_info::BuiltinInfo> {
        mix::extensions::SERVER_NAMES
            .iter()
            .map(|n| mix::builtins::core_builtin_info(n).unwrap())
            .collect()
    }
    fn call(&self, name: &str, args: Vec<Value>) -> MixResult<Option<Value>> {
        match name {
            "tcp_listen" => builtin_tcp_listen(args),
            "tcp_accept" => builtin_tcp_accept(args),
            "udp_listen" => {
                expect_args_between(name, &args, 2, 3)?;
                listen(args)
            }
            "http_serve" => builtin_http_serve(args),
            "http_recv" => builtin_http_recv(args),
            "udp_recv" => builtin_udp_recv(args),
            _ => unreachable!("family dispatch name"),
        }
    }
    fn sockets(&self) -> Option<&dyn mix::extensions::SocketExtension> {
        Some(self)
    }
}

static LISTENING: LazyLock<Mutex<std::collections::HashMap<u64, Option<std::net::SocketAddr>>>> =
    LazyLock::new(|| Mutex::new(Default::default()));
fn mark_listening(id: u64, addr: Option<std::net::SocketAddr>) {
    LISTENING.lock().unwrap().insert(id, addr);
}
fn is_subscribed(key: ClientKey) -> bool {
    api::is_subscribed(key)
}
fn unmark_subscribed(key: ClientKey) {
    api::unmark_subscribed(key)
}

impl mix::extensions::SocketExtension for Server {
    fn is_listener(&self, id: u64) -> bool {
        tcp_server::LISTENERS.lock().unwrap().contains_key(&id)
            || LISTENING.lock().unwrap().contains_key(&id)
    }
    fn local_addr(&self, id: u64) -> Option<std::net::SocketAddr> {
        if let Some(l) = tcp_server::LISTENERS.lock().unwrap().get(&id) {
            return l.sock.local_addr().ok();
        }
        LISTENING.lock().unwrap().get(&id).copied().flatten()
    }
    fn close(&self, id: u64) -> MixResult<Option<bool>> {
        if LISTENING.lock().unwrap().contains_key(&id) {
            return Err(refusal(
                "SOCKET_SUBSCRIBED",
                "close a listener subscription with tcp_unwatch",
            ));
        }
        let l = tcp_server::LISTENERS.lock().unwrap().remove(&id);
        if let Some(l) = l {
            close_listener(l);
            Ok(Some(true))
        } else {
            Ok(None)
        }
    }
    fn unmark(&self, id: u64) {
        LISTENING.lock().unwrap().remove(&id);
    }
    fn subscribed(&self, id: u64) -> bool {
        LISTENING.lock().unwrap().contains_key(&id)
    }
    fn own(&self, owner: &Arc<TcpOwner>, id: u64) {
        if let Some(l) = tcp_server::LISTENERS.lock().unwrap().get(&id) {
            let mut o = l.ownership.lock().unwrap();
            if o.is_none() {
                *o = Some(Ownership::new(owner.clone(), id));
            }
        }
    }
    fn retire(&self, owner: &TcpOwner) {
        for id in api::owner_ids(owner) {
            let l = tcp_server::LISTENERS.lock().unwrap().remove(&id);
            if let Some(l) = l {
                close_listener(l);
            }
        }
    }
    fn accept(
        &self,
        args: Vec<Value>,
        owner: Arc<TcpOwner>,
        interruptible: bool,
    ) -> mix::extensions::SocketFuture {
        Box::pin(async move {
            let (l, d) = prepare_accept(&args)?;
            pull_accept(l, d, owner, interruptible).await
        })
    }
    fn subscribe(
        &self,
        queue: Arc<Queue>,
        id: String,
        handle: u64,
        owner: Arc<TcpOwner>,
    ) -> MixResult<SocketSource> {
        subscribe_listener(queue, id, handle, owner)
    }
}

const ACCEPT_SLICE: std::time::Duration = std::time::Duration::from_millis(250);
enum DrainOutcome {
    More,
    Yield,
    Overflow,
    Terminal(String),
}
