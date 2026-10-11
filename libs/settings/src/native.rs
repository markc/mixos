// SPDX-License-Identifier: MIT OR Apache-2.0
//! Optional native executor over the host's EXISTING supervised connection.
//! It never connects, takes the incoming receiver, spawns a task or polls.
use crate::{
    consumer::{Consumer, Work, WorkKind},
    *,
};
use Client as SupervisedClient;
use bus::native_client::IncomingCommand;
pub use bus::native_client::SupervisedClient as Client;
use std::time::Duration;

pub const BOOTSTRAP_BUDGET: Duration = Duration::from_secs(1);
/// Owner stamp written by noded on reserved-publisher topics (services/noded
/// `BROKER_SERVICE_HEADER`). Read only from the verified broker frame.
const BROKER_SERVICE_HEADER: &str = "broker_service";

/// Sample a coherent lifecycle/generation pair on the UI loop. Holding the
/// watch read guard prevents a state transition between the two observations.
pub fn live_generation(client: &Client) -> Option<u64> {
    let lifecycle = client.subscribe_state();
    let state = lifecycle.borrow();
    (*state == bus::native_client::ConnState::Connected).then(|| client.connection_generation())
}

/// Execute one fenced RECOVERY action, bounded to one second. Hosts multiplex
/// this future with incoming events and existing work. Initial bootstrap must
/// use execute_until with the SAME deadline for both subscribe and read.
pub async fn execute(
    client: &SupervisedClient,
    work: &Work,
) -> Result<Option<Snapshot>, Diagnostic> {
    execute_until(client, work, tokio::time::Instant::now() + BOOTSTRAP_BUDGET).await
}

/// Pass the same initial deadline for subscribe AND read, so their combined
/// bootstrap cannot spend a second budget. Recovery uses execute() instead.
pub async fn execute_until(
    client: &SupervisedClient,
    work: &Work,
    deadline: tokio::time::Instant,
) -> Result<Option<Snapshot>, Diagnostic> {
    fn fault(code: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(code, "native", message)
    }
    if !client.is_connected() || client.connection_generation() != work.generation() {
        return Err(fault(
            "stale_connection",
            "Work belongs to another connection",
        ));
    }
    let deadline = deadline.min(tokio::time::Instant::now() + BOOTSTRAP_BUDGET);
    if deadline <= tokio::time::Instant::now() {
        return Err(fault(
            "read_timeout",
            "Native work deadline already elapsed",
        ));
    }
    let result = tokio::time::timeout_at(deadline, async {
        match work.kind() {
            WorkKind::Subscribe => {
                client
                    .subscribe_topic(&topic(&work.binding().profile))
                    .await
                    .map_err(|e| fault("subscribe_failed", e.to_string()))?;
                Ok(None)
            }
            WorkKind::Read => {
                let body = serde_json::to_value(ReadRequest {
                    binding: work.binding().clone(),
                    view: View::Full,
                })
                .map_err(|e| fault("invalid_request", e.to_string()))?;
                let value = match client
                    .call_typed("settingsd", "settings.get", body)
                    .await
                    .map_err(|e| fault("read_failed", e.to_string()))?
                {
                    bus::PortReply::Ok { value, .. } => value,
                    bus::PortReply::AppError { message, .. } => {
                        let structured = serde_json::from_str::<serde_json::Value>(&message).ok();
                        let status = structured
                            .as_ref()
                            .and_then(|v| v.get("status"))
                            .and_then(|s| s.as_str());
                        let code = if matches!(
                            status,
                            Some(
                                "wrong_target"
                                    | "validation_failed"
                                    | "unsupported_schema"
                                    | "snapshot_too_large"
                                    | "not_served"
                            )
                        ) {
                            "authority_refused"
                        } else {
                            "read_failed"
                        };
                        return Err(fault(code, message));
                    }
                };
                let encoded =
                    serde_json::to_vec(&value).map_err(|e| fault("invalid_read", e.to_string()))?;
                if encoded.len() > MAX_SNAPSHOT_BYTES + 64 * 1024 {
                    return Err(fault(
                        "snapshot_too_large",
                        "Read response exceeds envelope budget",
                    ));
                }
                if value.get("status").and_then(|s| s.as_str()) != Some("current") {
                    return Err(fault(
                        "invalid_read",
                        "Authority did not return current state",
                    ));
                }
                serde_json::from_value(
                    value
                        .get("snapshot")
                        .cloned()
                        .ok_or_else(|| fault("invalid_read", "Missing snapshot"))?,
                )
                .map(Some)
                .map_err(|e| fault("invalid_snapshot", e.to_string()))
            }
        }
    })
    .await
    .map_err(|_| {
        fault(
            "read_timeout",
            "Native work exceeded its bootstrap/recovery deadline",
        )
    })?;
    if !client.is_connected() || client.connection_generation() != work.generation() {
        return Err(fault(
            "stale_connection",
            "Connection changed during native work",
        ));
    }
    result
}

impl Consumer {
    /// Feed a multiplexed native command without taking over the app's receiver.
    /// Unrelated commands are ignored. The broker-authenticated authority stamp,
    /// exact topic and connection generation are checked before decoding.
    pub fn native_delivery(&mut self, command: &IncomingCommand) -> Option<Work> {
        self.decoded_delivery(Decoded::from_command(self.binding(), command)?)
    }

    /// Complete decoding on the host's worker, then feed it on the UI loop.
    /// A queued delivery still checks the current binding and connection here.
    pub fn decoded_delivery(&mut self, delivery: Decoded) -> Option<Work> {
        if &delivery.binding != self.binding() || self.generation() != Some(delivery.generation) {
            return None;
        }
        match delivery.result {
            Ok(snapshot) => self.observe(delivery.generation, snapshot),
            Err(error) => self.rejected_delivery(error),
        }
    }
}

/// Broker-owner/topic admission and bounded decoding happen off the UI loop.
/// This is a host-local value, never deserialisable caller evidence. The host
/// must pass a command from its existing verified broker receiver.
#[derive(Debug)]
pub struct Decoded {
    binding: Binding,
    generation: u64,
    result: Result<Snapshot, Diagnostic>,
    fingerprint: Option<[u8; 32]>,
}
impl Decoded {
    /// Exact same bounded broker payload on the same captured binding/socket.
    /// Revision equality alone cannot discard a contradiction or new authority.
    pub fn same_message(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.generation == other.generation
            && self.fingerprint.is_some()
            && self.fingerprint == other.fingerprint
    }
    /// Admit one broker delivery. Only a frame on OUR topic is judged: any
    /// other frame is [`Admission::Other`]. A frame on our topic without the
    /// broker's `settingsd` owner stamp is [`Admission::Refused`] with a stable
    /// reason code, never silently dropped. The stamp is only written by a
    /// noded that knows the settings topic is reserved to settingsd; an older
    /// noded delivers the same topic unstamped.
    pub fn admit(binding: &Binding, command: &IncomingCommand) -> Admission {
        // Compared without building the topic string: a refused frame is the hot
        // path under a peer's flood, and it must not allocate.
        let on_our_topic = command
            .topic()
            .and_then(|topic| topic.strip_prefix(crate::TOPIC_PREFIX))
            == Some(binding.profile.as_str());
        if !on_our_topic {
            return Admission::Other;
        }
        match command.header(BROKER_SERVICE_HEADER) {
            Some("settingsd") => {}
            Some(_) => return Admission::Refused(REFUSED_WRONG_OWNER),
            None => return Admission::Refused(REFUSED_MISSING_OWNER),
        }
        Admission::Admitted(Box::new(Self::decode(binding, command)))
    }

    /// Admitted deliveries only; see [`Self::admit`] for the refusal reasons.
    pub fn from_command(binding: &Binding, command: &IncomingCommand) -> Option<Self> {
        match Self::admit(binding, command) {
            Admission::Admitted(decoded) => Some(*decoded),
            Admission::Other | Admission::Refused(_) => None,
        }
    }

    fn decode(binding: &Binding, command: &IncomingCommand) -> Self {
        let result = if command.body.len() > MAX_SNAPSHOT_BYTES {
            Err(Diagnostic::new(
                "invalid_delivery",
                "snapshot",
                "Canonical delivery exceeds inline budget",
            ))
        } else {
            serde_json::from_str::<Snapshot>(&command.body)
                .map_err(|error| Diagnostic::new("invalid_delivery", "snapshot", error.to_string()))
        };
        Self {
            binding: binding.clone(),
            generation: command.generation,
            result,
            fingerprint: (command.body.len() <= MAX_SNAPSHOT_BYTES)
                .then(|| *blake3::hash(command.body.as_bytes()).as_bytes()),
        }
    }
}

/// Stable reason codes for a refused delivery on our topic. They are logged and
/// reported by hosts, so they never embed the peer-supplied header value.
pub const REFUSED_MISSING_OWNER: &str = "missing_broker_service";
pub const REFUSED_WRONG_OWNER: &str = "wrong_broker_service";

/// Outcome of [`Decoded::admit`] for one broker frame.
#[derive(Debug)]
pub enum Admission {
    /// Not a delivery on this binding's settings topic.
    Other,
    /// On our topic but not owner-stamped by settingsd. Never decoded.
    Refused(&'static str),
    Admitted(Box<Decoded>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn binding() -> Binding {
        Binding {
            instance: "host".into(),
            profile: "default".into(),
        }
    }

    /// A delivery as the native reader builds it: `topic` names the topic and
    /// `broker_service` is present only when the broker stamped the owner.
    fn delivery(headers: &[(&str, &str)]) -> IncomingCommand {
        IncomingCommand {
            generation: 1,
            from: "noded".into(),
            command: topic("default"),
            id: None,
            args: serde_json::Value::Null,
            body: "{}".into(),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn an_unstamped_frame_on_our_topic_is_refused_with_a_reason() {
        // A noded older than the settings reservation delivers the topic
        // without broker_service. Admission must say so, not return None.
        let frame = delivery(&[("topic", &topic("default"))]);
        assert!(matches!(
            Decoded::admit(&binding(), &frame),
            Admission::Refused(REFUSED_MISSING_OWNER)
        ));
        assert!(Decoded::from_command(&binding(), &frame).is_none());
    }

    #[test]
    fn a_frame_stamped_by_another_owner_is_refused_with_a_reason() {
        let frame = delivery(&[("topic", &topic("default")), ("broker_service", "operator")]);
        assert!(matches!(
            Decoded::admit(&binding(), &frame),
            Admission::Refused(REFUSED_WRONG_OWNER)
        ));
    }

    #[test]
    fn a_settingsd_stamped_frame_on_our_topic_is_admitted() {
        let frame = delivery(&[
            ("topic", &topic("default")),
            ("broker_service", "settingsd"),
        ]);
        assert!(matches!(
            Decoded::admit(&binding(), &frame),
            Admission::Admitted(_)
        ));
        assert!(Decoded::from_command(&binding(), &frame).is_some());
    }

    #[test]
    fn other_topics_are_not_judged_and_stay_silent() {
        let frame = delivery(&[("topic", "settingsd.desktop.changed.other")]);
        assert!(matches!(
            Decoded::admit(&binding(), &frame),
            Admission::Other
        ));
    }
}
