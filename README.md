# VIZIO

VIZIO SmartCast TVs, over the local HTTPS API built into the set — the same one the SmartCast
Mobile app talks to, on port 7345 (older firmware: 9000). Self-signed certificate; core accepts
it for the same reason it accepts every other LAN bridge's — see `Tls::LOCAL` in `core`.

One box, two things it does — same split as Roku and Hisense: "watch" goes to the
`media_player`, volume and input go to the `tv`. Unlike either of those, power and input really
are two distinct commands here rather than one key doubling as both, and pairing is not
optional — nothing works until it has happened.

## Setup

Asks for the address, sends it a pairing request, and shows a form for the 4-digit code the TV
puts on screen. No discovery yet — see the note in `manifests/vizio.tv.toml`.

## Switching inputs

Two things about this are not what the reference client (`pyvizio`, which Home Assistant's
`vizio` integration wraps) does, and both were found against a real V505-H19:

- The write takes the input's **`CNAME`** (`hdmi2`), not its display **`NAME`** (`HDMI-2`).
  A set answers `FAILURE` for the latter and does nothing. This looks likely to be the real
  cause of the long-running "cannot change input" reports against those projects.
- The **`HASHVAL` is invalidated by the switch it authorises**, so it cannot be cached: a
  switch is a read for a fresh one and then a write that spends it. Reusing one works exactly
  once and then silently stops.

Both are load-bearing, both are covered by tests, and both read like typos. Please do not
"correct" either back toward what the reference client sends.

## No app launching

SmartCast's local API has no endpoint that lists installed apps — VIZIO's own remote gets that
list from a cloud catalog, keyed by app ids this driver has no verified copy of. Guessing would
mean `launch_app` sometimes opening nothing at all, so it is left out rather than shipped wrong.

## Building

```bash
cargo build --release
```

Releases are built by [`junohouse/driver-ci`](https://github.com/junohouse/driver-ci): push to
`main` for a beta, tag `v1.2.0` for a release. To work on this against a local core checkout,
uncomment the `[patch]` block in `Cargo.toml`.
