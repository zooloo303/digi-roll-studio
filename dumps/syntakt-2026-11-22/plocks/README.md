# Syntakt p-lock parameter IDs — 2026-11-22

Contributor plan item 1 ("map a small useful set of p-lock parameters").
Read-only: every file is a `0x61` reply (pattern alone, 27,136 bytes) for
pattern **A01 (slot 0)**, taken with `crates/app/examples/syntakt_plock_capture.rs`,
which reads twice and refuses to save if the two reads differ. Nothing was sent
to the box but dump requests.

Box: Syntakt, product 30, **OS 1.42, build 0097**, USB, macOS.
Track 1: **SY BITS** (digital). Track 9: **SD HARD** (analog).
Every lock is on **TRIG 1** of its track. `captures.tsv` lists the order.

## IDs

| ID | Parameter | Value as shown | Tracks measured |
|---|---|---|---|
| 28 | FLTR FREQ | raw / 256 | 1, 9 |
| 29 | FLTR RESO | raw / 256 | 1, 9 |
| 39 | AMP ATK | raw / 256 | 1, 9 |
| 40 | AMP HOLD (AHD) | raw / 256 | 9 |
| 41 | AMP DEC | raw / 256 | 9 (AHD), 1 (ADSR) |
| 42 | AMP SUS (ADSR) | raw / 256 | 1 |
| 43 | AMP REL (ADSR) | raw / 256 | 1 |
| 44 | AMP DELAY send | raw / 256 | 9 |
| 45 | AMP REVERB send | raw / 256 | 9 |
| 46 | AMP PAN | raw / 256 − 64 (L64 = 0x0000, R47 = 0x6F00) | 9 |
| 47 | AMP VOL | raw / 256 | 1, 9 |
| 49 | AMP envelope mode (AMP page 2) | high byte: ADSR = 1; AHD = 0 inferred, not captured | 1 |

FREQ, RESO, ATK and VOL carry **the same ID on a digital and an analog
machine**. The others were measured on one track each; do not assume more.

The ID belongs to the parameter, not to the knob position: in ADSR mode the
AMP page reads ATK · DEC · SUS · REL, and DEC is still 41 (captured in both
modes) while the third knob is SUS = 42.

## Values

- 16-bit big-endian, **display × 256**; the low byte is the fraction a fine
  knob turn leaves: `53.27` = `0x3546`, `88.08` = `0x5815`, `53.96` = `0x35F6`,
  `80.21` = `0x5037`. An integer on screen can still carry a fraction (`80` was
  `0x5002`). The screen **truncates** to two decimals: `0x45B0` = 69.6875 shows
  `69.68`. Keep the raw value.
- Minimum and maximum of FREQ and RESO: `0x0000` and `0x7F00`.

## Pool behaviour seen on the way (evidence for item 2, not a full study)

- Records are kept **sorted by ID** and shift to make room: FREQ (28) locked
  after RESO (29) on track 9 went in front of it (`11-…`).
- Removing a lock by deleting and re-placing the trig returned the pattern
  **byte-identical** to the baseline (`04-back`, `08-back` = `00-baseline`).
- **Deleting a trig clears its records**, twice: track 9's (nine records,
  between `19` and `20`, an accidental TRIG press) and track 1's (six, `26`).
  The step word left behind differed: `0x0000` for track 9, `0x0380` for
  track 1 (only bit 0 cleared). Not explained.
- A knob turned **without a trig held** changes the kit and **not one byte of
  the pattern** (`00b-…`, `08b-…`).

## Files

`00-baseline` — a trig on track 1 TRIG 1, no locks.
`00b-other-track-reso-kit-only`, `08b-t9-reso-kit-only-no-trig` — kit-only edits.
`01`–`04` RESO on track 1 and back; `05`–`08` FREQ and back; `09` track 9 trig;
`10`–`19` track 9: RESO, FREQ, AMP page, PAN L64; `20`–`25` track 1: ATK, VOL,
envelope mode ADSR, REL, SUS, DEC; `26-final-cleared` — track 1's trig deleted.
Each `.syx` is the raw frame for the `.bin` beside it.

Tests: `digi_protocol::syntakt_pattern::measured_plock_tests` decode every
value above from these files.
