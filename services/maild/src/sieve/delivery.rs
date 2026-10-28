// SPDX-License-Identifier: MIT OR Apache-2.0
//! Apply account Sieve to native local delivery. All destinations and flags are
//! checked before message side effects; script/policy errors retain the default.
use super::{Context, Plan, Vacation, compile, store};
use crate::{
    db::{self, Db},
    mailstore::{MailStore, MailboxRole, SqliteMailStore},
};
use anyhow::{Result, ensure};
use mail_parser::MessageParser;
use mds::{ContainerAttrs, ContainerId, Flags, ItemId, Mds, Tags};
use std::{collections::HashMap, sync::Arc};

/// Remove sender-supplied verdicts before adding the daemon's own values.
pub fn stamp_spam(data: &[u8], verdict: &str, score: f64) -> Vec<u8> {
    let mut bytes =
        format!("X-Mixos-Spam-Verdict: {verdict}\r\nX-Mixos-Spam-Score: {score:.4}\r\n")
            .into_bytes();
    let mut cursor = 0;
    if let Some(message) = MessageParser::default().parse(data) {
        for header in message.headers() {
            if ["X-Mixos-Spam-Verdict", "X-Mixos-Spam-Score"]
                .iter()
                .any(|n| header.name.as_str().eq_ignore_ascii_case(n))
            {
                bytes.extend_from_slice(&data[cursor..header.offset_field as usize]);
                cursor = header.offset_end as usize;
            }
        }
    }
    bytes.extend_from_slice(&data[cursor..]);
    bytes
}

#[derive(Debug)]
pub struct Rejected(pub String);
impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for Rejected {}

/// Original verified SMTP context, retained across local redirects so recipient
/// policy is rerun without treating stored Authentication-Results as evidence.
pub struct Routing<'a> {
    pub state: &'a crate::smtp::SmtpState,
    pub ingress: &'a crate::smtp::inbound::IngressAuth,
    pub verify: &'a maild_auth::VerifyResult,
    pub raw_data: &'a [u8],
    pub remote_ip: std::net::IpAddr,
}

pub struct Delivery<'a> {
    pub account: &'a db::account::Account,
    pub from: &'a str,
    pub to: &'a str,
    pub data: &'a [u8],
    pub default_mailbox: ContainerId,
    pub default_tags: Tags,
    pub external_domains: &'a [String],
    /// Authenticated local ingress or SPF/DMARC-aligned sender. Prevents
    /// forged-envelope backscatter on asynchronous reject and vacation.
    pub sender_verified: bool,
    pub spam: bool,
    pub smtp_reject: bool,
    pub routing: Option<&'a Routing<'a>>,
}

fn flags(raw: &[String]) -> Result<(Flags, Tags)> {
    let mut bits = 0;
    let mut tags = Vec::new();
    for word in raw.iter().flat_map(|s| s.split_whitespace()) {
        let bit = match word.to_ascii_lowercase().as_str() {
            "\\seen" => Some(Flags::SEEN),
            "\\answered" => Some(Flags::ANSWERED),
            "\\flagged" => Some(Flags::FLAGGED),
            "\\deleted" => Some(Flags::DELETED),
            "\\draft" => Some(Flags::DRAFT),
            "\\recent" => continue,
            _ => None,
        };
        if let Some(bit) = bit {
            bits |= bit;
        } else {
            if word.starts_with('\\') || !word.is_ascii() {
                continue;
            }
            let Ok(tag) = crate::keyword::normalise_user_keyword(word) else {
                continue;
            };
            if !tags.contains(&tag) && tags.len() < crate::keyword::MAX_KEYWORDS_PER_MESSAGE {
                tags.push(tag);
            }
        }
    }
    Ok((Flags(bits), Tags::from(tags)))
}

fn address(address: &str) -> Result<()> {
    let (local, domain) = address
        .rsplit_once('@')
        .ok_or_else(|| anyhow::anyhow!("invalid address"))?;
    ensure!(
        !local.is_empty()
            && !domain.is_empty()
            && address.len() <= 254
            && address
                .bytes()
                .all(|b| b > 32 && b < 127 && !b"<>(),;:\\[]\"".contains(&b)),
        "invalid address"
    );
    Ok(())
}

/// Evaluate only. Exposed for delivery diagnostics and malformed stored-script
/// recovery tests; the caller must still validate the action destinations.
pub async fn plan(
    db: &Db,
    ms: &Arc<SqliteMailStore>,
    delivery: &Delivery<'_>,
) -> Result<Option<Plan>> {
    let Some(script) = store::active(db, delivery.account.id).await? else {
        return Ok(None);
    };
    let data = delivery.data.to_vec();
    let from = delivery.from.to_string();
    let to = delivery.to.to_string();
    let ms = ms.clone();
    let account = delivery.account.id;
    tokio::task::spawn_blocking(move || {
        let script = compile(&script.source)?;
        let paths = if script.needs_mailboxes() {
            mailbox_snapshot(&super::mailboxes::load(&ms, account)?)?
        } else {
            Vec::new()
        };
        script.evaluate_with_mailboxes(
            Context {
                data: &data,
                from: &from,
                to: &to,
            },
            &paths,
        )
    })
    .await?
    .map(Some)
}

fn mailbox_snapshot(boxes: &[super::mailboxes::Mailbox]) -> Result<Vec<String>> {
    ensure!(
        boxes.len() <= super::MAX_STEPS,
        "mailbox snapshot count limit"
    );
    let by_id: HashMap<_, _> = boxes.iter().map(|entry| (entry.id, entry)).collect();
    let mut steps = 0;
    let mut bytes = 0;
    let mut paths = Vec::new();
    for mailbox in boxes {
        if !mailbox.writable {
            continue;
        }
        let mut parts = Vec::new();
        let mut path_bytes = 0;
        let mut current = Some(mailbox);
        while let Some(entry) = current {
            steps += 1;
            ensure!(steps <= super::MAX_STEPS, "mailbox snapshot work limit");
            ensure!(parts.len() < boxes.len(), "mailbox parent cycle");
            let name = if entry.inbox { "INBOX" } else { &entry.name };
            path_bytes += name.len() + 1;
            bytes += name.len() + 1;
            ensure!(bytes <= 1024 * 1024, "mailbox snapshot allocation limit");
            parts.push(name.to_string());
            current = entry
                .parent
                .map(|parent| {
                    by_id
                        .get(&parent)
                        .copied()
                        .ok_or_else(|| anyhow::anyhow!("missing mailbox parent"))
                })
                .transpose()?;
        }
        bytes += path_bytes;
        ensure!(bytes <= 1024 * 1024, "mailbox snapshot allocation limit");
        parts.reverse();
        paths.push(parts.join("/"));
    }
    Ok(paths)
}

/// Return the first stored item for existing verdict/log events. None means the
/// script handled the message without a local filing (discard/redirect/reject).
pub async fn deliver(
    db: &Db,
    ms: &Arc<SqliteMailStore>,
    delivery: Delivery<'_>,
) -> Result<Option<ItemId>> {
    let mut remaining = super::MAX_REDIRECTS;
    deliver_inner(db, ms, delivery, &mut remaining).await
}

async fn deliver_inner(
    db: &Db,
    ms: &Arc<SqliteMailStore>,
    delivery: Delivery<'_>,
    remaining: &mut usize,
) -> Result<Option<ItemId>> {
    let selected = plan(db, ms, &delivery).await;
    let mut plan = match selected {
        Ok(Some(plan)) => plan,
        Ok(None) => Plan {
            filings: vec![super::interpreter::Filing::default()],
            ..Plan::default()
        },
        Err(error) => {
            tracing::warn!(account=delivery.account.id, %error, "Sieve error; keeping message");
            Plan {
                filings: vec![super::interpreter::Filing::default()],
                ..Plan::default()
            }
        }
    };
    let external = delivery
        .account
        .email
        .rsplit_once('@')
        .is_some_and(|(_, d)| {
            delivery
                .external_domains
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(d))
        });
    let validation = validate(db, &delivery, &plan, external, *remaining).await;
    let redirects = match validation {
        Ok(redirects) => redirects,
        Err(error) => {
            tracing::warn!(account=delivery.account.id, %error, "Sieve action refused; keeping message");
            plan = Plan {
                filings: vec![super::interpreter::Filing::default()],
                ..Plan::default()
            };
            Vec::new()
        }
    };
    if let Some(reason) = &plan.reject {
        if delivery.smtp_reject && (reason.is_ascii() || plan.extended_reject) {
            let mut reason: String = if reason.is_ascii() {
                reason.chars().filter(|c| !c.is_control()).collect()
            } else {
                "Message refused by recipient filter".into()
            };
            let mut end = reason.len().min(400);
            while !reason.is_char_boundary(end) {
                end -= 1;
            }
            reason.truncate(end);
            return Err(Rejected(reason).into());
        }
        if delivery.sender_verified && !delivery.from.is_empty() {
            let domain = delivery
                .account
                .email
                .rsplit_once('@')
                .map(|(_, d)| d)
                .unwrap_or("example.com");
            let bytes = rejection_report(domain, delivery.from, delivery.to, reason);
            enqueue(db, ms, "", delivery.from, &bytes).await?;
        } else {
            tracing::info!(
                account = delivery.account.id,
                "Sieve rejection DSN suppressed: null or unverified sender"
            );
        }
        return Ok(None);
    }
    // Resolve *all* filing destinations before putting the first message.
    let ms2 = ms.clone();
    let id = delivery.account.id;
    let default = delivery.default_mailbox;
    let filings = plan.filings.clone();
    let resolved = tokio::task::spawn_blocking(move || -> Result<Vec<_>> {
        let mut resolved: Vec<(ContainerId, Flags, Tags)> = Vec::new();
        let mut resolver = if filings.iter().any(|filing| filing.mailbox.is_some()) {
            Some(MailboxResolver::from_mailboxes(super::mailboxes::load(
                &ms2, id,
            )?)?)
        } else {
            None
        };
        // Check every non-create destination before creating any folders.
        let planned: Vec<_> = filings
            .iter()
            .filter(|filing| filing.create)
            .filter_map(|filing| filing.mailbox.as_deref())
            .map(super::interpreter::mailbox_key)
            .collect();
        for filing in &filings {
            if !filing.create
                && let Some(name) = &filing.mailbox
            {
                let key = super::interpreter::mailbox_key(name);
                let prefix = format!("{key}/");
                if !planned
                    .iter()
                    .any(|path| path == &key || path.starts_with(prefix.as_str()))
                {
                    resolver.as_mut().unwrap().resolve(&ms2, id, name, false)?;
                }
            }
        }
        for filing in filings {
            let target = if let Some(name) = filing.mailbox {
                resolver
                    .as_mut()
                    .unwrap()
                    .resolve(&ms2, id, &name, filing.create)?
            } else {
                default
            };
            let (flags, tags) = flags(&filing.flags)?;
            if let Some((_, existing_flags, existing_tags)) =
                resolved.iter_mut().find(|(prior, _, _)| prior == &target)
            {
                // RFC 5232 section 2: the last flag list wins when duplicate
                // deliveries to one mailbox are collapsed.
                *existing_flags = flags;
                *existing_tags = tags;
            } else {
                resolved.push((target, flags, tags));
            }
        }
        Ok(resolved)
    })
    .await?;
    let resolved = match resolved {
        Ok(targets) => targets,
        Err(error) => {
            tracing::warn!(account=id,%error,"Sieve fileinto failed; keeping message");
            plan.redirects.clear();
            plan.vacation = None;
            vec![(delivery.default_mailbox, Flags(0), Tags::new())]
        }
    };
    let mut first = None;
    for (target, flags, mut tags) in resolved {
        // Content publishing tags may only survive a keep in the global
        // filter's original destination, never a user-selected fileinto.
        if target == delivery.default_mailbox {
            for tag in delivery.default_tags.iter() {
                tags.insert(tag.clone());
            }
        }
        let ms = ms.clone();
        let data = delivery.data.to_vec();
        let item = tokio::task::spawn_blocking(move || -> Result<ItemId> {
            let message = MessageParser::default()
                .parse(&data)
                .ok_or_else(|| anyhow::anyhow!("parse message"))?;
            let hash = ms.mds().put_blob(&data)?;
            Ok(ms
                .create_email(
                    id,
                    target,
                    hash,
                    crate::jmap::email::build_envelope(&message),
                    &crate::jmap::email::extract_in_reply_to(&message),
                    flags,
                    tags,
                    chrono::Utc::now().timestamp_millis(),
                )?
                .0)
        })
        .await?;
        match item {
            Ok(item) => {
                first.get_or_insert(item);
            }
            Err(error) if first.is_some() => {
                tracing::warn!(%error,"Sieve filing failed after another filing committed; accepting partial delivery")
            }
            Err(error) => return Err(error),
        }
    }
    if !plan.redirects.is_empty() {
        *remaining -= plan.redirects.len();
        let mut handled = first.is_some() || plan.discarded;
        for (destination, local) in redirects {
            let depth = redirect_depth(delivery.data);
            if depth >= super::MAX_REDIRECTS {
                tracing::warn!("Sieve redirect loop suppressed");
                continue;
            }
            let mut bytes = format!("X-Mixos-Sieve-Hop: {}\r\n", depth + 1).into_bytes();
            bytes.extend_from_slice(delivery.data);
            let result = async {
                if let Some(account) = local {
                    let inbox = ms
                        .mailbox_by_role(account.id, MailboxRole::Inbox)?
                        .ok_or_else(|| anyhow::anyhow!("redirect recipient has no Inbox"))?;
                    let (target, routed_data, spam) = if let Some(routing) = delivery.routing {
                        crate::smtp::inbound::route_sieve_redirect(
                            routing,
                            &account,
                            delivery.from,
                            &destination,
                            &bytes,
                        )
                        .await?
                    } else {
                        (inbox, bytes.clone(), delivery.spam)
                    };
                    Box::pin(deliver_inner(
                        db,
                        ms,
                        Delivery {
                            account: &account,
                            from: delivery.from,
                            to: &destination,
                            data: &routed_data,
                            default_mailbox: target,
                            default_tags: Tags::new(),
                            external_domains: delivery.external_domains,
                            sender_verified: delivery.sender_verified,
                            spam,
                            smtp_reject: false,
                            routing: delivery.routing,
                        },
                        remaining,
                    ))
                    .await?;
                } else {
                    enqueue(db, ms, delivery.from, &destination, &bytes).await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            match result {
                Ok(()) => handled = true,
                Err(error) if handled => {
                    tracing::warn!(%error,"Sieve redirect failed after another action committed; accepting partial delivery")
                }
                Err(error) => return Err(error),
            }
        }
    }
    if let Some(vacation) = plan.vacation
        && let Err(error) = vacation_reply(db, ms, &delivery, &vacation, external).await
    {
        tracing::warn!(%error,"Sieve vacation reply failed; retaining original delivery outcome");
    }
    Ok(first)
}

fn rejection_report(domain: &str, sender: &str, recipient: &str, reason: &str) -> Vec<u8> {
    let safe = crate::smtp::inbound::header_value_safe;
    let domain = safe(domain);
    let sender = safe(sender);
    let recipient = safe(recipient);
    let boundary = format!("sieve-{}", uuid::Uuid::new_v4());
    let reason = reason
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n");
    let date = chrono::Utc::now().to_rfc2822();
    // RFC 3464 multipart/report: readable reason and machine-readable status.
    // Do not attach the original message or reflect its headers.
    format!("From: <postmaster@{domain}>\r\nTo: <{sender}>\r\nDate: {date}\r\nMessage-ID: <{boundary}@{domain}>\r\nSubject: Message refused by recipient filter\r\nAuto-Submitted: auto-generated\r\nMIME-Version: 1.0\r\nContent-Type: multipart/report; report-type=delivery-status; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\nThe recipient's mail filter refused your message.\r\n\r\n{reason}\r\n\r\n--{boundary}\r\nContent-Type: message/delivery-status\r\n\r\nReporting-MTA: dns; {domain}\r\n\r\nFinal-Recipient: rfc822; {recipient}\r\nAction: failed\r\nStatus: 5.7.1\r\n\r\n--{boundary}--\r\n").into_bytes()
}

fn redirect_depth(data: &[u8]) -> usize {
    MessageParser::default()
        .parse(data)
        .map(|m| {
            m.headers()
                .iter()
                .filter(|h| h.name.as_str().eq_ignore_ascii_case("X-Mixos-Sieve-Hop"))
                .filter_map(|h| h.value.as_text()?.trim().parse::<usize>().ok())
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}
async fn validate(
    db: &Db,
    delivery: &Delivery<'_>,
    plan: &Plan,
    external: bool,
    remaining: usize,
) -> Result<Vec<(String, Option<db::account::Account>)>> {
    let mut redirects = Vec::new();
    ensure!(
        plan.redirects.is_empty() || redirect_depth(delivery.data) < super::MAX_REDIRECTS,
        "redirect hop limit"
    );
    ensure!(
        plan.redirects.len() <= remaining,
        "local redirect budget exhausted"
    );
    for target in &plan.redirects {
        address(target)?;
        let local = db::account::get_by_email(&db.conn, &target.to_ascii_lowercase()).await?;
        ensure!(
            local.is_some() || external,
            "off-site redirect disabled for this domain"
        );
        redirects.push((target.clone(), local));
    }
    for filing in &plan.filings {
        flags(&filing.flags)?;
        if let Some(name) = &filing.mailbox {
            ensure!(
                !name.is_empty() && name.len() <= 255 && !name.chars().any(char::is_control),
                "invalid mailbox name"
            );
            ensure!(
                name.split('/')
                    .all(|s| !s.is_empty() && s != "." && s != ".."),
                "invalid mailbox path"
            );
        }
    }
    if let Some(vacation) = &plan.vacation {
        if let Some(from) = &vacation.from {
            address(from)?;
            ensure!(
                from.eq_ignore_ascii_case(&delivery.account.email),
                "vacation :from must be account address"
            );
        }
        if vacation.mime {
            let entity = MessageParser::default()
                .parse(&vacation.reason)
                .ok_or_else(|| anyhow::anyhow!("invalid vacation MIME entity"))?;
            ensure!(
                vacation.reason.contains("\n\n") || vacation.reason.contains("\r\n\r\n"),
                "vacation MIME entity needs a header/body separator"
            );
            ensure!(
                entity.headers().iter().all(|h| h
                    .name
                    .as_str()
                    .to_ascii_lowercase()
                    .starts_with("content-")),
                "vacation MIME entity may contain only Content-* headers"
            );
        }
    }
    Ok(redirects)
}

struct MailboxResolver {
    children: HashMap<(Option<ContainerId>, String), ContainerId>,
    inbox: Option<ContainerId>,
    paths: HashMap<String, ContainerId>,
    steps: usize,
    refreshed: bool,
}
impl MailboxResolver {
    fn from_mailboxes(boxes: Vec<super::mailboxes::Mailbox>) -> Result<Self> {
        ensure!(
            boxes.len() <= super::MAX_STEPS,
            "destination snapshot count limit"
        );
        let inbox = boxes.iter().find(|entry| entry.inbox).map(|entry| entry.id);
        let mut children = HashMap::new();
        let mut bytes = 0;
        for entry in boxes {
            bytes += entry.name.len();
            ensure!(
                bytes <= 1024 * 1024,
                "destination snapshot allocation limit"
            );
            children.insert((entry.parent, entry.name), entry.id);
        }
        Ok(Self {
            children,
            inbox,
            paths: HashMap::new(),
            steps: 0,
            refreshed: false,
        })
    }
    fn resolve(
        &mut self,
        ms: &SqliteMailStore,
        account: i32,
        path: &str,
        create: bool,
    ) -> Result<ContainerId> {
        let path_key = super::interpreter::mailbox_key(path);
        if let Some(id) = self.paths.get(&path_key) {
            return Ok(*id);
        }
        let mut parent = None;
        for (index, name) in path.split('/').enumerate() {
            self.steps += 1;
            ensure!(
                self.steps <= super::MAX_STEPS,
                "destination path work limit"
            );
            let key = (parent, name.to_string());
            let existing = if index == 0 && name.eq_ignore_ascii_case("INBOX") {
                self.inbox
            } else {
                self.children.get(&key).copied()
            };
            let id = if let Some(id) = existing {
                id
            } else {
                ensure!(create, "fileinto mailbox does not exist");
                match ms.create_mailbox(
                    account,
                    name,
                    parent,
                    ContainerAttrs {
                        special_use: None,
                        subscribed: false,
                        extra: serde_json::json!({}),
                    },
                ) {
                    Ok(id) => id,
                    Err(error) => {
                        // Recover one concurrent create, with bounded work. A
                        // changing namespace cannot force repeated full reloads.
                        ensure!(!self.refreshed, "destination namespace changed repeatedly");
                        self.refreshed = true;
                        let fresh = super::mailboxes::load(ms, account)?;
                        self.steps += fresh.len();
                        ensure!(
                            self.steps <= super::MAX_STEPS,
                            "destination refresh work limit"
                        );
                        fresh
                            .into_iter()
                            .find(|entry| entry.parent == parent && entry.name == name)
                            .map(|entry| entry.id)
                            .ok_or(error)?
                    }
                }
            };
            self.children.insert(key, id);
            parent = Some(id);
        }
        let id = parent.ok_or_else(|| anyhow::anyhow!("empty mailbox path"))?;
        self.paths.insert(path_key, id);
        Ok(id)
    }
}

async fn enqueue(
    db: &Db,
    ms: &Arc<SqliteMailStore>,
    from: &str,
    to: &str,
    bytes: &[u8],
) -> Result<()> {
    let ms = ms.clone();
    let bytes = bytes.to_vec();
    let hash = tokio::task::spawn_blocking(move || ms.mds().put_blob(&bytes)).await??;
    // The existing queue worker performs normal per-domain DKIM signing.
    crate::smtp::queue::enqueue(&db.conn, from, &[to.to_string()], hash).await?;
    Ok(())
}

async fn vacation_reply(
    db: &Db,
    ms: &Arc<SqliteMailStore>,
    delivery: &Delivery<'_>,
    vacation: &Vacation,
    external: bool,
) -> Result<()> {
    let Some(message) = MessageParser::default().parse(delivery.data) else {
        return Ok(());
    };
    if delivery.spam
        || !delivery.sender_verified
        || delivery.from.is_empty()
        || delivery.from.eq_ignore_ascii_case(&delivery.account.email)
    {
        return Ok(());
    }
    if address(delivery.from).is_err() {
        return Ok(());
    }
    let sender_local = db::account::get_by_email(&db.conn, &delivery.from.to_ascii_lowercase())
        .await?
        .is_some();
    if !sender_local && !external {
        return Ok(());
    }
    let sender_part = delivery
        .from
        .split('@')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if ["mailer-daemon", "postmaster", "owner-"]
        .iter()
        .any(|s| sender_part.starts_with(s))
        || sender_part.ends_with("-request")
    {
        return Ok(());
    }
    for name in [
        "Auto-Submitted",
        "List-Id",
        "List-Post",
        "List-Help",
        "List-Unsubscribe",
        "List-Subscribe",
        "List-Owner",
        "List-Archive",
    ] {
        for value in message.header_values(name) {
            if name != "Auto-Submitted"
                || !value
                    .as_text()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("no")
            {
                return Ok(());
            }
        }
    }
    if message.header_values("Precedence").any(|h| {
        h.as_text().is_some_and(|v| {
            ["bulk", "list", "junk"]
                .iter()
                .any(|p| v.trim().eq_ignore_ascii_case(p))
        })
    }) {
        return Ok(());
    }
    // Never respond to a Bcc-only recipient. :addresses adds recognised
    // recipient identities; it does not authorise sender spoofing.
    let mut addresses = vacation.addresses.clone();
    addresses.push(delivery.account.email.clone());
    let addressed = [message.to(), message.cc()]
        .into_iter()
        .flatten()
        .flat_map(|a| a.iter())
        .filter_map(|a| a.address())
        .any(|a| addresses.iter().any(|v| v.eq_ignore_ascii_case(a)));
    if !addressed {
        return Ok(());
    }
    if !store::reply_due(
        db,
        delivery.account.id,
        delivery.from,
        &vacation.handle,
        vacation.days,
    )
    .await?
    {
        return Ok(());
    }
    let subject = crate::smtp::inbound::header_value_safe(
        vacation.subject.as_deref().unwrap_or("Auto: Out of Office"),
    );
    let mut builder = mail_builder::MessageBuilder::new()
        .from(delivery.account.email.as_str())
        .to(delivery.from)
        .subject(subject.as_str())
        .header(
            "Auto-Submitted",
            mail_builder::headers::raw::Raw::new("auto-replied"),
        )
        .text_body(vacation.reason.as_str());
    if let Some(id) = message.message_id() {
        builder = builder.in_reply_to(id);
    }
    let mut bytes = builder.write_to_vec()?;
    if vacation.mime {
        let message = MessageParser::default()
            .parse(&bytes)
            .ok_or_else(|| anyhow::anyhow!("vacation reply parse error"))?;
        let mut mime_reply = Vec::new();
        for header in message.headers() {
            let name = header.name.as_str();
            if name.eq_ignore_ascii_case("MIME-Version")
                || name.to_ascii_lowercase().starts_with("content-")
            {
                continue;
            }
            mime_reply.extend_from_slice(
                &bytes[header.offset_field as usize..header.offset_end as usize],
            );
            if !mime_reply.ends_with(b"\r\n") {
                mime_reply.extend_from_slice(b"\r\n");
            }
        }
        mime_reply.extend_from_slice(b"MIME-Version: 1.0\r\n");
        let entity = vacation
            .reason
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "\r\n");
        mime_reply.extend_from_slice(entity.as_bytes());
        if !mime_reply.ends_with(b"\r\n") {
            mime_reply.extend_from_slice(b"\r\n");
        }
        bytes = mime_reply;
    }
    // Null reverse path prevents replies to this automatic response; From
    // identifies the account for the normal queue worker's DKIM selection.
    let ms = ms.clone();
    let hash = tokio::task::spawn_blocking(move || ms.mds().put_blob(&bytes)).await??;
    store::queue_reply(
        db,
        delivery.account.id,
        delivery.from,
        &vacation.handle,
        vacation.days,
        hash,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn destination_metadata_limits_precede_index_allocation() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let ms = SqliteMailStore::new(Arc::new(mds::SqliteCasMds::open(tmp.path())?));
        ms.create_mailbox(
            1,
            "Inbox",
            None,
            ContainerAttrs {
                special_use: Some("\\Inbox".into()),
                subscribed: true,
                extra: serde_json::json!({}),
            },
        )?;
        let record = super::super::mailboxes::load(&ms, 1)?
            .into_iter()
            .next()
            .unwrap();
        assert!(
            MailboxResolver::from_mailboxes(vec![record.clone(); super::super::MAX_STEPS + 1])
                .is_err()
        );
        let mut oversized = record;
        oversized.name = "x".repeat(1024 * 1024 + 1);
        assert!(MailboxResolver::from_mailboxes(vec![oversized]).is_err());
        Ok(())
    }
}
