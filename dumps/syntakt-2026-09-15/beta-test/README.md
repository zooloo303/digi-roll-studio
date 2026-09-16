# The 0.5.5-beta.1 hardware test

Fetched through the beta's own IN panel on 2026-09-15, then read again with
`examples/syntakt_probe` to find the cause of anything that did not match the
box. Syntakt OS 1.40 build 0082, USB, macOS 15.7.9 on Apple Silicon.

**B01 was programmed before it was fetched**, so every field had an expected
answer first:

| where | programmed |
|---|---|
| pattern | length 32, swing 58% |
| T1 step 1 | a trig, nothing locked |
| T1 step 5 | E5, velocity 80 |
| T1 step 9 | length 1/32, micro −1/48 |
| T1 step 13 | condition 1:2 |
| T2 step 3 | condition 50% |
| T2 step 7 | condition PRE, micro −1/48 |
| T2 step 11 | a trig with FLTR RESO locked |

Every one of those arrived as programmed. Three things did not match the box,
and each has a test over these captures.

## 1. A lane on the box was never mentioned

The pool holds one record: paramId 29, T2, step 11, value 126. The import's
`plock_lanes_not_carried` was declared and shown and never assigned, so the
panel's "stays on the box" line could not appear.

## 2. Residue was reported as a trig

`B01-known-values.bin`, T1 step 11, reads `03 80`: trig bits set, note bit
clear. **Nothing is lit there on the box**, and T1 has no pool record. The
import counted any non-empty word that plays no note as a lock trig, and
reported "1 trig sounds no note". Lock trigs are now counted from the pool.

What this box writes for a *real* lock trig is still not measured.

## 3. Trigs past the length were imported

`A02-trigs-past-length.bin`: pattern length 16, and the box confirmed T12's
own length is 16. T12 plays 8 trigs — steps 2, 3, 7, 10, 11, 12, 15, 16 — and
the payload holds the same shape again at 17–32, 33–48 and 49–64. All 32 were
imported; the box plays 8.

The fix has two halves, and the second matters more than the first. The import
leaves those 24 out of the roll. **A write then leaves them on the box**: until
this, it encoded all 64 steps of a track and would have cleared every trig
past the length that nobody had been shown.

## Also checked, and matched

A03 (8 trigs on T2, swing 70) and A09 (7 trigs on T8, swing 75), by trig count
and swing against the box.
