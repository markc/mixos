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
  the polar circles, a polar day is light and a polar night is dark.
- **A manual flip holds until the next event.** Choosing Light or Dark while
  the mode is `auto` sets `state.appearance.mode_override`. daylight clears it
  in the same revision that next changes `state.appearance.daylight`, so the
  override lasts until the next sunrise or sunset.

Until daylight has written its key, Preferences shows the Auto choice
disabled, naming `daylight.mix`.

## Running it

daylight runs once per machine, beside the settings authority, never beside a
mirror: a desk's mirror relays the authority's revisions, so every desk on
the machine flips together. It is a Mix Bus citizen; the user unit runs it in
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
- on every revision of the `time` or `state` keys (the retained
  `settingsd.desktop.keys.default.time` and `.state` topics);
- when the timer fires;
- when `/etc/localtime` is replaced (an inotify watch on its directory);
- on resume from suspend: logind's `PrepareForSleep(false)` signal, heard
  through `gdbus monitor` on the system bus. The timer runs on the monotonic
  clock, which stops during suspend, so resume is when it must be re-armed.

When there is no event in the next two days (a polar day or night), it looks
again in a day.

It writes only when the sun's state differs from the stored one, so its own
write, coming back on the `state` topic, finds nothing to do.

## Commands

| Command | Answers |
|---|---|
| `daylight.status` | `{binding, zone, zone_from, place, daylight, next, next_kind, writes, last_reason, last_error, resume, localtime_watch}`: `zone_from` is `time.zone` or `localtime`, `next` the Unix time of the armed event, `resume` is `login1`, `unavailable`, `lost` or `off` |

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

## The zone table

`share/daylight/zones.mx` is generated, not written by hand:

```text
$ mix services/daylight/tools/zones_from_tzdb.mix [--tzdata /usr/share/zoneinfo] [--check]
```

It takes each zone's coordinates from tzdb's `zone.tab` (one row per country
and zone), then `zone1970.tab` for any zone that lacks them, then gives every
`tzdata.zi` link (such as `Australia/Queensland`) its target's place. The time
zone database is in the public domain; its licence is in
`share/daylight/LICENSE-tzdb.txt`, and the table is listed in
`tools/assets.conf.mix`.
