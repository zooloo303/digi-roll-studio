# The 0.5.5-beta.2 hardware write test

Four writes through the beta's OUT panel, each followed by a restore from its
automatic backup. App 0.5.5-beta.2 (`c552d67`), macOS 15.7.9 on Apple Silicon,
Syntakt OS 1.40 build 0082, USB, all three slots on PER PATTERN scale.

Every file here is an unpacked `0x60` → `0x50` reply (31,744 bytes, pattern and
kit), read with `examples/syntakt_probe --only 60` independently of the app.

| file | read |
|---|---|
| `A02-baseline.bin`, `A03-baseline.bin`, `B01-baseline.bin` | before anything was written |
| `A03-after-write-round1.bin` | after round 1 |
| `B01-after-write-round2.bin` | after round 2 |
| `A02-after-write-round3.bin` | after round 3 |
| `B01-after-write-round4.bin` | after round 4 |

Not kept, because each was byte-identical to its baseline: the read after round
1's cancel, the read after each restore, and a final read of all three slots.
Each app backup was byte-identical to its baseline, and each pre-restore snapshot
was byte-identical to the after-write read.

B01 differs from `../beta-test/B01-known-values.bin` in the pool only. Between
the two tests it gained an empty record (paramId 29, T1, no values), and the T2
step 11 RESO lock moved to the second record.

## The rounds

| round | slot | edit | app result |
|---|---|---|---|
| 1 | A03 T2 | step 5 up a semitone; cancelled once first | verified |
| 2 | B01 T1 | step 5 E5 → F5 | verified |
| 3 | A02 T12 | step 2 up a semitone, then Duplicate bar, so 8 notes past the 16-step length | verified, shown as an error |
| 4 | B01 T1 + T2 (sync) | T2 step 11 up a semitone | verified |

## What the reads showed

Every change stayed in the tracks that were written. The pool, the `03 80`
residue on B01 T1 step 11, conditions, micro, swing, length, the other tracks,
the kit and A02 T12's 24 stored trigs past its length were all untouched.

But each write changed more than its edit:

| round | bytes changed | the edit | the rest |
|---|---|---|---|
| 1 | 24 | T2 step 5 note `FF` → 61 | 23 note/velocity/length lane bytes `FF` → 60 / 100 / 14, T2's defaults |
| 2 | 10 | T1 step 5 note 64 → 65 | 9 lane bytes `FF` → T1's defaults |
| 3 | 1 | T12 step 2 note 38 → 39 | none: every T12 lane was already locked |
| 4 | 18 | T2 step 11 note `FF` → 61 | 17 lane bytes `FF` → the defaults |

Nothing sounded different. Every trig that had followed its track's default
came back locked at that value, though, so a later change to the default on
the box would no longer reach it. Writes now keep `FF` in each lane that held
it while the value is still the default (`syntakt_pattern::locks_for`).
`crates/core/tests/all/syntakt_write.rs` replays all four rounds over these
baselines: each now changes one byte, and round 3 lands byte-for-byte on
`A02-after-write-round3.bin`.

Round 3's result read `verified byte-identical`, in red, in a modal titled "The
write did not go as asked". That happened because the past-length skip was a
warning, even though the confirm dialog had named it before consent. It is now
`WriteResult::skipped`, an informational count.

The B01 fetch reported "2 p-lock lane(s) stay on the box", counting the empty
record. Only lanes with a locked step are counted now.
