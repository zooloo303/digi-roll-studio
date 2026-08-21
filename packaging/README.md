# Packaging

Two downloads, built two different ways, named so the install page can find
them.

| Platform | Artefact | Built by | Runs where |
|---|---|---|---|
| macOS, Apple Silicon | `Digi-Roll-Studio-<version>-macOS-AppleSilicon.dmg` | `macos/build-dmg.sh` | any arm64 Mac |
| Windows x64 | `Digi-Roll-Studio-<version>-Windows-x64-Setup.exe` | `windows/build-installer.ps1` | Windows only |

Both land in `dist/`, which is gitignored. `.github/workflows/release.yml` runs
both on a `v*` tag and hangs the results off a **draft** release.

## The asset names are load-bearing

The install page picks each download out of the GitHub release by matching the
asset *name*:

```js
{ el: 'dl-mac', test: n => /\.(dmg)$/.test(n) || (/\.(zip|tar\.gz)$/.test(n) && /(mac|osx|darwin|apple|aarch64|arm64|universal)/.test(n)) }
{ el: 'dl-win', test: n => /\.(exe|msi)$/.test(n) || (/\.zip$/.test(n) && /(win|windows|x86_64-pc|x64)/.test(n)) }
```

A name that stops matching does not break the page — the buttons keep their
hardcoded fallback of `/releases/latest`, so people land on a file list and have
to pick for themselves. Silent, and worth not doing.

## macOS: the re-signing step is not optional

`cargo build` produces a binary the linker has already ad-hoc signed. Copying it
into `Contents/MacOS/` and then writing an `Info.plist` and an `AppIcon.icns`
around it **invalidates that signature**, because the signature covers a code
directory that takes in the bundle's resources.

On arm64 that is fatal, not cosmetic: macOS will not launch a binary whose
signature does not verify, and the error is *"the application is damaged and
can't be opened"* — the case the install page documents as an edge case behind
an `xattr -dr` command. Skip the re-sign and it is not an edge case, it is what
every download does.

So the last thing `build-dmg.sh` does to the bundle, after the binary, the plist
and the icon are all in place, is:

```sh
codesign --force --deep --sign - "dist/Digi-Roll Studio.app"
```

and then it verifies — twice, once on the bundle and once on the copy inside the
mounted image, because what the user launches is the copy that came out of the
`.dmg`.

Ad-hoc is not a Developer ID and not notarisation. The page's *Privacy &
Security → Open Anyway* walkthrough is still the expected first run; that is a
different, clickable thing from damaged.

## Windows: three things the app needed before it could be installed

None of these show up in a `cargo run`, which is why they are called out:

1. **`windows_subsystem = "windows"`** (`crates/app/src/main.rs`). Without it the
   exe is a console binary and every launch opens a black `cmd` window next to
   the real one, for the life of the process.
2. **The exe's own icon and name** (`crates/app/build.rs`). The runtime
   `with_icon()` sets the icon of a *running window*; Explorer, the taskbar's
   pinned entry, the Start-menu shortcut and the SmartScreen dialog all read a
   resource compiled into the file. Unset, they read `digi_roll_studio`.
3. **A static CRT** (`.cargo/config.toml`). MSVC targets link
   `vcruntime140.dll` dynamically by default, so a machine without a VC
   redistributable gets a missing-DLL box before any of our code runs.

The installer is per-user (`PrivilegesRequired=lowest`) and raises no UAC
prompt. An unsigned installer asking for admin gets the red *unknown publisher*
dialog, which would stack a second scary prompt on top of the SmartScreen
warning the page already walks people through.

## Regenerating the icons

Both platforms build their icon from the committed PNGs in `icons/`, so the art
has one home.

- **macOS** — `build-dmg.sh` runs `icons/README.md`'s `iconutil` recipe on every
  build; there is no committed `.icns`.
- **Windows** — `icons/windows/icon.ico` **is** committed, so neither CI nor a
  Windows box needs an image library. Re-make it after changing the art:

  ```sh
  python3 packaging/windows/make-ico.py
  ```

  macOS only (it borrows the ICO encoder in `sips`). 16 and 32 come from the
  purpose-drawn masters in `icons/mac/`, which use the simplified cut described
  in `icons/README.md`.

## Cutting a release

```sh
# 1. Bump [workspace.package] version in Cargo.toml, commit.
# 2. Tag it. The workflow checks the tag against Cargo.toml and fails if they
#    disagree, so the assets can't be named one version while the release says
#    another.
git tag v0.1.0
git push origin v0.1.0
# 3. The workflow tests, builds both, and drafts a release.
# 4. Open the draft, check both assets are there, Publish.
```

Publishing is the step that changes the site: the page reads
`/releases/latest`, which ignores drafts.

To rehearse without spending a tag, run the workflow manually — it builds and
uploads both artefacts and stops before the release.

## Building either one by hand

```sh
packaging/macos/build-dmg.sh              # needs an arm64 Mac
packaging/macos/build-dmg.sh --no-build   # reuse the existing release binary
```

```powershell
packaging\windows\build-installer.ps1     # needs Windows + Inno Setup 6.3+
packaging\windows\build-installer.ps1 -NoBuild
```

There is deliberately no mingw cross-compile from macOS to Windows: it would
mean a toolchain on every dev machine, and `build.rs` skips the resource
compiler when cross-compiling, so the exe it produced would ship without its
icon. The Windows half is built on Windows.
