# September 9, 2026 firmware captures

Captured from Neil's Digitakt II OS 1.16/build 0079, Digitone II
OS 1.11/build 0059, and Analog Four MKI OS 1.55D/build 0201.

- `identity.txt`: live identity responses, including build and product IDs.
- `A01.syx`, `A16.syx`, `kit-0.syx`: original read-only SysEx replies,
  saved by `capture_firmware` after checksum/count validation.
- `sound-A-1.bin`: unmodified +Drive `/soundbanks/A/1` files.
- `A16-readback.syx`: the post-write payload fetched by the restore flow,
  framed as its pre-restore backup. These are fresh hardware reads, not the
  encoder's proposed output; the framing is reconstructed by DRS.
- `verified.txt`: identities and results from `verify_firmware`. All three
  write/readback cycles and restorations passed full-payload byte comparison.

The initial read-only captures and subsequent write tests used different
project states: Neil loaded throwaway projects before authorizing writes to
A16. Do not treat the initial A16 capture as the pre-write backup. Those exact
backups remain in `local/firmware-2026-09-09/verify-{dt2,dn2,a4}/`.

The test notes, conditions and p-lock values are specified in the verification
example and asserted against the captured readbacks by `firmware_2026_09.rs`.
See PLAN.md's September OS compatibility entry for scope and limitations.
