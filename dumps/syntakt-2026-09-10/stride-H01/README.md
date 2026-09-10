# Syntakt track and step addressing — 2026-09-10

Two capture pairs against pattern **H01 (slot index 112)**, one trig each, on a
track that had nothing on it. Same box as the sweep in the parent folder:
product 30, build 0082, OS 1.40 — the OS the donated pairs were taken on.

`before.bin` → `trig7.bin` is a trig added at **track 7, step 5**.
`trig7.bin` → `trig16.bin` is a trig added at **track 7, step 16**.
`empty-A01.bin` is an untouched slot, for reference.

All are `0x61` payloads (pattern alone, 27136 bytes). Offsets are decimal.

## Finding the pattern the box was on

The first attempt read slot 0 and saw **nothing change** — not for a note edit,
not for a trig. Two dumps taken back to back with nothing touched were
byte-identical, so the reads were stable and the fault was not noise: the box
simply was not on A01. Sweeping `0x61` across slots 0–127 and comparing each to
an empty one found exactly one that differed by more than a few bytes, slot 112,
which is bank H pattern 1.

**A dump request cannot ask which pattern is loaded** — every reply here echoes
the index that was sent — so a sweep either side of one edit is the cheapest way
to find the live slot, and `syntakt_probe --indices` exists for that.

## The addressing, measured

    trig word   = 4 + 983×(track-1) + 2×(step-1)
    pitch       = 4 + 983×(track-1) + 128 + (step-1)
    velocity    = 4 + 983×(track-1) + 192 + (step-1)
    length      = 4 + 983×(track-1) + 256 + (step-1)
    micro       = 4 + 983×(track-1) + 320 + (step-1)

**Track stride 983 is confirmed as a track, not just a repeating block.** The
trig on track 7 landed in the seventh block, at 5910 — the block starting at
4 + 983×6 — and nowhere else in the dump moved.

**Step stride is 2 bytes, not 4.** The step-5 trig alone could not tell those
apart: it landed at block offset +8, which is step 5 at a 2-byte stride and step
3 at a 4-byte one, and the step-word table reads `0000 0010 0000 0010 0381 …`,
which looks like 4-byte groups if you want it to. Step 16 settles it — it landed
at **+30**, where a 4-byte stride would have put it at +60.

The four lane offsets fall out of the same arithmetic and reproduce the donated
track-1 step-1 recordings exactly (132, 196, 260, 324), so the 983-byte block is
128 bytes of step words — 64 steps × 2 — followed by 64-byte lanes.

## What a trig-on writes

It **sets bits rather than assigning a value**: `0x0381` is OR-ed into whatever
the step word held.

| step | before | after |
|---|---|---|
| 5 | `0000` | `0381` |
| 16 | `0010` | `0391` |

`0x0010 | 0x0381 = 0x0391`. An empty even-numbered step carries `0x0010` and an
empty odd-numbered one carries `0x0000`; what that bit means is **not
established**.

A freshly made trig reads `ff` in the pitch, velocity and length lanes and `00`
in micro — the same "still `ff` after trig creation" the donated notes recorded,
now seen on a different track and step.

## Pitch, and what `ff` means

`trig16.bin` → `note-lock.bin` is a **note lock** on the trig at track 7 step 5,
set by holding the trig and turning NOTE. The box displayed **D5 before and E5
after**. Exactly one byte moved:

    6034  ff -> 40    block 7 +132  =  pitch, step 5

which is the address the arithmetic above predicts, and `0x40` is 64 — **E5 in
the octave numbering the boxes use, where MIDI 60 is C5**. So the pitch lane
holds a raw MIDI note number, and this is the second lane the formula has been
checked against.

**`ff` means "no lock, take the track's default".** The evidence is that the box
displayed D5 for that step *while the lane byte was* `ff` — the note it played
came from somewhere else. And that somewhere is measured too:

| | block +960 | note |
|---|---|---|
| track 1 | `3d` | 61 = C#5 |
| track 7 | `3e` | 62 = D5 |

Track 7's default reads D5, which is exactly what the box showed for the
unlocked step. The earlier notes called `3c 64 0e` at 964–966 "candidate
defaults, never resolved into notes"; those are block offsets 960–962 of track
1, so **+960 is the track's default note**, in the same raw MIDI as the lane.

`+961` and `+962` are very likely the default velocity and length — they sit in
the lane order, and read `67 0d` on track 1 against `64 0e` on track 7 — but
**neither has been tested with an edit** and neither is claimed here.

## Length is the gen-2 scale, unchanged

Three locks on the same trig (track 7, step 5), each one edit apart:

| box showed | byte | `length_byte_to_steps` |
|---|---|---|
| 1/16 — the track default, read while the lane was `ff` | 14 | 1 step |
| 1/8 | 30 | 2 steps |
| 1/32 | 6 | 0.5 steps |

A step at SCALE 1x is a sixteenth, so all three land **exactly** on the scale
`protocol::pattern::length_byte_to_steps` already implements for the DT2 and
DN2 — including the part that makes it worth checking. That function is
piecewise: below 14 it is a flat 1/16-step ladder from 0.125, and from 14 up it
doubles every sixteen values. Two points inside one branch would have fitted
half a dozen curves; 1/32 was chosen to land in the *other* branch, and byte 6
is what it predicts.

So **the Syntakt's length encoding is the gen-2 one**, and the existing
conversion can be reused rather than re-derived. Files: `len-before.bin` →
`len-after.bin` → `len32.bin`.

## Micro-timing is a signed byte, 24 ticks to a step

Two locks on the same trig, opposite directions, same magnitude:

| box showed | byte | as signed |
|---|---|---|
| nothing (a fresh trig) | `00` | 0 |
| −1/48 | `f8` | −8 |
| +1/48 | `08` | +8 |

A bar is sixteen steps at SCALE 1x, so 1/48 of a bar is a third of a step, and a
third of a step reading 8 puts **24 ticks in a step** — the same resolution the
DT2 and DN2 use. Negative is earlier, positive is later, and the two directions
are symmetric.

**This is why the earlier notes warned that micro's `ff` "must not be treated as
the same sentinel as the other fields".** The pitch, velocity and length lanes
use `ff` for *no lock, take the track default*; micro has no such sentinel
because the byte is **signed**, and `ff` there is simply −1. A fresh trig reads
`00`, not `ff`, which is the same distinction seen from the other side.

Files: `micro-neg.bin`, `micro-pos.bin`.

## Velocity is raw, and so are the defaults

| box showed | byte |
|---|---|
| 100 — the track default, read while the lane was `ff` | 100 |
| 64 | 64 |

The velocity lane holds the displayed number, unscaled.

There was a false alarm on the way: an edit aimed at 1 came back as 5, which
looked like a non-linear mapping until the next point landed on 64 exactly. The
encoder had settled on 5, so **the byte was right and the intended value was
not**. A dump reports the box's state when it was asked, not what the screen
said a moment earlier; when a lane disagrees with what someone meant to set, the
lane is the more reliable witness.

That also finishes the defaults block. All three of `+960`, `+961` and `+962`
have now been read off the display of an unlocked step and matched:

| offset | track 7 | shows as |
|---|---|---|
| +960 | 62 | D5, the default note |
| +961 | 100 | the default velocity |
| +962 | 14 | 1/16, the default length |

## Every per-step field now decodes

| field | lane | encoding |
|---|---|---|
| pitch | +128 | raw MIDI note, `ff` = no lock |
| velocity | +192 | raw, `ff` = no lock |
| length | +256 | gen-2 `length_byte_to_steps`, `ff` = no lock |
| micro | +320 | signed byte, 24 ticks to a step, no sentinel |

Three of the four reuse the DT2/DN2 conversions unchanged.

## Tempo and swing

Both sit near the end of the pattern region. **Swing was set as the
pattern's swing**, stated by the person turning the knob, so that one is a
pattern-level field on the box's own terms and not merely by where it landed.
Tempo is recorded more carefully: it is *carried in the pattern dump*, which
is where it was read, and whether the box also keeps a project tempo that
overrides it was not tested.

**Tempo is BPM × 120**, at `23199` as a big-endian 32-bit value:

| box showed | value |
|---|---|
| 130.0 | 15600 |
| 100.0 | 12000 |

Only the low two bytes have ever moved — 65535/120 is 546 BPM, so the top half
has nothing to say — but the field is recorded as it was first measured, and
`23201` as a 16-bit value would read the same in every capture so far.
120 units to the BPM is a twelfth of the 0.1 the screen shows, which is what
makes the display's decimal place representable.

**Swing is the offset from straight, not the percentage**, one byte at `23207`:

| box showed | byte |
|---|---|
| 50% (straight) | 0 |
| 60% | 10 |

That is the digis' convention exactly — `pattern_settings` on a DT2 or DN2
stores `0` for 50% and `30` for 80%. So swing needs no conversion of its own
either.

**Pattern length is a raw step count**, one byte at `23204`. The donated pair
recorded `10 → 20`, which is 16 → 32; this capture reads `40`, and the pattern
is 64 steps.

Files: `tempo-130.bin` → `tempo-100.bin` → `swing-60.bin`.

## Not established

The meaning of the individual bits in `0x0381`, and of the `0x0010` an empty
even-numbered step carries. Anything above step 16, or beyond the four lanes and
the three defaults. What the remaining ~600 bytes of a track block hold. p-lock
allocation, which has never been observed on this box. The tempo and swing
conversions. Which of `0x60` and `0x65` is the stored slot and which the working
state — the two differ in a fixed 156 bytes and one note edit moved a single
byte in the `0x65` kit region, which is not enough to call it.

No decoder is implemented, no write path exists, and there is no firmware
allowlist entry for this box. Nothing here was written to the Syntakt: every
edit above was made by hand on the box itself.
