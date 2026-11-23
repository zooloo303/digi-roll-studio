# Digi-Roll Studio v0.6.0

Adds Drum Steps and a track-led Kit Builder, and restores pattern-write access
for the latest verified DT2, DN2 and A4 firmware.

- **Firmware compatibility:** Digitakt II 1.17 (0082), Digitone II 1.12 (0062),
  Analog Four 1.56 (0204). Fixes #10. Earlier verified builds remain supported.
- **Drum Steps:** a 16-step-page editor alongside Piano Roll, with toggle,
  paint/erase, velocity, shared selection and undo, per-track playheads and Follow.
- **Kit Builder (`K`):** track grid, explicit preset destinations, bank/name/tag
  filters with Match any/all, selected-track or all-auditioned-track revert,
  and recovery backups retained after failed verification.
- **A4 writes:** preserve the destination's kit assignment. Unassigned blank
  patterns now ask you to assign a saved kit and save the pattern before writing.
- **Piano Roll:** fixes track focus persisting after marquee selection.
- Includes the measured Syntakt p-lock parameter map from @Tomokiiiiii (#11).
  This does not enable writes on Syntakt OS 1.42 or add a Syntakt p-lock editor.

Hardware verification wrote track 1 of A16 on all three updated devices,
compared the full payload, then restored and verified each original byte-for-byte.
The A4 test used a saved kit assignment. Original captures and regression tests
are committed with the source.

The new editors have offline interaction and native visual coverage. Full
Kit Builder audition/revert acceptance on these firmware versions remains
unverified. Recovery backups last for the app session; save wanted kits on the
hardware. Full-kit saving from DRS is not included.

Downloads: macOS Apple Silicon, Windows x64, Linux x86_64 tarball and Arch package.
