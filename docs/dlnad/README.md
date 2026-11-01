---
title: DLNA media server
description: How dlnad serves a media directory to TVs and other DLNA renderers on one LAN interface, and how to drive it from the Bus.
---

# DLNA media server

`dlnad` puts a folder of films, series and music on your TV. It is a DLNA /
UPnP AV media server: the TV finds it on the network, browses the folder and
plays the files, seeking as it likes. There is no library to build or
rescan. Every browse lists the directory as it is now, so a file you drop in
shows up the next time the TV looks.

It is a [Bus citizen](../mix/serve.md) written in Mix (`mix --serve`), so the
same service answers the TV over HTTP and SSDP and answers people and agents
on the Bus.

## What it serves

- **One media directory**, browsed live. Folders become browse levels;
  dotfiles are hidden; files are shown when their extension is a known video,
  audio or image type.
- **Subtitles**: a video's sibling `.srt` with the same name is offered to the
  renderer (the `sec:CaptionInfoEx` element and `CaptionInfo.sec` header that
  Samsung and LG sets read).
- **Byte-range streaming**, so renderers seek and resume. There is no
  transcoding: the files go out as they are, which suits renderers that play
  MKV, MP4 and HEVC natively.

## Discovery

Renderers find a server by SSDP on UDP 239.255.255.250:1900. dlnad joins that
group on the interface of its `listen` address, answers searches, and sends
`ssdp:alive` adverts every `announce_s` seconds, with `ssdp:byebye` before an
identity change. On a host with several networks, `listen` is what puts the
server on the network your TV is on.

Some TVs never join the multicast group and miss multicast while they doze,
so they forget a server between adverts. List their addresses in `renderers`
and every advert is also sent to them unicast, which their Wi-Fi delivers
reliably.

## Configuration

`<etc>/dlnad/dlnad.conf.mix` (normally `/etc/mixos/dlnad/dlnad.conf.mix`):

```mix
{
  media_root: "/srv/media",
  listen: "192.0.2.30",
  port: 8200,
  name: "MixOS Media",
  ssdp: true,
  renderers: ["192.0.2.212"],
  announce_s: 60,
  max_age: 1800
}
```

| Key | Default | Meaning |
|---|---|---|
| `media_root` | (required) | the directory to serve |
| `listen` | (required) | the LAN IPv4 renderers reach; HTTP binds it, SSDP joins on it |
| `port` | `8200` | HTTP port |
| `name` | `MixOS Media` | friendly name shown on the TV |
| `ssdp` | `true` | announce and answer searches (needs mix 0.118.0 or later) |
| `renderers` | `[]` | IPv4 addresses that also get every advert unicast |
| `announce_s` | `60` | seconds between alive adverts |
| `max_age` | `1800` | how long a renderer may cache an advert |
| `max_conns` | `64` | HTTP connections held at once; when full, the oldest idle one is dropped |

The server's identity (its UPnP uuid and friendly name) lives in
`<var>/dlnad/identity.conf.mix` and survives restarts. A configuration change
needs a service restart.

## Bus commands

| Command | Body | Reply |
|---|---|---|
| `dlnad.status` | — | identity, configuration, SSDP state, counters |
| `dlnad.list` | `{id, start, count}` | one container's children as data (`id` `"0"` is the top) |
| `dlnad.renderers` | — | clients seen over HTTP or searching by SSDP, with their user agent |
| `dlnad.announce` | — | sends the alive adverts now |
| `dlnad.identity.rotate` | `{keep_name}` | byebye, a new uuid (and name), alive |

```mix
send dlnad dlnad.status
print($reply.name .. " at " .. $reply.location)
```

**When the TV shows stale content or an old server**, rotate the identity.
Many TVs cache a server's browse tree under its uuid; a new uuid is a server
they have never seen. With the default the name changes too (`MixOS Media
1A2B`), so the fresh entry is easy to pick; `{keep_name: true}` keeps the
name, and the TV may need a restart to notice.

## Running it

The unit is `dlnad.service` (a dynamic user, state under
`/var/lib/mixos/dlnad`). The media directory must be readable by that user:
world-readable, or add its group with a drop-in (`SupplementaryGroups=`).
Install the component with `mix tools/component_install.mix dlnad`.

## Limits

- One media directory, IPv4 only, HTTP (no TLS: DLNA renderers do not speak it).
- No transcoding, thumbnails or metadata scraping; titles are file names.
- UPnP eventing is accepted and never fired; renderers re-browse on the
  container update id, which changes when the top folder changes.
- The SSDP and HTTP event commands (`dlnad.http.*`, `dlnad.ssdp`) are visible
  on the Bus like any `on` handler; they act only on sockets the running
  service owns.
