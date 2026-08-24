//! The Backups group's list, driven through headless egui passes.
//!
//! `restore.rs` covers the flow a press starts. This covers the thing a press is
//! *chosen from*, which lives inside a draw and is therefore invisible to any test
//! that stops at `plan`: whether the rows on screen are the rows in the store.
//!
//! **This file exists because of a deliberate-bug pass.** Three mutations of the
//! panel's freshness rules — never re-reading, a snapshot toggle that read
//! nothing, and a restore that did not re-list what it had just changed — all
//! passed a green suite of fifteen tests. Two of the three are now impossible
//! rather than tested (`Stash::generation` is asked every frame, and both rings are
//! always read); this is what holds the third, and what would have caught the other
//! two.
//!
//! The harness is `ui::pianoroll`'s: `Context::run_ui` over a fixed rect, with the
//! font-atlas delta cleared or epaint's debug assert fires when it drops.

use digi_core::device::{DeviceIo, PortRef};
use digi_core::{two_box_session, DeviceId, Session};
use digi_protocol::backup_stash::{BackupContext, Stash, StashEntry};
use digi_protocol::safe_write::{pattern_kit_file, Timestamp};
use digi_roll_studio::ui::restore::RestorePanel;

const RECT: egui::Rect =
    egui::Rect { min: egui::Pos2 { x: 0.0, y: 0.0 }, max: egui::Pos2 { x: 320.0, y: 900.0 } };

const NOW: Timestamp =
    Timestamp { year: 2026, month: 8, day: 1, hour: 12, minute: 34, second: 56 };

/// A DT2 with both ports wired, so its block draws its list rather than a reason
/// it cannot restore anything.
fn session() -> (Session, DeviceId) {
    let mut session = two_box_session();
    let id = session
        .devices
        .iter()
        .find(|d| d.model.key == "DT2")
        .expect("two_box_session has both boxes")
        .id;
    let device = session.device_mut(id).expect("just found it");
    device.io = DeviceIo {
        input: Some(PortRef { id: "in".into(), name: "Digitakt II".into() }),
        output: Some(PortRef { id: "out".into(), name: "Digitakt II".into() }),
        ..DeviceIo::default()
    };
    (session, id)
}

fn tmp_stash(tag: &str) -> Stash {
    let dir = std::env::temp_dir()
        .join(format!("digi-roll-app-panel-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Stash::at(dir)
}

/// A pattern-kit dump for one slot, stored under `kind`.
///
/// The bytes are not a real capture: nothing in this file decodes them, because
/// what is under test is which *rows* a block shows. `restore.rs` is where real
/// payloads go on and come off a box.
fn store(stash: &Stash, index: u8, kind: &str, second: u32) -> StashEntry {
    let at = Timestamp { second, ..NOW };
    let file = pattern_kit_file("digitakt2", family("digitakt2"), index, &[0u8; 64], kind, at);
    stash
        .stash(
            &file,
            &BackupContext {
                device_name: "Digitakt II".into(),
                kit_name: format!("KIT{index}"),
                track_index: Some(0),
            },
        )
        .expect("a writable temp directory")
}

/// The dump family byte for a box, off `protocol`'s own product table — the same
/// place an identity handshake gets it.
fn family(slug: &str) -> u8 {
    digi_protocol::device::PRODUCTS
        .iter()
        .find(|p| p.slug == slug)
        .map(|p| p.family)
        .unwrap_or_else(|| panic!("{slug} is not a product this repo knows"))
}

/// One headless frame of the group.
fn draw(ctx: &egui::Context, panel: &mut RestorePanel, session: &Session) {
    let input = egui::RawInput { screen_rect: Some(RECT), ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| {
        panel.ui(ui, session, digi_roll_studio::ui::write::PortsPresent::unknown(), false, false);
    });
    output.textures_delta.clear();
}

/// Filesystem timestamps are what the freshness rule compares, so a test that
/// changes the store has to let the clock move first. 20ms is far more than APFS
/// needs and still nothing to a test run.
fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(20));
}

#[test]
fn a_box_shows_its_own_backups_newest_first() {
    let (session, id) = session();
    let stash = tmp_stash("lists");
    let older = store(&stash, 0, "backup", 10);
    tick();
    let newer = store(&stash, 4, "backup", 40);

    let ctx = egui::Context::default();
    let mut panel = RestorePanel::at(stash.clone());
    draw(&ctx, &mut panel, &session);

    let files: Vec<&str> = panel.showing(id).iter().map(|e| e.file.as_str()).collect();
    assert_eq!(files, [newer.file.as_str(), older.file.as_str()], "newest first");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_backup_stored_since_the_last_frame_is_in_the_list_on_the_next_one() {
    // **The escape this file was written for.** A write finishing puts a backup in
    // the store, and the list has to have it by the time anyone looks — without
    // being told, because a restore's own snapshot and a second instance are not
    // things anything here gets told about.
    let (session, id) = session();
    let stash = tmp_stash("fresh");
    store(&stash, 0, "backup", 10);

    let ctx = egui::Context::default();
    let mut panel = RestorePanel::at(stash.clone());
    draw(&ctx, &mut panel, &session);
    assert_eq!(panel.showing(id).len(), 1);

    tick();
    let added = store(&stash, 1, "backup", 30);
    draw(&ctx, &mut panel, &session);

    let files: Vec<&str> = panel.showing(id).iter().map(|e| e.file.as_str()).collect();
    assert_eq!(files.len(), 2, "the store changed and the list did not follow it");
    assert_eq!(files[0], added.file, "and the new one is at the top");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn an_unchanged_store_is_not_read_again() {
    // The other half of following the store: a frame that changes nothing must not
    // go back to disk, or the comparison is decoration over a directory scan per
    // frame.
    //
    // **Asserted by making a re-read visibly wrong.** The first draft of this test
    // compared `generation()` before and after three frames, which passes whether
    // the panel caches or not — reading a directory does not change it — and a
    // deliberate-bug pass caught the name promising more than the body checked.
    // What is observable from out here is *what the panel is holding*: delete the
    // backup's own file and leave `index.json` alone, and the marker does not move,
    // so a panel that keeps its word still has the row and a panel that looks again
    // has lost it.
    //
    // It also pins the known limit of an mtime marker, which `Stash::generation`
    // owns up to: a file vanishing without the index changing is not noticed until
    // someone presses Refresh.
    let (session, id) = session();
    let stash = tmp_stash("still");
    let entry = store(&stash, 0, "backup", 10);

    let ctx = egui::Context::default();
    let mut panel = RestorePanel::at(stash.clone());
    draw(&ctx, &mut panel, &session);
    assert_eq!(panel.showing(id).len(), 1);

    let before = stash.generation();
    std::fs::remove_file(stash.dir().join(&entry.file)).expect("the backup was there");
    assert_eq!(stash.generation(), before, "removing a .syx does not restamp the index");

    draw(&ctx, &mut panel, &session);
    draw(&ctx, &mut panel, &session);
    assert_eq!(
        panel.showing(id).len(),
        1,
        "an unchanged marker was read as a reason to scan the folder again"
    );
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn pre_restore_snapshots_are_hidden_by_default_and_the_toggle_is_only_a_filter() {
    // Decision 6, and the shape that keeps it honest: both rings are read on every
    // reload, so ticking the box reveals rows that are already in hand. The draft
    // that read snapshots only while the box was ticked needed the toggle to
    // remember to re-list, and forgetting that left it doing nothing at all.
    let (session, id) = session();
    let stash = tmp_stash("snapshots");
    let backup = store(&stash, 0, "backup", 10);
    tick();
    let snapshot = store(&stash, 0, "pre-restore", 20);

    let ctx = egui::Context::default();
    let mut panel = RestorePanel::at(stash.clone());
    draw(&ctx, &mut panel, &session);

    let files: Vec<&str> = panel.showing(id).iter().map(|e| e.file.as_str()).collect();
    assert_eq!(files, [backup.file.as_str()], "a restore list is not the place for these");

    // No redraw between these two: if the tick had to trigger a re-read, this
    // would be empty of the snapshot and the assert below would say so.
    panel.show_snapshots(true);
    let shown: Vec<&str> = panel.showing(id).iter().map(|e| e.file.as_str()).collect();
    assert_eq!(shown, [backup.file.as_str(), snapshot.file.as_str()]);
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn one_boxs_block_never_offers_another_boxs_capture() {
    // Decision 1, from the list's end rather than `plan`'s: a Digitone capture is
    // not a thing the Digitakt's block can express, which is better than a thing it
    // refuses.
    let (session, id) = session();
    let stash = tmp_stash("families");
    let mine = store(&stash, 0, "backup", 10);
    tick();
    // A Digitone II backup in the same store — which is what the store looks like
    // the moment anyone writes to both boxes.
    let theirs =
        pattern_kit_file("digitone2", family("digitone2"), 0, &[0u8; 64], "backup", NOW);
    stash.stash(&theirs, &BackupContext::default()).expect("a writable directory");

    let ctx = egui::Context::default();
    let mut panel = RestorePanel::at(stash.clone());
    draw(&ctx, &mut panel, &session);

    let files: Vec<&str> = panel.showing(id).iter().map(|e| e.file.as_str()).collect();
    assert_eq!(files, [mine.file.as_str()]);
    assert_eq!(stash.backups(None).len(), 2, "both really are in the store");
    let _ = std::fs::remove_dir_all(stash.dir());
}

#[test]
fn a_box_with_nothing_in_the_store_shows_no_rows() {
    let (session, id) = session();
    let stash = tmp_stash("empty");

    let ctx = egui::Context::default();
    let mut panel = RestorePanel::at(stash.clone());
    draw(&ctx, &mut panel, &session);

    assert!(panel.showing(id).is_empty());
    // And an empty store has no index to stamp, which the freshness rule has to
    // survive: the first backup stored moves it from `None` to a time.
    assert_eq!(stash.generation(), None);
    tick();
    store(&stash, 0, "backup", 10);
    draw(&ctx, &mut panel, &session);
    assert_eq!(panel.showing(id).len(), 1, "a first backup has to appear too");
    let _ = std::fs::remove_dir_all(stash.dir());
}
