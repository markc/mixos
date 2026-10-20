---
title: Notifications
description: How notifyd keeps the desk's notifications, shows them as toasts and in the Notifications page, and tells an agent or app which button the human pressed.
---

# Notifications

`notifyd` is the desk's notification service. Anything on the Bus can post a
notification: an app saying a job is done, or an agent asking "is this okay?"
with Yes and No buttons. The desk shows it as a toast in the top right corner
and lists it on the Notifications page of the right-hand panel. When the
human presses a button, closes it, or lets it time out, notifyd records that
outcome and publishes it, so the sender finds out without anyone switching to
a dedicated app.

One notifyd runs per desk session (`notifyd@N`), under the Bus name `notify`.

## Posting a notification

```text
send notify notify.send body='{"summary":"Build finished","source":"cbc"}'
```

The reply comes back at once with the new notification's id:
`{"id": 7, "status": "open"}`. Nothing waits for the human.

To ask a question, add up to four buttons, and optionally a timeout:

```text
send notify notify.send body='{"summary":"Deploy to staging?","actions":[{"id":"yes","label":"Yes"},{"id":"no","label":"No"}],"timeout_ms":300000}'
```

## Hearing the answer

Every change is published on the topic `notify.changed`. The inner command
says what happened, and the body is the notification as `notify.get` shows
it:

| Inner command | When |
|---|---|
| `notification.created` | a notification was posted |
| `notification.answered` | the human pressed a button; `outcome.action` is its id |
| `notification.closed` | any other outcome: `dismissed`, `expired` or `closed` |

Subscribe before you send, so the answer cannot arrive before you are
listening, and match on the id the send returned. If you were disconnected,
`notify.get {id}` tells you the outcome afterwards.

An expired question was not answered. Treat it as "no answer", never as yes.

## Verbs

| Verb | Body | Reply |
|---|---|---|
| `notify.send` | `summary`, and optionally `body`, `app`, `source`, `actions` (`[{id, label}]`), `timeout_ms`, `request_id`, `thread` | `{id, status}` |
| `notify.get` | `{id}` | the notification, with `outcome` once it has one |
| `notify.list` | `{all?}` | open notifications, oldest first; `all: true` includes answered ones |
| `notify.action` | `{id, action}` | the human pressed a button (the toast calls this) |
| `notify.dismiss` | `{id}` | the human closed it without choosing |
| `notify.close` | `{id}` | the sender, or the Notifications page, withdrew it |
| `notify.close_all` | `{}` | closes every open notification without buttons; questions stay |

Each notification gets exactly one outcome. The first one wins, and a later
button press or timeout is refused with `ALREADY_ANSWERED`.

`notify.action`, `notify.dismiss`, `notify.close` and `notify.close_all`
settle a notification, so notifyd takes them only from callers on this node
(the broker's `broker_origin` stamp). A peer across the mesh can post and read
notifications but never answer or withdraw one for the person. That matters
for questions such as portald's screen-share consent.

Errors come back as `{error_code, message}`: rc 10 `USAGE` (a bad body),
rc 13 `FORBIDDEN` (not a local caller), rc 14 `NOT_FOUND`,
rc 15 `ALREADY_ANSWERED`, rc 16 `TOO_MANY_OPEN`.

## Limits

A summary is at most 120 characters, a body 500, and a button label 32. A
notification has at most four buttons. At most eight questions can wait for an
answer at once; the ninth is refused until one is answered. notifyd keeps the
last 200 notifications, and never drops one that is still open.

## State

Notifications are kept in one JSON file, rewritten atomically on every change:
`~/.local/state/mixos/notifyd/state.json`, or `$MIXOS_NOTIFYD_STATE`. After a
restart, open notifications stay open and their timeouts carry on; notifyd
never answers on the human's behalf. A state file it cannot read is moved
aside, never overwritten.
