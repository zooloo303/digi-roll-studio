# Credits and third-party notices

digi-roll-studio as a whole is licensed under the GNU General Public License
version 3 or later — see `LICENSE`. Parts of it derive from other people's work
under permissive terms, and those terms are reproduced below. The GPL does not
displace them: the files concerned keep their original notices, and this file
carries the license text those notices require.

---

## elk-herd — the reason the protocol layer exists

**elk-herd by Mark Lentczner ("mzero")**, BSD-2-Clause.

The upstream repository at `github.com/mzero/elk-herd` was withdrawn by its
author in 2026. The attribution below is taken from the `LICENSE.txt` of the
`elk-herd-main` source archive (retrieved 2026-08-26), and is reproduced
verbatim — BSD-2-Clause clause 1 requires the notice to travel with the source,
and a dead URL does not discharge that.

elk-herd is the de-facto public documentation of Elektron's SysEx protocol.
Without it there is no byte-level knowledge to port and no project here. The
following in `crates/protocol` and `crates/midi` is derived from it:

| Here | From elk-herd |
|---|---|
| `protocol/src/sevenbit.rs` | `src/ByteArray/SevenBit.elm` — the seven-bit packing format |
| `protocol/src/protocol.rs` | `src/SysEx/SysEx.elm`, `src/SysEx/Dump.elm`, `src/SysEx/ApiUtil.elm` — API and dump framing, checksum14 |
| `protocol/src/pattern.rs` | `Elektron/Digitakt/{Dump,CppStructs}.elm` — the DT2 pattern/track/kit struct skeleton |
| `protocol/src/device.rs` | `src/SysEx.elm`, `src/Elektron/Instrument.elm`, `src/Project/Update.elm` — the identity handshake's payload format |
| `midi/src/device.rs` | the dump request/response conventions the handshake and fetch paths follow |

What is **not** from elk-herd, and is digi-roll's own reverse engineering
against real hardware: the note-trig record pool, the trig-condition and p-lock
byte lanes, the pattern-settings block, and the entire Digitone II mapping.

Retaining this attribution is a project rule, not a courtesy — `PLAN.md` §7
rule 6.

### BSD-2-Clause

```
BSD 2-Clause License

Copyright (c) 2017 - 2025, Mark Lentczner

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

---

## digi-roll — the direct ancestor

**[digi-roll](https://github.com/zooloo303/digi-roll)** — a browser piano roll
for the same boxes, started by the same author as this project and with
contributions from others (see below). digi-roll is where the protocol work was
hardware-verified, and this repo is a port of it: the seven-bit
codec, the pattern structs, the byte lanes, `chords.js`, the generator and the
safe-write rules all came across from there. Where the two disagree about a byte,
digi-roll is right until hardware says otherwise.

---

## Ángel Linares García — DN1 support, and the +Drive file API

**Ángel Linares García**, who also builds **DNX**, a sibling project
independently reverse-engineering the Digitone family.

He is a contributor to digi-roll, the codebase this repo is a port of, and the
work below came across with the rest of it. This file did not name him until
2026-08-26; the public-history squash that created this repository left his
commits invisible here, which is a reason the omission went unnoticed and not an
excuse for it.

In digi-roll:

| What | Where it landed |
|---|---|
| Digitone 1 read-only support — decode, p-locks, fixtures, hardware test plan | `js/elektron/dn1/pattern.js`, `test/dn1.test.js`, five DN1 preset captures under `dumps/fixtures/` |
| Corrections to the device table, protocol framing, safe-write and trig-conditions | `js/elektron/{device,protocol,safe-write,trig-cond}.js` |
| **The Elektron +Drive file API, documented** | `docs/plus-drive-file-api.md` |

That last one is the load-bearing one for anything in this repo that reads a
+Drive. It is measured on real hardware and on a USB capture of Elektron's own
Transfer application — not inferred from elk-herd, which has no Digitone
support at all. It also corrected a claim digi-roll had been carrying: that the
DN2 has no +Drive file API. It does. The `50–5E` opcodes a DN2 advertises are a
**second, renumbered file API** on the API mechanism, not dump types and not a
gap — so under the API header `0x53` is *List*, while under a dump header the
same byte is a *Sound dump*. This project independently made the identical
mistake before reading his document, which is the best argument for the
document existing.

If the +Drive file API is implemented here, this credit travels with it.

---

## Dependencies

The build pulls its dependencies from crates.io under their own licenses,
predominantly MIT/Apache-2.0. The load-bearing ones:

- **[midir](https://github.com/Boddlnagg/midir)** — MIDI I/O over CoreMIDI, ALSA
  and WinMM. Chosen over `rtmidi` because those APIs ship with the OS, so there
  is no third-party C library to install.
- **[egui](https://github.com/emilk/egui) / eframe** — the entire UI.
- **serde** — the session file.

`cargo tree` is the authority on the full set; run `cargo license` or
`cargo about` if you need the complete manifest.

---

## A note on the copyright line above

The BSD-2-Clause text is reproduced here in full, as the license requires of
anyone redistributing the derived source. This file previously credited the
holder only as "mzero", the handle used throughout this codebase and throughout
digi-roll before it, and noted that elk-herd's own `LICENSE` would be canonical
if it stated a fuller name. It does: **Copyright (c) 2017 - 2025, Mark
Lentczner**. That is now the credit above, reproduced verbatim, and it is the
one to keep.
