// Release builds on Windows are GUI apps, not console apps.
//
// Without this the linker marks the exe as a console subsystem binary, and every
// launch — from the Start menu, from the desktop shortcut, from the installer's
// "run now" tick — opens a black cmd window alongside the real one and keeps it
// there for the life of the process. It is the single most obvious way a
// packaged Rust GUI app looks broken, and it is invisible until someone runs the
// installed build rather than `cargo run`.
//
// Gated on `debug_assertions` so a debug build stays a console app: this crate
// prints nothing itself, but a panic message going to a window that closes with
// the process is the difference between a debuggable crash and a silent one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use digi_roll_studio::engine::EngineLink;
use digi_roll_studio::ui::edit::EditPanel;
use digi_roll_studio::ui::generate::GeneratePanel;
use digi_roll_studio::ui::harmony::HarmonyPanel;
use digi_roll_studio::ui::pianoroll::PianoRoll;
use digi_roll_studio::ui::ports::PortsPanel;
use digi_roll_studio::ui::rail::{self, Sidebars};
use digi_roll_studio::ui::autoconnect::AutoConnect;
use digi_roll_studio::ui::restore::RestorePanel;
use digi_roll_studio::ui::session::SessionPanel;
use digi_roll_studio::ui::setup::SetupPanel;
use digi_roll_studio::ui::sync::SyncPanel;
use digi_roll_studio::ui::tracks::Selection;
use digi_roll_studio::ui::transfer::TransferPanel;
use digi_roll_studio::ui::write::WritePanel;
use digi_roll_studio::ui::{edit, setup, tools, transport, workspace};
use digi_core::device::PortRef;
use digi_core::history::{Content, History};
use eframe::egui;

struct App {
    /// The session model from Phase 2. The app opens with a DT2 and a DN2 in it,
    /// 16 tracks each.
    session: digi_core::Session,
    /// The engine thread, and the ports it is sending to.
    engine: EngineLink,
    roll: PianoRoll,
    ports: PortsPanel,
    /// The fetch group of the Setup panel: which slot each box is pulling, and
    /// the last answer it got.
    transfer: TransferPanel,
    /// The send group beneath it: the smallest of the three things in this app
    /// that can store bytes on a box — one track of one slot.
    write: WritePanel,
    /// The biggest: every track of the session onto every box, behind one
    /// dialog and one backup per slot.
    sync: SyncPanel,
    /// Which of the two dangerous groups in the Setup panel are folded open.
    setup: SetupPanel,
    /// Gives an unclaimed Elektron port to a box that has none, on launch and
    /// when one is plugged in. Never moves a port anyone picked by hand.
    autoconnect: AutoConnect,
    /// The backups group under that — the other one, and the bigger of the two:
    /// a restore replaces a whole slot rather than one track of it.
    restore: RestorePanel,
    /// Which of the 32 tracks the roll is editing.
    selection: Selection,
    /// Which side panels are open, and which tool the left one shows.
    bars: Sidebars,
    /// The rail's fourth slot: save and open the session file, and the close
    /// guard that stops the window taking unsaved work with it.
    session_file: SessionPanel,
    /// The rail's *first* slot, real since Phase 9: velocity, length, PROB,
    /// swing, duplicate bar, clear, the p-lock lane list, and MIDI files.
    edit: EditPanel,
    /// The rail's *second* slot, real since Phase 11: the key the roll tints by,
    /// chord draw, and harmonising a selection.
    harmony: HarmonyPanel,
    /// The rail's *third* slot: genre, progression, feel, seed, and the row list
    /// of parts a press of Generate writes into the session — see
    /// `ui::generate`'s header for why that press never touches a box on its
    /// own.
    generate: GeneratePanel,
    /// Undo and redo, over the music only. `core::history` has the argument for
    /// where that line is; the shell's job is the two calls below that decide
    /// where one step ends.
    history: History,
}

impl Default for App {
    fn default() -> Self {
        Self {
            session: digi_core::default_session(),
            engine: EngineLink::default(),
            roll: PianoRoll::default(),
            ports: PortsPanel::default(),
            transfer: TransferPanel::default(),
            write: WritePanel::default(),
            sync: SyncPanel::default(),
            setup: SetupPanel::default(),
            autoconnect: AutoConnect::default(),
            restore: RestorePanel::default(),
            selection: Selection::default(),
            bars: Sidebars::default(),
            session_file: SessionPanel::default(),
            edit: EditPanel::default(),
            harmony: HarmonyPanel::default(),
            generate: GeneratePanel::default(),
            history: History::default(),
        }
    }
}

impl eframe::App for App {
    // egui 0.36 replaced `App::update(&Context)` with `App::ui(&mut Ui)`, and
    // folded `SidePanel` into `Panel`. Panels now nest inside a Ui rather than
    // being hung off the Context.
    //
    // **The order the panels are added is the layout.** The first is the
    // outermost, so the transport goes in first and spans the window above
    // everything; then the rail, which cannot be hidden; then the two panels that
    // can be; then the workspace takes what is left. `ui::mod` draws it.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Anything that edits the session sets this, and one snapshot goes to the
        // engine at the end of the frame. One snapshot, not one per edit: the
        // engine takes a whole session at a time so the boxes can never pick up
        // halves of an edit (PLAN.md §4).
        let mut edited = false;
        // **The history moved, which is not the same as an edit.** An undo changes
        // the session and has to reach the engine and the dirty flag, but the shell
        // must not record a step for it — see `edit::Outcome::stepped`.
        let mut stepped = false;

        // **The content as of before this frame, and the order matters.** `Session`
        // is copy-on-write, so holding this is what makes the frame's first edit
        // *clone* the pattern it touches rather than mutating it in place — taking
        // the snapshot afterwards would hand the history a view of the edit it was
        // meant to be the "before" of.
        //
        // Only while no step is open: a drag's second and later frames need none,
        // and skipping them is what keeps a forty-frame gesture from cloning a
        // pattern forty times.
        let before = (!self.history.is_open()).then(|| Content::of(&self.session));

        // **The engine owns which scene is sounding, and this follows it.** A
        // scene change is taken at a boundary the engine decides, so the model's
        // `current_scene` is a report of what the boxes are doing rather than a
        // control. Copying it here is what makes the patterns pane and the roll
        // land on the patterns that are now playing the moment the switch goes
        // past, without either of them knowing scenes exist. It is not an edit:
        // the engine is where the number came from.
        let sounding = self.engine.playing_scene();
        if sounding < self.session.scenes.len() {
            self.session.current_scene = sounding;
        }

        // The two write groups' workers talk to the window, not to the panel they
        // are drawn in: each blocks on a confirm dialog, and the Setup panel can be
        // closed. Polled and drawn here so closing that panel mid-write cannot
        // strand a thread on a question nobody can be shown.
        self.write.tick(ui);
        self.sync.tick(ui);
        self.restore.tick(ui);

        // **The close guard, and it has to be here.** `App::on_exit` runs after
        // the decision to close has been taken, so it can tidy up but cannot
        // refuse; egui reports the request as viewport input and `CancelClose`
        // withdraws it. Drawn from the shell rather than from the Session panel
        // for the same reason the two write workers are: that panel can be
        // closed, and a question nobody can be shown is a window that will not
        // shut.
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.session_file.allow_close() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        if self.session_file.guard_ui(ui, &self.session) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Cmd+Z and Cmd+Shift+Z, read here rather than in the Edit panel because
        // that panel can be closed and a shortcut that only works while a panel is
        // showing is a shortcut nobody finds. Same reason the close guard is here.
        stepped |= edit::shortcuts(ui, &mut self.session, &mut self.roll, &mut self.history);

        // **Auto-connect, before the panels are drawn.** It may give a box its
        // ports, and a strip drawn from the old routing would be a frame behind —
        // the same reason `PortsPanel::poll` asks for a repaint when a handshake
        // lands. Held off while anything is talking to a box: those hold the
        // ports open, and re-enumerating underneath one is not a state to be
        // good at.
        //
        // Its answer joins `edited` rather than sitting on its own, because a
        // routing change has to reach the engine or the box would be bound and
        // silent until the next note moved.
        let talking = self.transfer.busy()
            || self.write.busy()
            || self.restore.busy()
            || self.sync.busy();
        edited |= self.autoconnect.tick(&mut self.session, &mut self.ports, talking, ui.ctx());

        // The ports as of the last enumeration, for rebinding a session off
        // disk. The cached list rather than a fresh one: it is what every other
        // view of the desk uses, so a load agrees with what Setup is showing.
        // **After auto-connect**, because that is now the thing that moves the
        // list most often — Setup's Refresh is no longer the only one, and
        // reading it first would rebind a loaded session against ports that
        // were enumerated a frame ago.
        let available_in: Vec<PortRef> = self
            .ports
            .inputs()
            .iter()
            .map(|p| PortRef { id: p.id.clone(), name: p.name.clone() })
            .collect();
        let available_out: Vec<PortRef> = self
            .ports
            .outputs()
            .iter()
            .map(|p| PortRef { id: p.id.clone(), name: p.name.clone() })
            .collect();

        // **Both of these panels get an explicit frame, and it is a zero-margin
        // one.** `egui::Panel`'s default frame carries `Margin::symmetric(8, 2)`
        // *outside* the `Ui` the body paints into, so a bar or a rail that fills
        // its own `Ui` edge to edge still leaves a sliver of the theme's default
        // panel fill around it — a pale seam under the transport bar and down the
        // rail's outer edge, at exactly the two places the v2 design puts a 1px
        // border. Each body already paints its own background (`transport::ui`
        // fills `PANEL_BG_RAISED` and draws the bar's bottom border, `rail::ui`
        // fills `INSET_BG`), so the frame here only has to get out of the way;
        // the fill is repeated on the rail's so a resize frame cannot flash the
        // default colour before the body has drawn.
        egui::Panel::top("transport")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                edited |= transport::ui(
                    ui,
                    &mut self.engine,
                    &mut self.session,
                    &mut self.bars.setup_open,
                );
            });

        // 86px, not 88: `design_handoff_digi_roll_ui_v2/README.md` §2b and its
        // sizes table both say 86, and the rail's rows are drawn to that width.
        egui::Panel::left("rail")
            .exact_size(86.0)
            .frame(egui::Frame::NONE.fill(digi_roll_studio::ui::INSET_BG))
            .show(ui, |ui| rail::ui(ui, &mut self.bars));

        // Both side panels are opened and closed the same way, and both need the
        // same dance: `show_collapsible` holds a `&mut` to the open flag for the
        // whole call — including drag-to-collapse, which it uses to set it — so
        // the panel's own × cannot touch it. Each body reports the click instead,
        // and it is applied here, after the borrow has gone.
        let tool = self.bars.tool;
        let mut tool_open = self.bars.tool_open;
        // 330px — the v2 width for the left panel (§2c's body-columns table).
        // Grew from 272 with the panels themselves: the slider rows are now
        // `[ label 62px ][ track flex:1 ][ value 38px ]`, and at 272 the track
        // between the two fixed ends was down to a stub.
        //
        // **`exact_size`, not `default_size`, and that is load-bearing.**
        // `default_size` sets only the *initial* width and leaves the maximum
        // unbounded. A v2 section header ends in a `Separator` and the panels
        // are full of plain labels, and both of those ask for every pixel they
        // can see — so an unbounded panel got all of them: the tool panel grew
        // over the roll, over Setup, over the whole centre column, which is the
        // one thing this layout may never do. The roll is always visible.
        //
        // Pinning it also fixes the second half of the same bug. A label only
        // wraps against a known width; in an auto-sizing panel there is none,
        // so the prose ran off the edge and was clipped rather than wrapped.
        // 330px is §2c's own figure for this column, so the spec width and the
        // width that makes the text behave are the same number.
        let panel = egui::Panel::left("tools")
            .exact_size(330.0)
            .show_collapsible(ui, &mut tool_open, |ui| {
                tools::ui(
                    ui,
                    tool,
                    &mut self.session_file,
                    &mut self.edit,
                    &mut self.harmony,
                    &mut self.generate,
                    &mut self.session,
                    self.selection,
                    &mut self.roll,
                    &mut self.history,
                    &available_in,
                    &available_out,
                )
            });
        let tool_outcome = panel.map(|r| r.inner).unwrap_or_default();
        self.bars.tool_open = tool_open && !tool_outcome.close;
        edited |= tool_outcome.edited;
        stepped |= tool_outcome.stepped;
        // **Not folded into `edited`.** A session just read off disk needs the
        // engine rebuilt around it, but it is not unsaved work — folding the two
        // together would mark a file dirty the instant it was opened.
        let reloaded = tool_outcome.reloaded;

        let mut setup_open = self.bars.setup_open;
        // Pinned for the same reason the tool panel is, above — §2c's 320px.
        let panel = egui::Panel::right("setup").exact_size(320.0).show_collapsible(
            ui,
            &mut setup_open,
            |ui| {
                setup::ui(
                    ui,
                    &mut self.session,
                    &mut self.engine,
                    &mut self.setup,
                    &mut self.ports,
                    &mut self.autoconnect,
                    &mut self.transfer,
                    &mut self.write,
                    &mut self.sync,
                    &mut self.restore,
                    self.selection,
                )
            },
        );
        if let Some(response) = panel {
            let (changed, close) = response.inner;
            edited |= changed;
            setup_open &= !close;
        }
        self.bars.setup_open = setup_open;

        egui::CentralPanel::default().show(ui, |ui| {
            edited |= workspace::ui(
                ui,
                &mut self.session,
                &mut self.engine,
                &mut self.selection,
                &mut self.roll,
            );
        });

        // One change-tracking path, not two: the flag the engine snapshot is
        // keyed off is the flag the session file is keyed off. An undo counts —
        // it is a change relative to what is on disk.
        self.session_file.mark_edited(edited || stepped);

        // **Where an undo step begins and ends.** `begin` on the first frame of a
        // change, `commit` when the pointer comes up — so a drag across forty
        // frames is one step and a keypress is its own. A gesture that changed
        // nothing back to nothing is dropped inside `commit`. `core::history` has
        // the whole argument, including why the desk is not in it.
        //
        // A load or a history move abandons any open step rather than committing
        // it: it would be measured against music no longer in the window.
        if reloaded {
            self.history.clear();
        } else if edited {
            if let Some(before) = before {
                self.history.begin(before);
            }
        }
        if !ui.ctx().input(|i| i.pointer.any_down()) {
            self.history.commit(&self.session);
        }

        if edited || reloaded || stepped {
            // Also picks up a routing change — identifying a box gives it an out
            // port, and the engine has to be rebuilt around the new connection.
            self.engine.sync(&self.session);
        } else {
            self.engine.reroute(&self.session);
        }
    }
}

/// The window icon — mark C, the 256px master out of `icons/`.
///
/// **Not decoration: without this the app wears egui's own icon.** The default
/// `ViewportBuilder` ships a white `e` on black, so an unset icon is not the OS
/// default, it is someone else's branding.
///
/// **This does set the Dock icon on macOS, with no `.app` bundle involved — and
/// the obvious reasoning says otherwise, so it is written down.** winit's
/// `with_window_icon` really is documented "Windows and X11 only", because Cocoa
/// has no per-*window* icon. eframe does not stop there: `native::app_icon`'s
/// `set_title_and_icon_mac` hands the same bytes to
/// `NSApplication::setApplicationIconImage`, which is the *application* icon and
/// is what the Dock and ⌘-Tab read. Checking winit and stopping was how this
/// comment first claimed the opposite.
///
/// So the Dock is the **largest** consumer of the PNG below, which is the one
/// argument for making the master bigger than it is. The bundle this asks for now
/// exists — `packaging/macos/build-dmg.sh` assembles it, and runs the `iconutil`
/// recipe from `icons/README.md` to get the `.icns` that the Finder and the
/// installer read. This PNG is still what the Dock shows once the app is running.
///
/// The bytes are embedded rather than read at run time so the binary stays a
/// single file. Reaching out of the crate into the workspace's `icons/` keeps one
/// copy of the mark instead of a drifting duplicate — which does mean `icons/`
/// is now a build input, not just a design folder, so it has to be committed.
fn icon() -> egui::IconData {
    // 256px. Enough for a magnified Dock tile (128pt @2x) with nothing spare, so
    // this is the size to raise first if the Dock ever looks soft — 512 would be
    // the safer master, and the only reason it is not that today is that 1024
    // would put 468 KB of never-drawn pixels in the binary.
    const PNG: &[u8] = include_bytes!("../../../icons/windows/icon-256.png");
    // Embedded at compile time, so a failure here means the committed PNG is
    // broken — a build mistake, not a run-time condition, and one worth being
    // loud about rather than launching with a silently missing icon.
    eframe::icon_data::from_png_bytes(PNG).expect("icons/windows/icon-256.png is not a valid PNG")
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        // Wide enough for the roll to be worth looking at with the rail, a tool
        // panel and Setup all open at once — which is the layout's whole claim.
        //
        // Bumped from 1440 × 900 by the v2 pass, which widened the tool panel
        // from 272 to 330: at the old size the three of them took 736px of a
        // 1440 window and the centre column — now the *only* thing between them,
        // since scenes moved into the transport bar — was left 704. 1600 × 1000
        // puts it back to 864 and keeps the window inside a 1680 × 1050 display,
        // which the README's own 1900 × 1120 would not.
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 1000.0])
            .with_icon(icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Digi Roll Studio",
        options,
        Box::new(|cc| {
            // The app's one global style change, and it has to be made before
            // the first frame: egui's labels are selectable by default, which
            // silently ate the click on every custom-painted row in this UI.
            // `ui::install_style` has the whole story.
            digi_roll_studio::ui::install_style(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
}

#[cfg(test)]
mod tests {
    /// The icon decodes, and to the size the name claims.
    ///
    /// `icon()` panics on a bad PNG and is called before the first frame, so
    /// without this the first thing to find a broken or moved asset is a launch.
    /// Cheap to check here instead: the bytes are already in the binary.
    #[test]
    fn icon_decodes_at_256() {
        let icon = super::icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }

    /// The committed `.ico` is a well-formed multi-size icon.
    ///
    /// `icons/windows/icon.ico` is a *committed build input*, same as the PNG
    /// above: `build.rs` hands it to the Windows SDK's resource compiler, and
    /// Inno Setup reads it for the installer's own icon. It is committed rather
    /// than generated at build time so that neither CI nor a Windows box needs
    /// an image library — `packaging/windows/make-ico.py` regenerates it, and
    /// only runs on macOS.
    ///
    /// Which leaves a committed binary that nothing on this machine reads. Both
    /// consumers are Windows-only, so a truncated or wrongly-rebuilt file would
    /// get through a full green run here and surface as a failed release build,
    /// or — worse, because `rc.exe` is happy with a valid-but-thin icon — as a
    /// shipped installer wearing a blurry 256 scaled down into every slot.
    /// DEVELOPMENT.md lesson 2, and lesson 9: the check that cannot run where
    /// the asset is consumed should run where it is *edited*.
    ///
    /// So this parses the container by hand and asserts the contract
    /// `make-ico.py` claims: six sizes, 32-bit, the small ones as DIBs with the
    /// doubled height an ICO wants, and the 256 as a PNG.
    #[test]
    fn windows_ico_has_all_six_sizes() {
        const ICO: &[u8] = include_bytes!("../../../icons/windows/icon.ico");

        let u16_at = |o: usize| u16::from_le_bytes([ICO[o], ICO[o + 1]]);
        let u32_at = |o: usize| {
            u32::from_le_bytes([ICO[o], ICO[o + 1], ICO[o + 2], ICO[o + 3]]) as usize
        };

        assert_eq!(u16_at(0), 0, "reserved");
        assert_eq!(u16_at(2), 1, "type 1 is an icon; 2 would be a cursor");
        let count = u16_at(4) as usize;
        assert_eq!(count, 6, "16, 32, 48, 64, 128, 256");

        let mut sizes = Vec::new();
        for i in 0..count {
            let e = 6 + 16 * i;
            // A byte cannot hold 256, so the format spells it 0. Every real
            // packer relies on this and every hand-rolled parser forgets it.
            let width = if ICO[e] == 0 { 256 } else { ICO[e] as usize };
            let height = if ICO[e + 1] == 0 { 256 } else { ICO[e + 1] as usize };
            assert_eq!(width, height, "entry {i} is not square");
            assert_eq!(ICO[e + 2], 0, "entry {i} claims a colour palette");
            assert_eq!(u16_at(e + 6), 32, "entry {i} is not 32-bit");

            let len = u32_at(e + 8);
            let off = u32_at(e + 12);
            assert!(
                off + len <= ICO.len(),
                "entry {i} runs {} bytes past the end of the file",
                off + len - ICO.len()
            );
            let payload = &ICO[off..off + len];

            if width == 256 {
                assert_eq!(
                    &payload[..8],
                    b"\x89PNG\r\n\x1a\n",
                    "the 256 should be the PNG itself, not a quarter-megabyte DIB"
                );
            } else {
                // BITMAPINFOHEADER. The height is stored doubled because the
                // struct describes the colour rows *and* a 1-bit AND mask
                // beneath them, even at 32-bit where the alpha channel has
                // already made the mask redundant.
                let dib = |o: usize| {
                    u32::from_le_bytes([
                        payload[o],
                        payload[o + 1],
                        payload[o + 2],
                        payload[o + 3],
                    ]) as usize
                };
                assert_eq!(dib(0), 40, "entry {i} is not a BITMAPINFOHEADER");
                assert_eq!(dib(4), width, "entry {i} DIB width disagrees with the index");
                assert_eq!(
                    dib(8),
                    height * 2,
                    "entry {i} DIB height should be double the icon height"
                );
            }
            sizes.push(width);
        }

        assert_eq!(
            sizes,
            vec![16, 32, 48, 64, 128, 256],
            "ascending, and none of the sizes Windows draws are missing"
        );
    }
}
