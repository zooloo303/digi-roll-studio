# Packaging

Four downloads, built four different ways, named so the install page can find
them.

| Platform | Artefact | Built by | Runs where |
|---|---|---|---|
| macOS, Apple Silicon | `Digi-Roll-Studio-<version>-macOS-AppleSilicon.dmg` | `macos/build-dmg.sh` | any arm64 Mac |
| Windows x64 | `Digi-Roll-Studio-<version>-Windows-x64-Setup.exe` | `windows/build-installer.ps1` | Windows only |
| Linux x86_64 | `Digi-Roll-Studio-<version>-Linux-x86_64.tar.gz` | `linux/build-tarball.sh` | glibc 2.35+ (Debian 12, Ubuntu 22.04, Fedora 36) |
| Arch x86_64 | `digi-roll-studio-<version>-1-x86_64.pkg.tar.zst` | `linux/build-pkg.sh` | Arch and derivatives |

All four land in `dist/`, which is gitignored. `.github/workflows/release.yml`
runs all four on a `v*` tag and hangs the results off a **draft** release.

**Why Linux is two downloads.** The tarball runs anywhere and can only *describe*
what it needs at runtime; the Arch package declares it, so `pacman -U` refuses
to install onto a machine that would then crash inside `dlopen`. Where pacman is
what you have, the package is the better install — and it is the one Linux
install this app has actually been run from. Neither is a different build: same
commit, same `cargo build --release`, different wrapper.

## The asset names are load-bearing

The install page picks each download out of the GitHub release by matching the
asset *name*:

```js
{ el: 'dl-mac', test: n => /\.(dmg)$/.test(n) || (/\.(zip|tar\.gz)$/.test(n) && /(mac|osx|darwin|apple|aarch64|arm64|universal)/.test(n)) }
{ el: 'dl-win', test: n => /\.(exe|msi)$/.test(n) || (/\.zip$/.test(n) && /(win|windows|x86_64-pc|x64)/.test(n)) }
```

**Linux has no button yet.** The page predates the Linux build, so its two
assets are only reachable from the release's file list. The matcher the page
needs, written to sit *after* the macOS one so the `.tar.gz` alternative there
gets first refusal on a Mac-named archive:

```js
{ el: 'dl-linux', test: n => /\.pkg\.tar\.zst$/.test(n) || (/\.tar\.gz$/.test(n) && /linux/i.test(n)) }
```

Which is also why `build-tarball.sh` names its output `-Linux-x86_64`: the
macOS matcher takes any `.tar.gz` whose name says mac, darwin, apple, arm64 or
universal, and a Linux asset must not be able to answer to that.

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

## Linux: what the tarball has to say that a package does not

Only `libasound` is linked. Everything else — Wayland, X11, xkbcommon, Vulkan,
EGL, D-Bus — is `dlopen`'d at runtime by eframe and wgpu, which means:

- `ldd` on the binary lists four libraries and tells you nothing about what it
  actually needs. `build-tarball.sh` runs it anyway, as a check that this is
  still true rather than as a dependency list.
- A machine missing one gets no link error and no useful message. It gets the
  app quitting inside `dlopen` at launch, which reads as a broken download.

The Arch package solves this by declaring all of them in `depends` — the
PKGBUILD's header lists where each SONAME came from. The tarball cannot, so it
carries an `INSTALL.md` naming them per distro, and an `install.sh` that puts
the binary, the icon and the desktop entry under `~/.local` without root.

`install.sh` rewrites the entry's `Exec=` to an absolute path. The committed
`.desktop` says `Exec=digi_roll_studio`, which is right for the package's
`/usr/bin` and wrong for `~/.local/bin`: a session's launcher does not reliably
have that directory on `$PATH`, and a menu entry that silently does nothing is
worse than no menu entry.

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
# 3. The workflow tests, builds all four, and drafts a release.
# 4. Open the draft, check all four assets are there, Publish.
```

Publishing is the step that changes the site: the page reads
`/releases/latest`, which ignores drafts.

To rehearse without spending a tag, run the workflow manually — it builds and
uploads all four artefacts and stops before the release.

## Building either one by hand

```sh
packaging/macos/build-dmg.sh              # needs an arm64 Mac
packaging/macos/build-dmg.sh --no-build   # reuse the existing release binary
```

```powershell
packaging\windows\build-installer.ps1     # needs Windows + Inno Setup 6.3+
packaging\windows\build-installer.ps1 -NoBuild
```

```sh
packaging/linux/build-tarball.sh          # needs Linux + libasound2-dev
packaging/linux/build-tarball.sh --no-build
packaging/linux/build-pkg.sh              # needs Arch: makepkg, base-devel
```

`build-pkg.sh` stamps the PKGBUILD's `pkgver` from `Cargo.toml` and its
`sha256sums` from the tarball it just made, so neither can drift from the
version being built. It passes `--nodeps` because the toolchain on a dev
machine is rustup's, not pacman's `rust`; the runtime `depends` are still
declared and still checked by `pacman -U` at install.

There is deliberately no mingw cross-compile from macOS to Windows: it would
mean a toolchain on every dev machine, and `build.rs` skips the resource
compiler when cross-compiling, so the exe it produced would ship without its
icon. The Windows half is built on Windows.
