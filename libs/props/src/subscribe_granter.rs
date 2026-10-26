// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! Contract: `docs/docs/spec/props/README.md#watch-and-topics`.

//! Watch-mediated subscriber bridge trait.
//!
//! Watch handlers ask noded to subscribe the original caller to the reserved
//! records-changed or audit topic, filtered to the requested namespace.
//! The subscriber name is routing data, never an access policy.

//! Production implementation wraps a `bus::native_client::NodedClient` and
//! issues the `noded.props.subscribe_grant` Bus command. Test
//! implementations (e.g. the recorder used in this crate's mutation
//! tests) just collect `(topic, target_peer, namespace)` tuples and
//! return success without ever leaving the test process.
//!
//! `Send + Sync` + boxed-future let callers hold the granter behind
//! `Arc<dyn ...>` and pass
//! it across spawn boundaries without static-type coupling.

use std::future::Future;
use std::pin::Pin;

pub trait SubscribeGranter: Send + Sync {
    /// Grant `target_peer` a broker subscription on `topic` filtered
    /// to publishes whose body's top-level `namespace` field equals
    /// `namespace`. Returns `Ok(())` when noded acknowledges with
    /// `rc=0`; any non-zero rc or transport failure maps to `Err`.
    ///
    /// The implementation is responsible for asserting (via noded's
    /// `subscribe_grant` command) that the calling service is the
    /// reserved-topic owner — the handler itself does not re-check
    /// ownership before calling `grant`. The owner-peer authentication
    /// happens broker-side using the connection-level `peer_id`
    /// established by `noded.register`.
    fn grant<'a>(
        &'a self,
        topic: &'a str,
        target_peer: &'a str,
        namespace: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;
}
