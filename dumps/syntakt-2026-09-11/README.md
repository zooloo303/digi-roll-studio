# Writing to a Syntakt: the three things that had to be true at once

**It works.** A pattern built on a computer now lands on the box, verified by a
read-back that could have failed. What follows is what the write needed, each
part measured against a control rather than reasoned about.

The day before, none of it worked. `write-attempt/README.md` in the 09-10 folder
is that negative result, and it stands: everything it says was sent really was
sent, and really was ignored.

## 1. The dump type is `0x50`, not `0x51`

`panel-send-0x50.syx` is the box's own SETTINGS > SYSEX DUMP > SYSEX SEND with
PATTERN selected: **36,294 bytes on the wire, 31,744 unpacked, dump type
`0x50`** — the pattern *with its kit*.

Every attempt on 09-10 sent `0x51`, the pattern alone, and `0x51` was chosen
deliberately: a dump that cannot address a sound cannot damage one. **That safety
argument was sound and it was also the reason nothing landed.** The box answers
`0x61` with a `0x51` happily; it appears to store nothing of it.

SEND and RECEIVE are the two halves of one menu, so what one emits is the best
evidence available about what the other takes — and it cost one read-only
listen to find out, against a plan that had budgeted a CoreMIDI spy and
Elektron's own software.

The pattern region is shared: the first **23,552 bytes** of a `0x50` payload
were byte-identical to the `0x51` read of the same slot. Nothing in the decoder
had to change.

## 2. The destination is the index byte, not the armed slot

Two trials, because the first was confounded and saying so is cheaper than
being wrong twice:

| armed in SYSEX RECEIVE | index byte | landed in | reads |
|---|---|---|---|
| A01 | 0 | A01 | confounded — both named the same slot |
| A02 | 2 | **A03** | the index byte decides |
| nothing armed | 3 | **A04** | no arming step is needed at all |

`a02-untouched-control.bin` is A02 read before any write. It is byte-identical
to `after-idx-01.bin`, read after all of them: **the armed slot was not written
to**, which is the control that makes the middle row mean something.

This is the opposite of SYXGRID's finding on a Digitone II, where the index is
"descriptive on READ" and the destination is the armed slot. That project
removed a destination-slot picker for being "a fiction". On a Syntakt running OS
1.40 the picker would have been true. **Two gen-2 boxes, two answers** — so
neither should be assumed for a third.

The manual agrees on arming, for what it is worth: §14.5.2 says the box "is
continuously listening for SysEx data". There is no interlock between a stray
36 KB SysEx and an overwritten pattern.

## 3. The frame has to be split, and the piece size is what matters

The controlled pair, same bytes and same destination minutes apart:

| delivery | landed |
|---|---|
| one `send` of 36,294 bytes | **nothing**, silently |
| 142 packets of 256 bytes at DIN rate | every byte |

That is PLAN.md §9's Analog Four finding reproduced on a box nine years newer,
and it was the wrong conclusion to stop at. **The rate is not the constraint.
The piece size is**, and a sweep says so:

| piece | packets | landed |
|---|---|---|
| 4,096 | 9 | yes |
| 8,192 | 5 | yes |
| 16,384 | 3 | yes |
| 20,000 | 2 | **no** |
| 36,294 (one call) | 1 | **no** |

All of the sweep ran at 6 ms between pieces, two orders of magnitude faster than
DIN, and the whole 36 KB goes out in about a twentieth of a second. Fewer
packets is not the problem — 3 pieces work and 2 do not — so the boundary is
somewhere between 16 KB and 20 KB per call, which has the shape of a buffer
rather than of a box that cannot keep up.

**This matters because it means the Syntakt needs no store path of its own.**
`device::paced_send` already chunks at `SEND_CHUNK`, 4 KB on macOS, which is
four times inside the boundary; the DT2 and DN2 go out through it today.

It also means the Windows question is open and probably bad. `SEND_CHUNK` is
`usize::MAX` there, because WinMM refuses a chunk that does not begin `0xF0` —
so on Windows this frame goes out in one call, which is the shape that stores
nothing. That is the Analog Four's position exactly (`a4_transfer::CAN_PACE`),
and a panel offering a Syntakt write on Windows would be offering the thing that
has never worked. **Untested: there is no Windows machine here.**

## The read-back that would have failed

`after-idx-00.bin` through `after-idx-04.bin` are A01–A05 after the runs.

| slot | before | after |
|---|---|---|
| A01 | empty | block 1, steps 1–16, swing 60 |
| A02 | one trig | one trig, byte-identical |
| A03 | empty | block 2, steps 1–8, swing 70 |
| A04 | empty | block 3, steps 1–4, swing 80 |
| A05 | empty | block 4, steps 1–2, swing 90 |

Every destination was empty first and the edits differ from each other, so a
box that ignored the write reads back differently from one that took it. **That
is the property 09-10's first "VERIFIED" did not have**, and the reason it was
worth nothing: it sent a slot's own bytes back into itself.

The full 23,552-byte pattern region of A01 matched what was sent, byte for
byte.

## What is still not known

- Whether `0x51` is stored under some condition nobody found, or never.
- What the 4,608 bytes of a `0x50` beyond the shared pattern region contain,
  beyond that they are the kit. Nothing here decodes them; they are carried
  through from the destination's own dump untouched.
- Whether the index byte is honoured on other gen-2 boxes, or on other Syntakt
  firmware. One box, one OS.
