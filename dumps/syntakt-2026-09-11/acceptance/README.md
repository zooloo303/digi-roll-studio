# The acceptance run, through the app's own flow

`examples/verify_firmware syntakt A05 … --throwaway`, 2026-09-11. This is the
same `safe_write::syntakt_safe_write_tracks` the Write panel and the mass send
call, driven without a mouse — so what it proves is the flow, not the widgets.

    Syntakt OS 1.40 build 0082: A05 track 1; restore afterwards
    WRITE:   ok=true, written=3, dropped=0, diffs=[], warnings=[]
    RESTORE: byte-identical

**`diffs=[]` is the strong claim here.** Every earlier run on this box compared
trig counts; this one compares all 31,744 bytes of the payload against what was
sent, and then restores the slot and compares again. Both passed.

What was sent, chosen so no two lanes carry the same number — a verify where
every lane holds the same value cannot tell a lane that arrived from a lane that
was already right:

| step | note | velocity | length | micro | condition |
|---|---|---|---|---|---|
| 1 | 60 | 100 | 14 | 0 | none |
| 5 | 67 | 80 | 7 | +8 | 2:4 |
| 10 | 55 | 120 | 14 | −8 | 1ST |

Swing went to 58%.

## The files

`A05-before.syx` is the backup the flow took before sending. `A05-after-the-write.syx`
is what the slot held afterwards, captured by the restore step's own
back-up-first rule. They differ in **34 wire bytes**, which is the write.

The box was left holding `A05-before.syx` again, byte for byte.

## What this still does not cover

The egui widgets above the flow. Pressing the button in the Write panel reaches
this exact function with a `SyntaktTrackWrite` built by
`core::syntakt_transfer`, and both halves have tests, but no hardware run has
gone through the panel itself.
