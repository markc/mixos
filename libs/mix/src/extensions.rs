// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explicit, process-local builtin families. Install before creating evaluators.
use crate::{builtin_info::BuiltinInfo, error::MixResult, value::Value};
use std::sync::{Arc, RwLock};

pub const SERVER_NAMES: &[&str] = &["tcp_listen", "tcp_accept", "udp_listen", "http_serve", "http_recv", "udp_recv"];
static FAMILIES: RwLock<Vec<&'static dyn BuiltinFamily>> = RwLock::new(Vec::new());

pub trait BuiltinFamily: Send + Sync {
    fn name(&self) -> &'static str;
    fn entries(&self) -> Vec<&'static BuiltinInfo>;
    fn call(&self, name: &str, args: Vec<Value>) -> MixResult<Option<Value>>;
    #[cfg(feature = "net-client")]
    fn sockets(&self) -> Option<&dyn SocketExtension> { None }
}

/// Idempotent for the same instance; duplicate names from another family refuse.
pub fn register(family: &'static dyn BuiltinFamily) -> MixResult<()> {
    let mut families = FAMILIES.write().unwrap();
    if families.iter().any(|f| std::ptr::eq(*f, family)) { return Ok(()); }
    let entries = family.entries();
    for entry in &entries {
        if families.iter().any(|f| f.entries().iter().any(|e| e.name == entry.name)) {
            return Err(crate::native_events::refusal("BUILTIN_DUPLICATE", entry.name));
        }
    }
    families.push(family);
    Ok(())
}

pub fn entries() -> Vec<&'static BuiltinInfo> {
    FAMILIES.read().unwrap().iter().flat_map(|f| f.entries()).collect()
}
pub fn family(name: &str) -> Option<&'static dyn BuiltinFamily> {
    FAMILIES.read().unwrap().iter().copied().find(|f| f.entries().iter().any(|e| e.name == name))
}
pub fn available(name: &str) -> bool { family(name).is_some() }
pub fn needs_host(name: &str) -> crate::MixError {
    crate::native_events::refusal("NEEDS_MIXD", format!("{name}() needs server capability; run it under: mixd run <script>"))
}

#[cfg(feature = "net-client")]
pub fn sockets() -> Option<&'static dyn SocketExtension> {
    FAMILIES.read().unwrap().iter().find_map(|f| f.sockets())
}

#[cfg(feature = "net-client")]
pub type SocketFuture = std::pin::Pin<Box<dyn std::future::Future<Output = MixResult<Value>>>>;

/// Hooks into core's shared socket handles and generation lifecycle. No listener
/// implementation or accept loop lives in core. Retirement runs after readers stop.
#[cfg(feature = "net-client")]
pub trait SocketExtension: Send + Sync {
    fn is_listener(&self, id: u64) -> bool;
    fn local_addr(&self, id: u64) -> Option<std::net::SocketAddr>;
    fn close(&self, id: u64) -> MixResult<Option<bool>>;
    fn unmark(&self, id: u64);
    fn subscribed(&self, id: u64) -> bool;
    fn own(&self, owner: &Arc<crate::server_api::TcpOwner>, id: u64);
    fn retire(&self, owner: &crate::server_api::TcpOwner);
    fn accept(&self, args: Vec<Value>, owner: Arc<crate::server_api::TcpOwner>, interruptible: bool) -> SocketFuture;
    fn subscribe(&self, queue: Arc<crate::server_api::Queue>, id: String, handle: u64,
        owner: Arc<crate::server_api::TcpOwner>) -> MixResult<crate::server_api::SocketSource>;
}
