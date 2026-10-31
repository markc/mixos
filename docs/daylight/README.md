---
title: Daylight
description: How daylight writes the sun's state from the time zone, so appearance mode auto turns light at sunrise and dark at sunset on every desk.
---

# Daylight

`daylight` is the sun's clock for `appearance.mode = auto`. It works out the
next sunrise or sunset for where the machine is and, when one passes, writes
`state.appearance.daylight` into settingsd. settingsd's resolver turns `auto`
into `light` or `dark` from that key, so every desk, app and desktop exporter
follows the change by the normal path: one revision, one receipt.

- **Where.** The place comes from `time.zone`. While that key is unset, it
  comes from the zone the host's `/etc/localtime` names. A table in
  `<prefix>/share/daylight/zones.mx` gives each zone its principal place, the
  city the IANA time zone database names it after (tzdb chooses the region's
  most populous city). There is no GPS, no network lookup and no user-set
  location. A zone with no place (`UTC`, `Etc/GMT+10`) gives the sun as unknown
  (`null`), and `auto` then resolves to dark.
- **When.** The standard sunrise equation, with the sun's centre at 0.833
  degrees below the horizon, which allows for refraction and the solar disc.
  It is within a minute or two of published times away from the poles. Inside
  the polar circles, a polar day is light and a polar night is dark; the day a
  polar day or night begins, the change is taken at that day's solar midnight,
  so the state still alternates sunrise, sunset across the boundary. The next
  event daylight waits for is always the one that changes the state: a sunset
  while it is light, a sunrise while it is dark.
- **A manual flip holds until the next event.** Choosing Light or Dark while
  the mode is `auto` sets `state.appearance.mode_override`. daylight clears it
  in the same revision that next changes `state.appearance.daylight`, so the
  override lasts until the next sunrise or sunset. A zone change that changes
  the sun's state clears it the same way, including a change to a zone with no
  place (daylight becomes `null`).

Until daylight has written its key, Preferences shows the Auto choice
disabled, naming `daylight.mix`.

## Running it

daylight runs once per machine, beside the settings authority, never beside a
mirror: a desk's mirror relays the authority's revisions, so every desk on
the machine flips together. Before its first write it asks
`settings.status`; beside a mirror (any role but `authority`) it logs why and
exits 0, writing nothing. It is a Mix Bus citizen; the user unit runs it in
the authority's session:

```text
$ systemctl --user enable --now mixos-daylight.service
```

The unit is installed as `<prefix>/share/daylight/units/daylight.service`;
copy or link it into `~/.config/systemd/user/` as `mixos-daylight.service`.

It binds `{instance: <node>, profile: "default"}`, the node being the native
node identity (`MIXOS_NODE_CONFIG`). The environment can change that and the
rest:

| Variable | Default | What |
|---|---|---|
| `MIXOS_DAYLIGHT_INSTANCE` | the node name | the settings instance |
| `MIXOS_DAYLIGHT_SETTINGSD` | `settingsd` | the settingsd service to call |
| `MIXOS_DAYLIGHT_ZONES` | `<share>/daylight/zones.mx` | the zone table |
| `MIXOS_DAYLIGHT_LOCALTIME` | `/etc/localtime` | the link that names the host's zone |
| `MIXOS_DAYLIGHT_RESUME` | `on` | `off` stops the login1 watch |

## Nothing polls

daylight arms one local timer, for the next sunrise or sunset, and works the
sun out again only when something changes:

- at start, and on each broker connection;
- on every revision of the `time` keys (the retained
  `settingsd.desktop.keys.default.time` topic), taken only for its own binding
  and only when the revision is newer than the last one seen;
- when the timer fires;
- when `/etc/localtime` is replaced (an inotify watch on its directory);
- on resume from suspend: logind's `PrepareForSleep(false)` signal, heard
  through `gdbus monitor` on the system bus. The timer runs on the monotonic
  clock, which stops during suspend, so resume is when it must be re-armed.
  If the watch exits, it is restarted, at most once a minute.

When no sunrise or sunset falls in the four solar days it looks at (a long
polar day or night), or the zone has no place, it looks again in a day.

It writes only when the sun's state differs from the stored one, fenced on
the revision it read; when another commit lands in between it reads again,
up to three times.

Mix has no way to cancel a sleeping task, so one wake task does the
waiting: a later deadline is taken up by the task already sleeping when it
wakes, and only an earlier one starts a new task (the old one finds itself
superseded and ends).

## Commands

| Command | Answers |
|---|---|
| `daylight.status` | `{binding, role, zone, zone_from, place, daylight, next, next_kind, due, tasks_started, writes, last_reason, last_error, resume, resumes, resume_restarts, localtime_watch, clock_offset}`: `zone_from` is `time.zone` or `localtime`; `next` the Unix time of the next event and `due` the armed wake; `resume` is `login1`, `restarting`, `unavailable`, `lost` or `off`; `clock_offset` is 0 outside tests |

```text
$ mix -c 'send daylight daylight.status; print($result)'
```

## Limits

- A wall-clock step (an NTP correction at boot, a manual clock change) is not
  heard: the timer then fires late or early by the step. A late timer still
  writes the right state when it fires, and an early one finds nothing to
  change and re-arms.
- Where a desk has no system bus, or no `gdbus`, resume is not heard and the
  first event after a resume is late by the time spent suspended.
  `daylight.status` reports it as `resume: "unavailable"`.

## Testing

`services/daylight/tests/daylight_test.mix` runs the maths, the zone table,
the citizen against a real settingsd on a private Bus, and against a fake one
(`tests/fixtures/fake_settingsd.mix`) for a mirror's role and for conflicts.
With `MIXOS_TEST=1` only, the citizen honours three test hooks:
`MIXOS_DAYLIGHT_CLOCK_OFFSET` (seconds added to the wall clock; it must be a
number and is clamped to seven days either way), `MIXOS_DAYLIGHT_RESUME_ARGV`
(a JSON argv run instead of `gdbus monitor`) and `MIXOS_DAYLIGHT_RESUME_GUARD`
(seconds between watch restarts).

## The zone table

`share/daylight/zones.mx` is generated, not written by hand:

```text
$ mix services/daylight/tools/zones_from_tzdb.mix [--tzdata /usr/share/zoneinfo] [--check]
```

It takes each zone's coordinates from tzdb's `zone.tab` (one row per country
and zone), then `zone1970.tab` for any zone that lacks them, then gives every
`tzdata.zi` link (such as `Australia/Queensland`) its target's place,
repeating until no link gains one. The time zone database is in the public
domain; its licence is in `share/daylight/LICENSE-tzdb.txt` and
`LICENSES/LicenseRef-Public-Domain.txt`, and the table is listed in
`tools/assets.conf.mix`.
