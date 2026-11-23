# November 23, 2026 firmware captures

DT2 1.17/build 0082, DN2 1.12/build 0062, A4 MKI 1.56/build 0204.

- `identity.txt`: live device identity.
- `A01.syx`, `A16.syx`, `kit-0.syx`: original read-only checksum/count-validated replies.
- `sound-A-1.bin`: original +Drive preset file.
- `A16-pre-write.syx`: fresh backup immediately before the successful write test.
- `A16-readback.syx`: hardware post-write read fetched by the restore flow,
  framed by DRS as the pre-restore backup (not the proposed encoded output).
- `verified.txt`: successful write/readback and byte-identical restore record.

Initial captures predate test preparation. In particular, the initial A4 A16
has an unset FF kit reference. Its first test failed because the device changed
that byte to 0, then restored exactly. After the user assigned/saved a kit,
the repeated test succeeded. The pre-write/readback pair records that successful
run. DRS now refuses unset-kit track writes before sending. See PLAN.md's
November verification entry for details and limitations.
