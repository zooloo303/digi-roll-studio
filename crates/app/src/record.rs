// The glue between the engine's placed events and the model — PLAN.md §12.4.4.
//
// Stage 3 of three. The rules are all in `core::record`, the arithmetic is all
// in `engine::record`, and what is left here is the part that needs to know
// about a selection, a history and a console: which track a take lands on, when
// a take begins and ends, and what it says when it is over.
//
// **Ticked once per frame from the shell, before the workspace draws.** Order
// matters twice. Before the panels, so a note played this frame is already in
// the track when the roll paints it — otherwise every recorded note appears a
// frame late, which at 60 Hz is visible on a fast run. And after
// `transport::shortcuts`, so pressing R while stopped can arm *and* start the
// transport in one frame rather than two.
//
// # The four things a frame does
//
// 1. Re-point thru and re-state REC. Both are comparisons in `EngineLink` and
//    cost a command only when something moved.
// 2. Drain what the engine placed, opening a take on the first one.
// 3. Notice a take that has ended, close it, commit the undo step, post the
//    report.
// 4. Nothing else. This file holds no rule about which note wins a step and no
//    arithmetic about which step a note is on.

use digi_core::device::DeviceId;
use digi_core::history::{Content, History};
use digi_core::record::{PlacedEvent, Take, TakeOptions};
use digi_core::Session;
use eframe::egui;

use crate::engine::EngineLink;
use crate::ui::console;
use crate::ui::tracks::Selection;

/// A take in progress, and everything that would end it.
struct OpenTake {
    take: Take,
    /// The track it is writing to. A selection change closes it and opens
    /// another — §9 decision 3.
    target: (DeviceId, usize),
    /// How the console will name that track.
    label: String,
    /// The scene that was sounding when it opened. A scene change swaps the
    /// pattern under the track, so a take that carried on would be writing into
    /// music the player is no longer hearing.
    scene: usize,
    /// Whether the "routed nowhere" line has already been said. Once per take:
    /// it is worth saying, and worth saying once.
    warned_no_port: bool,
}

/// Recording's UI-thread half.
#[derive(Default)]
pub struct Recorder {
    armed: bool,
    quantize: bool,
    open: Option<OpenTake>,
    /// Reused frame to frame, so a take that is running allocates nothing on
    /// this thread.
    scratch: Vec<PlacedEvent>,
    /// Whether the transport was running last frame, so a stop can be noticed
    /// as the *edge* it is. Polling `is_playing()` alone cannot tell "STOP was
    /// pressed" from "PLAY has not taken effect yet", and REC-while-stopped
    /// starts the transport a frame or two before the engine says so.
    was_playing: bool,
    /// The last record-input failure said out loud, so it is said **once** and
    /// not once per frame. `EngineLink` holds the text and the Setup row shows
    /// it live; the console is where you find it if the panel is closed.
    reported_failure: Option<String>,
}

impl Recorder {
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Arm or disarm. Arming alone captures nothing — the transport also has to
    /// be running (decision 5) — so this never touches the transport itself;
    /// the REC button and the `R` key do that, because "press REC while stopped
    /// and it plays from the top" is a *control's* behaviour rather than a
    /// recorder's.
    pub fn set_armed(&mut self, armed: bool) {
        self.armed = armed;
    }

    pub fn quantize(&self) -> bool {
        self.quantize
    }

    pub fn set_quantize(&mut self, on: bool) {
        self.quantize = on;
    }

    /// Whether a take is running, which is what holds the shell's undo step
    /// open — a take is one step the way a drag is one step.
    pub fn take_open(&self) -> bool {
        self.open.is_some()
    }

    /// One frame. Returns whether the session changed.
    ///
    /// `before` is the shell's snapshot of the content as of the start of this
    /// frame, and it is `Option` for the shell's own reason: there is none on
    /// the second and later frames of a gesture, because a step is already
    /// open. A take opening on such a frame therefore joins the step that is
    /// already there, which is right — nothing is lost, and `History::begin` is
    /// a no-op when one is open anyway.
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        engine: &mut EngineLink,
        session: &mut Session,
        selection: Selection,
        history: &mut History,
        before: Option<&Content>,
    ) -> bool {
        let playing = engine.is_playing();
        let target = resolve_target(session, selection);

        // §5.3: a record input that would not open says so once, wherever you
        // are looking. A rebuild that fixes it clears the memo, so a keyboard
        // unplugged and plugged back in reports again.
        let failure = engine.input_failure().map(str::to_owned);
        if failure != self.reported_failure {
            if let Some(line) = &failure {
                console::post(ctx, format!("Record input would not open — {line}"));
            }
            self.reported_failure = failure;
        }

        // 1. Thru follows the selection, always — armed or not, playing or
        //    stopped. Selecting a track is how you choose what the keyboard
        //    sounds, and that is decision 2.
        engine.set_monitor(session, target);
        // A take can be armed with nothing selected. The engine is told so
        // rather than being left pointed at the last track, which would record
        // onto a track nobody is looking at.
        engine.set_record(self.armed, target, self.quantize);

        // 3a. Has the take that is open ended? Checked before this frame's
        //     events are drained, so the last note of a take lands in that take
        //     and not in the next one.
        let stopped = self.was_playing && !playing;
        self.was_playing = playing;
        let ended = match &self.open {
            Some(open) => {
                stopped
                    || !self.armed
                    || Some(open.target) != target
                    || open.scene != session.current_scene
            }
            None => false,
        };
        let mut edited = false;
        if ended {
            edited |= self.finish(ctx, session, history);
        }
        // **STOP disarms** — §9 decision 1. A stray key after stopping must not
        // land in the next PLAY. The box leaves REC lit and one press re-arms;
        // this is the one place the design knowingly differs from it.
        if stopped {
            self.armed = false;
            engine.set_record(false, target, self.quantize);
        }

        // 2. Everything the engine placed since the last frame.
        self.scratch.clear();
        engine.drain_placed(&mut self.scratch);
        if self.scratch.is_empty() {
            return edited;
        }
        let Some(target) = target else {
            // Placed events with nowhere to put them. Only reachable if the
            // selection moved between the engine placing them and this frame
            // draining them, which is a real race and not an error — but it is
            // counted rather than silently dropped.
            if let Some(open) = &mut self.open {
                open.take.report.dropped_no_target += self.scratch.len();
            }
            return edited;
        };

        // Opening a take is the first placed event of one, not REC being
        // pressed: a take with nothing in it is not a take, gets no undo step
        // and gets no console line.
        if self.open.is_none() {
            if let Some(before) = before {
                history.begin(before.clone());
            }
            self.open = Some(OpenTake {
                take: Take::new(),
                target,
                label: track_label(session, target),
                scene: session.current_scene,
                warned_no_port: false,
            });
        }
        let Some(open) = &mut self.open else {
            return edited;
        };

        // 2b. A track routed nowhere still records — you can play the keyboard
        //     into a track you have not cabled yet — but silently recording
        //     something you cannot hear is the surprise this line exists to
        //     prevent.
        if !open.warned_no_port && engine.resolve_track_port(session, target.0, target.1).is_none()
        {
            open.warned_no_port = true;
            console::post(
                ctx,
                format!(
                    "REC: {} is routed nowhere — recording anyway, but you will not hear it",
                    open.label
                ),
            );
        }

        let Some(options) = take_options(session, target, self.quantize) else {
            return edited;
        };
        let Some(track) = target_track(session, target) else {
            return edited;
        };
        for event in self.scratch.drain(..) {
            edited |= open.take.push(event, track, &options);
        }
        edited
    }

    /// Close the open take: commit its undo step and say what it did.
    fn finish(&mut self, ctx: &egui::Context, session: &mut Session, history: &mut History) -> bool {
        let Some(open) = self.open.take() else {
            return false;
        };
        let report = match target_track(session, open.target) {
            Some(track) => open.take.close(track),
            // The track went away under the take — a box removed in Setup mid-
            // recording. The report is still worth having.
            None => {
                let mut scratch =
                    digi_core::model::Track::new(0, digi_core::model::TrackKind::Audio);
                open.take.close(&mut scratch)
            }
        };
        if report.is_empty() {
            // Nothing landed, so there is nothing to undo and nothing to say.
            // The step the shell may have opened is dropped by `commit`'s own
            // unchanged-content rule the moment the pointer is up.
            return false;
        }
        // The whole take is one step, closed here rather than by the shell's
        // per-frame `commit` — which `take_open()` is holding off for exactly
        // this reason.
        history.commit(session);
        console::post(ctx, report.line(&open.label));
        // **`false`, and it is not an oversight.** The notes were written on the
        // frames they arrived on and were reported as edits then. Closing a take
        // moves nothing; saying it did would cost a whole-session snapshot down
        // the channel for a button release.
        false
    }
}

/// The `(device, track)` the roll is editing, as the engine names it — `None`
/// when the selection points at nothing, which is an empty desk or a device
/// index left over from a removed box.
fn resolve_target(session: &Session, selection: Selection) -> Option<(DeviceId, usize)> {
    let device = session.devices.get(selection.device)?;
    // Resolved through the pattern rather than through the model's track count,
    // so a selection naming a track the sounding scene does not have is `None`
    // here and not a panic later.
    session.current_pattern(device.id)?.track(selection.track)?;
    Some((device.id, selection.track))
}

fn target_track(session: &mut Session, target: (DeviceId, usize)) -> Option<&mut digi_core::Track> {
    let (device, track) = target;
    let slot = session.slot_in_scene(session.current_scene, device)?.slot();
    session.device_mut(device)?.pattern_mut(slot)?.track_mut(track)
}

/// Everything the rules need that lives outside them: the box's polyphony cap,
/// the track's length, and how long one of its steps is at this tempo and SCALE.
fn take_options(
    session: &Session,
    target: (DeviceId, usize),
    quantize: bool,
) -> Option<TakeOptions> {
    let (device, index) = target;
    let model = session.device(device)?.model;
    let track = session.current_pattern(device)?.track(index)?;
    Some(TakeOptions {
        notes_per_trig: model.notes_per_trig as usize,
        max_steps: track.length_steps.max(1) as f64,
        quantize,
        step_secs: digi_engine::time::track_step_seconds(session.tempo_bpm, track.scale),
    })
}

/// How the console names a track: the box's name and the track's, `DT2 T3`.
fn track_label(session: &Session, target: (DeviceId, usize)) -> String {
    let (device, index) = target;
    let name = session
        .device(device)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| String::from("?"));
    format!("{name} T{}", index + 1)
}

/// Whether the selection names a track a take could land on. The REC button
/// reads this to say why it is disabled.
pub fn has_target(session: &Session, selection: Selection) -> bool {
    resolve_target(session, selection).is_some()
}

/// The name the tooltip and the console use for the selected track.
pub fn selected_label(session: &Session, selection: Selection) -> Option<String> {
    resolve_target(session, selection).map(|t| track_label(session, t))
}

/// Whether the selected track will actually be heard. Used by the REC tooltip,
/// so "you will not hear it" is on screen before the take rather than only
/// after it.
pub fn selection_is_routed(engine: &EngineLink, session: &Session, selection: Selection) -> bool {
    resolve_target(session, selection)
        .and_then(|(d, t)| engine.resolve_track_port(session, d, t))
        .is_some()
}
