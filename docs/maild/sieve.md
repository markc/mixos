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

Extensions: `fileinto`, `mailbox` (`fileinto :create` and account-scoped
`mailboxexists` for writable mailboxes), `envelope`, `imap4flags`,
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

`fileinto` uses slash-separated mailbox paths; `INBOX` resolves by role.
`:create` creates missing parents. Non-writable destinations are treated as
missing, consistently with `mailboxexists`. Missing destinations without `:create`
keep the message at its default destination. Flags map to the same MDS flags
and validated user keywords as IMAP/JMAP. File actions snapshot the current
flags; repeated filings into one mailbox use the last flag list. Invalid,
unsupported and excess native keywords are ignored. `:copy` preserves implicit keep. Variables support substitution, wildcard
captures, case modifiers, wildcard quoting and length.

`body` uses decoded MIME text, strips HTML markup for `:text`, selects MIME types
with `:content`, and exposes the untransformed body with `:raw`. MIME selectors
support variables, and selected binary parts retain their decoded octets.
Body wildcards preserve earlier match captures. Relational
`:value`/`:count` support `gt`, `ge`, `lt`, `le`, `eq`, `ne`; `:count` requires
`i;ascii-numeric`. Header `:index N` selects one occurrence, with `:last`
counting from the end.
`address` parses every field named by the script, including custom address
headers. ASCII casemap ordering folds letters to uppercase before comparison.

## Delivery and policy

SMTP classifies spam, runs the global Mix `inbound_filter`, then runs account
Sieve. This preserves the existing filter's spam inputs. The Mix destination,
or classifier Inbox/Junk default, is Sieve's keep destination. The stored message
has daemon-generated `X-Mixos-Spam-Verdict` and `X-Mixos-Spam-Score` headers;
incoming copies are removed. Verdicts are `HAM`, `SPAM` or `UNKNOWN` when
classification is disabled or fails. JMAP/native Bus submissions also run
recipient Sieve on local delivery, with Inbox as keep, without rerunning the
inbound classifier or global Mix filter. These local copies receive fresh
`UNKNOWN`/zero spam headers before the recipient's script runs.

Evaluation produces a complete action plan. Syntax/runtime errors,
missing destinations and refused redirects cancel the plan and keep the message.
Limits: 64 KiB per script, 10,000 evaluation/matching/allocation steps, 64
filing/redirect actions, 1 MiB of variables and four redirects. Matching charges
for scanned bytes as well as instructions. Header parsing is charged once for
the header section; attachments do not spend the header-test budget. Body
transforms charge only selected parts. Filing and `mailboxexists` read at
most 10,000 metadata records and 1 MiB of names/attributes, omitting message
counts. Their path processing also has a 10,000-visit, 1 MiB budget; exhaustion
keeps the message. Local redirects run the destination's
script; SMTP redirects also rerun the destination's classifier and global filter
using the original verified ingress. Local hop counts stay out of band; remote
loops are bounded by this host's own `Received` trace lines. A hop/action limit
drops only excess redirects, retaining other actions and restoring implicit keep
when the suppressed redirect was the only delivery action. External SMTP ingress
strips `X-Mixos-Sieve-Hop`; that field never supplies a trusted hop count.
Off-site redirects add a `Received` trace and strip the private hop marker and
all `X-Mixos-Spam-*` fields, including folded continuations.

Once a filing or redirect has committed, later secondary-action I/O failures are
logged and the accepted delivery is retained. This follows maild's existing
partial-delivery behaviour and avoids retrying an already accepted copy.
Secondary actions have no durable replay across the separate MDS/mail.db stores.

`reject`/`ereject` return SMTP `550 5.7.1` after DATA for a single normal
recipient. Mixed-recipient/asynchronous delivery instead queues a DSN with a
null reverse path. DSNs to null/unverified senders are suppressed to prevent
backscatter. Discard is compatible with reject. Combining reject with filing, redirect or vacation is a
runtime error and keeps the original message.

Off-site redirects default to refused. `sieve_external_domains` lists exact
account domains allowed to redirect to non-local addresses. Vacation replies to
external senders are independently allowed by default; set
`sieve_vacation_external: false` to suppress them. These are operator
configuration, never script data. Local destinations must be account addresses.

Vacation supports `:days` (1..31, default 7), `:addresses`, `:subject`, `:from`
`:handle` and `:mime` entities. `:from` must be the account address. `:addresses` recognises
additional recipient identities; it does not authorise sender spoofing.
Recognised identities include the account and final envelope recipient, and must
appear in To/Cc/Bcc or Resent-To/Resent-Cc/Resent-Bcc. Replies require verified
ingress. Spam, null
senders, self-mail, list/bulk mail and automatic responses receive no reply.
A sender/response-handle stamp and its native queue row commit in one mail.db
transaction, permitting one reply per interval across concurrent deliveries and
restarts. Handles are stored as fixed-size hashes. Tracking retains at most
10,000 pairs per account and drops stamps past the maximum 31-day interval.
At capacity, new pairs receive no automatic reply until space expires; live
suppression records are preserved so existing senders cannot receive repeats.
Replies use a null reverse path,
`Auto-Submitted: auto-replied`, and the native outbound queue. Its worker
selects the account domain's DKIM signer; absent signing configuration retains
the existing unsigned behaviour. An active Sieve script takes ownership of
automatic responses; the older JMAP VacationResponse runs only without an
active Sieve script, preventing two automatic replies to the same message.
Without `:subject`, replies use `Auto: ` followed by the decoded original
subject, or `Automated reply` if it is absent. Non-ASCII subjects are emitted
as encoded words; ASCII subjects remain plain text.

## ManageSieve

Disabled by default. In the maild configuration:

```mix
sieve_listen: ["192.0.2.10:4190"]
sieve_implicit_tls: false
sieve_external_domains: []
sieve_vacation_external: true
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
literals (`{N+}`). When external vacation replies are disabled, valid PUTSCRIPT
and CHECKSCRIPT commands containing a vacation action return
`OK (WARNINGS) "vacation replies to external senders are disabled"`; scripts
are still accepted. An oversized PUTSCRIPT returns `NO (QUOTA/MAXSIZE)` and
closes the session without reading its non-synchronising literal. Other
malformed/oversized frames close the session. Limits: 128
connections, 180-second pre-authentication and 30-minute authenticated command
timeouts, 30-second TLS/authentication and response-write timeouts,
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
`sieve_scripts` stores current source; `sieve_versions` retains up to 16
revisions per name and 512 per account, including deleted/renamed names.
Versions increase across the account and are never reused after deletion. ManageSieve uses
the RFC's last-writer-wins editing semantics. Account deletion cascades to all
script/version/vacation-stamp rows.
