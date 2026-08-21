//! Stamps the icon and the version strings into the Windows executable.
//!
//! This is about the *file*, not the window. `main.rs`'s `icon()` sets the icon
//! of a running window (and, on macOS, the Dock tile); nothing it does at run
//! time can affect what Explorer, the taskbar's pinned entry, or the shortcut
//! the installer drops on the desktop draw before the process exists. Those read
//! a resource compiled into the PE, which is what this does.
//!
//! Everywhere else this file is an empty `main`.

fn main() {
    // Two guards, and they are not redundant, because a build script is compiled
    // and run on the *host* while the thing it is building is for the *target*:
    //
    //   - `cfg(windows)` is the host. `winresource` is declared under
    //     `[target.'cfg(windows)'.build-dependencies]`, so on macOS and Linux the
    //     crate is not fetched at all and this block must not exist to be
    //     compiled.
    //   - `CARGO_CFG_TARGET_OS` is the target, and is the only one that answers
    //     the question we actually care about. `cfg!(target_os = "windows")` here
    //     would describe the machine doing the building — the crate's own README
    //     calls this out as the mistake everyone makes.
    //
    // The gap between them: cross-compiling to Windows *from* macOS or Linux
    // takes neither branch, so that build produces a working but icon-less exe.
    // That is deliberate — it keeps `mingw-w64` off the list of things a Mac
    // needs to build this — and it is why the release workflow builds the Windows
    // binary on a Windows runner rather than cross-compiling it here.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        use std::path::Path;

        // Up out of `crates/app` to the workspace root, so the one committed copy
        // of the mark is the one that ships — the same reasoning as the
        // `include_bytes!` in `main.rs`. Absolute, because a relative path would
        // be written verbatim into a generated `.rc` that lives in `OUT_DIR` and
        // left for `rc.exe` to resolve against its include path.
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets this");
        let root = Path::new(&manifest)
            .parent()
            .and_then(Path::parent)
            .expect("crates/app is two levels below the workspace root");
        let icon = root.join("icons").join("windows").join("icon.ico");

        assert!(
            icon.exists(),
            "{} is missing — regenerate it with packaging/windows/make-ico.py",
            icon.display()
        );

        let mut res = winresource::WindowsResource::new();
        res.set_icon(icon.to_str().expect("the repo path is UTF-8"));

        // Without these, all four of these fields read `digi_roll_studio`: the
        // crate defaults them to `CARGO_PKG_NAME`, underscores and all. Windows
        // puts `FileDescription` — not `ProductName` — under the Name column in
        // Task Manager and in the "unrecognised app" SmartScreen dialog, so it is
        // the one that has to be right.
        res.set("FileDescription", "Digi-Roll Studio");
        res.set("ProductName", "Digi-Roll Studio");
        res.set("CompanyName", "zooloo303");
        res.set("OriginalFilename", "Digi-Roll Studio.exe");
        res.set(
            "LegalCopyright",
            "Free software under the GPL-3.0-or-later. Not affiliated with Elektron.",
        );

        res.compile().expect("rc.exe failed on the icon resource");
    }

    // `winresource` emits no rerun directives of its own, so without these a
    // redrawn icon would sit in the tree and never reach a rebuilt exe.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../icons/windows/icon.ico");
}
