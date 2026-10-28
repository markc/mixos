---
title: Account Sieve and ManageSieve
description: Account delivery scripts, forwarding policy and TLS script editing
---

# Account Sieve and ManageSieve

Each account can store 32 named Sieve scripts in mail.db, with one active
script. An empty active name disables Sieve. Uploads compile the whole script
before changing stored data, including replacements of active scripts.

The interpreter is Rust inside maild with no subprocess, filesystem or network
access. The maintained [Stalwart sieve-rs](https://github.com/stalwartlabs/sieve)
was evaluated first: its AGPL licence fails MixOS's permissive dependency policy.
No suitable maintained permissive Rust interpreter was found. This implementation
uses the existing MIME parser and native mail store without new dependencies.

## Language

Base commands are `require`, `if`/`elsif`/`else`, `stop`, `keep`, `discard` and
`redirect`. Tests are `address`, `header`, `exists`, `size`, `allof`, `anyof`,
`not`, `true` and `false`. Quoted and `text:` multi-line strings, hash and
bracketed comments, `:is`/`:contains`/`:matches`, `i;ascii-casemap` and `i;octet`
are supported.

Extensions: `fileinto`, `mailbox` (`fileinto :create`), `envelope`, `imap4flags`,
`copy`, `body`, `variables`, `vacation`, `relational`, `comparator-i;ascii-numeric`,
`reject`, `ereject` and `index`. Required capabilities must precede executable
statements. Unknown commands/tags/capabilities are refused, including in dead
branches. `date`/`currentdate` are deferred and not advertised.

```sieve
require ["fileinto", "mailbox", "imap4flags", "body"];
if header :is "X-Mixos-Spam-Verdict" "SPAM" {
    fileinto :create "Spam";
    stop;
}
if body :text :contains "release announcement" {
    fileinto :create :flags "\\Flagged" "Releases";
}
```

`fileinto` uses maild mailbox names. Missing destinations without `:create`
keep the message at its default destination. Flags map to the same MDS flags
and validated user keywords as IMAP/JMAP. File actions snapshot the current
flags. `:copy` preserves implicit keep. Variables support substitution, wildcard
captures, case modifiers, wildcard quoting and length.

`body` uses decoded MIME text, strips HTML markup for `:text`, selects MIME types
with `:content`, and exposes the untransformed body with `:raw`. Relational
`:value`/`:count` support `gt`, `ge`, `lt`, `le`, `eq`, `ne`; `:count` requires
`i;ascii-numeric`. Header `:index N` selects one occurrence, with `:last`
counting from the end.

## Delivery and policy

SMTP classifies spam, runs the global Mix `inbound_filter`, then runs account
Sieve. This preserves the existing filter's spam inputs. The Mix destination,
or classifier Inbox/Junk default, is Sieve's keep destination. The stored message
has daemon-generated `X-Mixos-Spam-Verdict` and `X-Mixos-Spam-Score` headers;
incoming copies are removed. Verdicts are `HAM`, `SPAM` or `UNKNOWN` when
classification is disabled or fails. JMAP/native Bus submissions also run
recipient Sieve on local delivery, with Inbox as keep, without rerunning the
inbound classifier or global Mix filter.

Evaluation produces a complete action plan. Syntax/runtime errors, invalid flags,
missing destinations and refused redirects cancel the plan and keep the message.
Limits: 64 KiB per script, 10,000 evaluation/matching/allocation steps, 64
filing/redirect actions and four redirects. Local redirects run the destination's
script; the four-hop header also bounds remote loops. A forged high hop count
can suppress forwarding but never grants extra hops.

`reject`/`ereject` return SMTP `550 5.7.1` after DATA for a single normal
recipient. Mixed-recipient/asynchronous delivery instead queues a DSN with a
null reverse path. DSNs to null/unverified senders are suppressed to prevent
backscatter. Combining reject with filing, redirect, discard or vacation is a
runtime error and keeps the original message.

Off-site redirects default to refused. `sieve_external_domains` lists exact
account domains allowed to redirect to non-local addresses. It also enables
those domains' Sieve vacation replies to non-local senders. This is operator
configuration, never script data. Local destinations must be account addresses.

Vacation supports `:days` (1..31, default 7), `:addresses`, `:subject`, `:from`
and `:handle`. `:from` must be the account address. `:addresses` recognises
additional To/Cc recipient identities; it does not authorise sender spoofing.
Replies require verified ingress and a recognised To/Cc identity. Spam, null
senders, self-mail, list/bulk mail and automatic responses receive no reply.
A durable atomic sender/response-handle stamp permits one reply per interval
across concurrent deliveries and restarts. Replies use a null reverse path,
`Auto-Submitted: auto-replied`, and the native outbound queue. Its worker
selects the account domain's DKIM signer; absent signing configuration retains
the existing unsigned behaviour. An active Sieve script takes ownership of
automatic responses; the older JMAP VacationResponse runs only without an
active Sieve script, preventing two automatic replies to the same message.

## ManageSieve

Disabled by default. In the maild configuration:

```mix
sieve_listen: ["192.0.2.10:4190"]
sieve_implicit_tls: false
sieve_external_domains: []
```

`false` selects RFC 5804 STARTTLS, the normal mode on 4190; `true` selects
implicit TLS. Both share SMTP/IMAP's TLS identities, SNI resolver and hot-reload
slot. Enabling the listener without TLS material fails startup. Port 4190 needs
no new unit privileges. NodeConfig fallback leaves this optional listener off;
set the keys in the maild configuration used by the daemon.

Commands: `CAPABILITY`, `AUTHENTICATE` PLAIN/LOGIN, `LISTSCRIPTS`, `GETSCRIPT`,
`PUTSCRIPT`, `CHECKSCRIPT`, `SETACTIVE`, `DELETESCRIPT`, `RENAMESCRIPT`,
`HAVESPACE`, `NOOP`, `LOGOUT`. Authentication and script access require TLS.
Passwords use the shared account verifier. PLAIN authorisation identity must
be empty or equal the login identity. Only the authenticated account's scripts
are accessible. Uploads accept quoted strings and non-synchronising octet
literals (`{N+}`). Malformed/oversized frames close the session. Limits: 128
connections, 180-second command timeout, 30-second TLS/authentication timeout,
three failed logins. STARTTLS discards buffered plaintext commands.

## Bus editing and versions

Like maild's existing operator commands, broker callers select an account;
ManageSieve instead enforces authenticated account isolation.

| Command | Request | Response |
| --- | --- | --- |
| `maild.sieve.list` | `account` | `scripts`: name, version, active, size |
| `maild.sieve.get` | `account`, `name` | name, version, active, source |
| `maild.sieve.put` | `account`, `name`, `source`, optional `expected_version` | new version |
| `maild.sieve.activate` | `account`, `name` (empty disables) | active name |

Account identifiers are email addresses. PUT's optional version guard uses zero
for a new script and refuses stale writes. Replacements preserve active state.
`sieve_scripts` stores current source; `sieve_versions` retains the latest 16
revisions per name. Versions are never reused after deletion. ManageSieve uses
the RFC's last-writer-wins editing semantics. Account deletion cascades to all
script/version/vacation-stamp rows.
