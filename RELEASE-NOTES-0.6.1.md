# Digi-Roll Studio v0.6.1

Fixes Drum Steps interaction during playback and improves Kit Builder and device switching.

- **Drum Steps:** left-click reliably toggles steps while stopped or playing,
  including fast clicks. Follow holds the visible page through an edit gesture.
- **Velocity:** Shift + left-drag adjusts velocity like Piano Roll: up increases,
  down decreases, and selected notes move together. Shift-click still selects;
  each velocity drag is one undo step.
- **Mouse buttons:** right-click and middle-click no longer edit drum steps.
  Right-click is reserved for future context actions.
- **Kit Builder:** track tiles have consistent dimensions. Long names and status
  lines truncate individually, with the full text available on hover.
- **Device focus:** BOX buttons in Kit Builder and Drum Steps switch the shared
  device selection directly, keeping both views in sync.
- **macOS packaging:** verifies that the executable declared by the app bundle
  is the newly built binary and matches the copy inside the installer. This
  prevents the stale-executable problem found in the local 0.6.0 test installers.

Validation includes 2,281 automated tests, Clippy, native full-app checks, and
user confirmation of the corrected macOS test build.

Downloads: macOS Apple Silicon, Windows x64, Linux x86_64 tarball and Arch package.
