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

## Not established

Whether `ff` means "inherit a default" and where that default lives. The
meaning of the individual bits in `0x0381`, and of the `0x0010` on even steps.
Anything above step 16 or beyond the four lanes. p-lock allocation. The
tempo, swing, length and micro conversions. No write path or firmware allowlist
entry exists for this box, and none is proposed.

## One loose end, recorded rather than explained

Before the live slot was found, a note edit on the box changed exactly one byte
in `0x65` — 24035, `ff → 06`, inside the kit region — while `0x60` and `0x61`
did not move. A second note edit of one semitone changed nothing at all, and the
no-edit control changed nothing. One byte moving once and then not again is not
enough to call it anything, and it is written down here only so the next session
does not re-derive it as new.
