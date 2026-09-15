# Plugin Preview

This is the isolated P0 application shell. Native hosting is not implemented yet.

Run `packaging/plugin-preview/run.sh` from this checkout. Add `--runtime-info`
to inspect startup paths without opening a window or MIDI client.

On macOS, `packaging/plugin-preview/build-macos.sh` creates an ad-hoc signed
preview app under this worktree's `dist/`. It does not install the app. Both
scripts use this worktree's `target/plugin-preview`, even if the shell has a
shared Cargo target configured. The preview identity is selected by the
`plugin-host` feature, including when launched directly from Finder.

Preview storage on macOS is `~/Library/Application Support/digi-roll-studio-plugin-preview`.
Recovery, backup stash and preset index share this root; eframe settings use its
`settings` child. Stable builds retain their existing paths. Future host caches,
logs and IPC must use the shared runtime root too. Hardware Auto-connect starts
off; it can be enabled deliberately in Setup. Open copies of hardware sessions.

Firmware and plugins are external dependencies. The user-supplied ROMs remain in
the original checkout's ignored `local/` directory. They are not copied into the
repository or application bundle. Third-party plugin preferences may be global.

See [the implementation plan](../../docs/PLUGIN_DEVICES_PLAN.md) for P1 onward.
