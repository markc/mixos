---
title: email
layout: page
---

# email — the MixOS JMAP mail client

`email` (`dev.mixos.email`) is the reserved MixOS mail client: a **pure
JMAP client** speaking HTTPS to any RFC 8620/8621-shaped server — the
mesh's own maild, or anyone else's server. Mail is listed, read, composed
and sent; calendars and address books are listed and shown.

## Running it

```
email [--noded-url URL] [--service NAME]
```

A second launch raises the first window. The first run shows the account
form:

| Field | What it is |
|---|---|
| Server URL | The JMAP server base (`https://…`). maild directly, or a webd vhost that proxies `/jmap`. |
| Email address | The login; the Basic username at the token exchange. |
| Password | Spent once per token lifetime; held in the config file (mode 0600) afterwards. |
| CA bundle path | Optional. A PEM bundle to trust for a server whose certificate is self-issued. |

The account persists in the app's config directory
(`~/.local/state/mixos/apps/email/config/config.conf.mix`); a malformed
file loads defaults and is never overwritten.

## What it does

- **Mail** — mailboxes with unread counts; the newest 100 emails per
  mailbox; read in the reader pane (plain text first, HTML source when
  there is no plain text); opening an email marks it read; compose (To, Cc,
  Bcc, Subject, body) and reply, sent through the proven recipe: build the
  message, upload it, create the Email in Sent, submit.
- **Calendar** — calendars, their events by start, the open event's
  detail.
- **Contacts** — address books, contacts with a server-side text filter
  over name, email and company, the open contact's detail.

## How it talks to the server

One HTTPS session per account. On connect: the password is exchanged for a
Bearer token (`POST /auth/tokens/issue`) and the Session is discovered
(`GET /.well-known/jmap`); every method call after that carries the token.
A server without the token endpoint falls back to Basic auth on every
request. Requests batch their methodCalls and chain `Email/query` into
`Email/get` through a ResultReference — one round trip per list.

The engine (`apps/email/crates/jmap`) is ported from the retired Dioxus
mail client, with its faults fixed: session discovery and URL templates
instead of hardcoded paths, real pagination, Bcc carried through,
`In-Reply-To` wrapped exactly once, CR/LF stripped from header inputs
(header injection), typed method-error handling, and attachment download
wired (the UI for it is a later slice).

## Keyboard

| Shortcut | Action |
|---|---|
| Ctrl+R | Refresh |
| Ctrl+Q | Quit |
| Ctrl+N | Compose |
| Ctrl+Shift+R | Reply |
| Ctrl+1 / 2 / 3 | Mail / Calendar / Contacts |

## Performance workload

The recorded workload for future profiling: a 10,000-email mailbox — the
list renders 100 rows from one query+get round trip, and the reader fetches
one email's bodies on open. Nothing else scales with mailbox size.
