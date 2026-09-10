# Syntakt dump sweep — 2026-09-10

One read-only sweep of every request opcode the guard admits (`0x60`–`0x6e`),
taken in a single sitting with nothing touched on the box in between, so the
answers describe one state. Produced by
`cargo run -p digi_roll_studio --example syntakt_probe -- --out <dir>`.

Box: product 30, **build 0082, version 1.40** — the same OS as the eight donated
pairs in the browser repo's `dumps/fixtures/issue-8-2026-09-08/syntakt/`, so
these are directly comparable with that evidence rather than a separate series.

`.syx` is the raw frame, `.bin` the unpacked payload. Offsets below are into the
payload, decimal.

| request | response | payload | what it looks like |
|---|---|---|---|
| `0x60` | `0x50` | 31744 | pattern **and** kit |
| `0x61` | `0x51` | 27136 | pattern alone |
| `0x62` | `0x52` | 5632 | kit alone, opens `00 00 00 0b` then `KIT 1` |
| `0x63` | `0x53` | 263 | one sound (`be ef ba ce`, name `NEW `) |
| `0x64` | `0x54` | 512 | struct version 3, unidentified |
| `0x65` | `0x55` | 31744 | pattern and kit again, **not identical to `0x60`** |
| `0x66` | `0x56` | 512 | struct version 3, unidentified, differs from `0x64` |
| `0x6b` | `0x5b` | 263 | one sound, same leading bytes as `0x63` |

`0x67`–`0x6a`, `0x6c`–`0x6e` time out.

## The combined/standalone boundary, resolved

`docs/syntakt-pattern-format.md` in the browser repo flagged this as unresolved:
the probe reports 27136 + 5632 = 32768, which is 1024 more than the combined
dump's 31744, so "concatenating standalone sizes cannot map this combined
layout" and "do not slice a kit using those probe sizes". With all three dumps
taken in the same state, the reason is now measured rather than inferred.

| region of `0x60` | bytes | content |
|---|---|---|
| 0 – 23552 | 23552 | pattern, **byte-identical** to `0x61` over the same range |
| 23552 – 29184 | 5632 | kit, **byte-identical to the whole of `0x62`** |
| 29184 – 31744 | 2560 | all zero |

And `0x61`'s own tail, 23552 – 27136, is 3584 bytes of **all zero**.

So both dumps carry the same 23552 bytes of pattern and are padded to different
sizes. **The 1024-byte discrepancy was padding counted as content.** The kit
begins at 23552 in the combined dump, which is where the earlier note had
already seen `00 00 00 0b` followed by `KIT 1`; it is now established that the
region from there is the standalone kit exactly, and that nothing follows it but
zeros.

The kit holds **twelve** sound structs on a 263-byte stride, first at kit offset
46 — the same stride the standalone sound dump (`0x63`) reports as its whole
length.

## `0x65` is a second, different combined dump

Same size as `0x60` and the same leading bytes, but **156 bytes differ**. Their
offsets start at 984, 1967, 2950 — a stride of **983**, which is exactly the
block size the earlier note measured for the "13 blocks of 983 bytes" region
beginning at offset 4.

So the two requests return different values for the same per-block field. What
that field is, and which of the two dumps is the stored slot and which the
working state, is **not established here**. It is the obvious next question and
wants a pair taken across a deliberate edit, not more reading of one state.

## Not established

Nothing here decodes a note. The measured fields in the browser repo's format
note still stand alone: default inheritance, stride beyond track 1 step 1,
p-lock allocation, and the tempo/swing/length/micro conversions all remain
unresolved, and no write path or firmware allowlist entry exists for this box.
