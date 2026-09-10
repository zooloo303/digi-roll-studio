# Writing to a Syntakt: what was tried, and the negative result

**Nothing was ever stored.** The box accepts a `0x51` pattern dump without an
error and applies none of it. Read this before trying again.

The `.syx` files here are the destination backups each attempt took before
sending — slot 1 (A02) at several points. They are the only thing the attempts
produced, and they are identical to each other.

## What was sent, and what came back

Every attempt fetched the destination, edited the payload, sent
`build_dump_message(0x16, 0x51, slot, payload)`, and re-fetched to compare.

| edit sent | result |
|---|---|
| none — the slot's own bytes back into itself | "verified", and **that proves nothing**: the content was already there |
| a note lock, `132: ff → 48` | the box still reads `ff` |
| the same, with the box switched to another pattern | the box still reads `ff` |
| the whole pattern into an *empty* slot | its trig word did not arrive |
| swing, `23207: 00 → 14` | the box still reads `00` |

The swing byte is what settles it. It is not per-step, not per-track, and one
byte; it did not stick either. **So this is not "the store ignores step data",
it is "no store is happening".**

## The mistake worth not repeating

The first attempt was reported as a success. It was not: sending a slot's own
bytes back into itself verifies whether or not anything happened, and the risk
had been named out loud beforehand and then not applied to the result. A write
test whose passing condition is also its failing condition tests nothing.

## Where to look next, and where not to

**Not a sweep of store opcodes.** DEVELOPMENT.md lesson 13 is an Analog Four
that had to be power-cycled four times over two days for exactly that, and its
conclusion is to price the probe: a sweep is right when a wrong guess costs a
round trip and wrong when it costs a power cycle. Nothing here has yet cost one,
and guessing at `0x5n` bodies is how that changes.

The same lesson names what ended it: capture Elektron's own software doing the
write. A CoreMIDI spy — MIDI Monitor on macOS installs one — recording Transfer
restoring a project would show the opcode, the framing and the order, with
nothing inferred. Whether Transfer moves patterns for this box at all is itself
unknown and worth establishing first.

## The box is fine

It answered identity normally after every attempt, on family `0x16` with dumps
supported. Nothing was wedged and nothing needed a power cycle.
