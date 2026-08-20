# Credits and third-party notices

digi-roll-studio as a whole is licensed under the GNU General Public License
version 3 or later — see `LICENSE`. Parts of it derive from other people's work
under permissive terms, and those terms are reproduced below. The GPL does not
displace them: the files concerned keep their original notices, and this file
carries the license text those notices require.

---

## elk-herd — the reason the protocol layer exists

**[elk-herd](https://github.com/mzero/elk-herd) by mzero**, BSD-2-Clause.

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
Copyright (c) mzero

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY
OF SUCH DAMAGE.
```

---

## digi-roll — the direct ancestor

**[digi-roll](https://github.com/zooloo303/digi-roll)** — a browser piano roll
for the same boxes, by the same author as this project. digi-roll is where the
protocol work was hardware-verified, and this repo is a port of it: the seven-bit
codec, the pattern structs, the byte lanes, `chords.js`, the generator and the
safe-write rules all came across from there. Where the two disagree about a byte,
digi-roll is right until hardware says otherwise.

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
anyone redistributing the derived source. The holder is credited as it is
credited throughout this codebase and throughout digi-roll before it — "mzero".
If elk-herd's own `LICENSE` states a fuller legal name or a copyright year, that
notice is the canonical one and this file should be corrected to match it.
