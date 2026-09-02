// The TRANSFER group of the Setup panel: ask a box for one of its patterns and
// land it in a slot of this session.
//
// Everything below the button already existed and was tested — `fetch_pattern_kit`
// → `decode_pattern_kit` → `Session::import_pattern` is the path the
// `fetch_pattern_kit` example has driven since the import seam landed. What was
// missing was any way to ask for it from inside the running app, so an import
// could only be performed by running an example from a terminal. This file is
// that button and nothing else: it holds no parsing, no byte offsets and no
// model rules, and every refusal it renders is one `core` or `protocol` made.
//
// **It is per box, and it is in the Setup panel, because a transfer is aimed at
// one box's slot** (PLAN.md §5, and §7 rule 4 — a transfer names one device and
// one slot, never "the session"). The left rail is what you are composing; this
// is what you are composing on.
//
// **Read-only, and structurally so.** The only thing this sends a box is a
// pattern *request* — the gen-2 `0x60`, or the A4's `0x64` since 2026-08-31 —
// through `ElektronDevice`, whose every dump path goes through
// `assert_request_opcode`. There is no write direction to reach even by
// accident (PLAN.md §7 rule 2), so pressing Fetch is the same safety class as
// Identify: it asks, it does not store.
//
// Four decisions worth the words:
//
//   1. **The fetch runs on a worker thread, and only one at a time.** The
//      handshake alone blocks for up to 5 s per request with two retries before
//      the ~111 KB dump even starts, so doing it on the UI thread would freeze
//      the window on any box that is switched off. The ports panel's Identify is
//      the pattern being copied, down to polling the channel and asking for a
//      repaint. One at a time because two dumps in flight would contend for
//      nothing useful — there is one desk and one person at it — and because a
//      single in-flight slot is a state a person can hold in their head.
//   2. **The box is identified first, and the pattern is decoded with the spec
//      of whatever answered** — not with the spec of the device the row belongs
//      to. That is what makes a mis-cabled desk an error rather than a corrupt
//      import: if the row says DN2 and a DT2 answers, `import_pattern` refuses
//      with `NotThisBox` instead of reading the DT2's bytes at the DN2's lane
//      offsets and importing plausible nonsense. The check is `core`'s; this
//      file's only job is to hand it honest evidence.
//   3. **`from` is any of the box's 256 slots; `into` is one of this session's.**
//      They are different spaces and pretending otherwise would cap a fetch at
//      bank A. `PatternRef::wire_index` is the one that decides a slot can be
//      asked for at all.
//   4. **The box's tempo is offered, never taken.** One clock, the studio's
//      (PLAN.md §7 rule 8), so an import does not move the session's tempo — but
//      `ImportReport` carries the box's, and a value that rides all the way home
//      and is then never shown may as well have been dropped at the wire. It is
//      a button, so adopting it is something a person did.
//
// **What has not been verified: a Fetch while the transport is playing.** That
// is the whole of it. Opening a second connection to a port the engine is
// already streaming clock on is expected to be fine on CoreMIDI, but "expected"
// is the honest word.
//
// **This paragraph used to open "no fetch has been started from this panel with
// a box on the other end", and that stopped being true on 2026-08-18** — Neil
// pressed Fetch with a box on the far side and a pattern landed in the slot he
// chose, closing the worker thread, the poll and the import landing somewhere a
// person picked (PLAN.md §9, "Fetch"). The commit that recorded it says it
// struck the claim "everywhere either file still claimed the fetch button or
// the import path had never met a box". It missed this one, and the stale
// sentence outlived the thing it described by twelve days.
//
// Worth a note beyond the correction, because the failure mode is general: a
// caveat is written once, at the moment it is true, and is then never read again
// by anyone who could falsify it. The person who runs the hardware check is
// looking at a box, not at a header comment three files away. `DEVELOPMENT.md`
// lesson 3 is the same shape from the user's side — a panel that lies about what
// is built — and this is its comment-level twin: **an out-of-date caveat does
// not read as out of date, it reads as a gap**, and the next person either
// re-runs work that is already done or, worse, believes the feature is unproven
// and routes around it.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};

use digi_core::a4_transfer::A4ImportReport;
use digi_core::device::{model_for_slug, Device, DeviceModel, PortRef};
use digi_core::import::{Fetched, ImportReport};
use digi_core::device::PatternRoute;
use digi_core::session::PatternRef;
use digi_core::{DeviceId, Session};
use digi_midi::{ElektronDevice, PortBinding};
use digi_protocol::a4_pattern::A4Pattern;
use digi_protocol::pattern::{decode_pattern_kit, PatternKit, Spec};
use eframe::egui::{self, Ui};

use crate::engine::EngineLink;

/// One fetched dump, owned, as the worker hands it back — in whichever format
/// the box that *answered* speaks, which is the point (see the header): the
/// import refuses a cross-format landing, and it can only do that if the bytes
/// arrive labelled by what they are rather than by what the row expected.
enum Dump {
    /// A gen-2 pattern-kit, decoded with the spec of the box that answered.
    ///
    /// Owned rather than a `Fetched`, which borrows and has to: the payload is
    /// ~111 KB and the trig lanes are read straight off it. So the worker
    /// returns the owned pieces and the borrow is taken on the UI thread, for
    /// the two statements it lives.
    Gen2 {
        spec: &'static Spec,
        kit: PatternKit,
        payload: Vec<u8>,
        from: PatternRef,
        /// What the box called itself in the handshake, for the failure line.
        answered: String,
    },
    /// A gen-1 Analog Four pattern, fetched with `0x64`.
    A4 { pattern: A4Pattern, answered: String },
}

/// A fetch in flight. The destination is captured at the press: the row's
/// pickers may move while the dump is crossing, and the answer belongs to the
/// slot it was asked for, not to whatever is selected when it lands.
struct Pending {
    device: DeviceId,
    into: PatternRef,
    rx: Receiver<Result<Dump, String>>,
}

/// What the last fetch on this row did.
enum Outcome {
    Imported { into: PatternRef, report: ImportReport },
    /// An Analog Four landing — its report counts different losses (trigless
    /// trigs, an invented velocity) so it words its own summary.
    ImportedA4 { into: PatternRef, report: A4ImportReport },
    /// Anything that stopped it: a port that would not open, a box that did not
    /// answer, a corrupt dump, a decode, or an import `core` refused.
    Failed(String),
}

/// One box's row: which slot to ask for, where to put it, and the last answer.
struct Row {
    from: PatternRef,
    into: PatternRef,
    /// Whether the destination has been picked by hand. Until it has, it follows
    /// the source, so the common case — pull this slot off the box again — is
    /// one picker rather than two kept in step by the person using them.
    into_pinned: bool,
    outcome: Option<Outcome>,
}

#[derive(Default)]
pub struct TransferPanel {
    rows: HashMap<DeviceId, Row>,
    pending: Option<Pending>,
}

impl TransferPanel {
    /// Whether a fetch is in flight, so the write button can be held off while
    /// one is. One transfer at a time, in either direction: there is one desk and
    /// one person at it, and two connections to the same box — one of them
    /// mid-dump — is a state nothing here has any reason to be good at.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Draw the group. `blocked` holds the button off while the *write* half of
    /// this panel is working. Returns whether the session changed.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        engine: &mut EngineLink,
        blocked: bool,
    ) -> bool {
        let mut edited = self.poll(session);
        if self.pending.is_some() {
            // Nothing wakes the UI thread when a worker finishes, so keep asking
            // while one is out. Same bargain as the ports panel's handshake.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }

        if session.devices.is_empty() {
            ui.weak("No boxes in this session.");
            return edited;
        }

        // Every box in the session, the A4 included: since 2026-08-31 it is
        // fetched by request exactly as the digis are, so the FRONT-PANEL DUMP
        // group this used to filter it out toward is gone. A live-only box
        // still gets a row, with `blocker`'s sentence instead of pickers.
        let devices: Vec<DeviceId> = session.devices.iter().map(|d| d.id).collect();
        let last = devices.len().saturating_sub(1);
        for (position, id) in devices.into_iter().enumerate() {
            edited |= self.device_ui(ui, session, engine, id, blocked);
            if position != last {
                ui.add_space(6.0);
            }
        }
        edited
    }

    /// One box's block. Returns whether the session changed.
    fn device_ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        engine: &mut EngineLink,
        id: DeviceId,
        blocked: bool,
    ) -> bool {
        let Some(device) = session.device(id) else { return false };
        ui.label(egui::RichText::new(&device.name).size(11.0).color(super::TEXT_SECONDARY));

        // Why this box cannot be fetched from, if it cannot. Said before the
        // pickers rather than behind a dead button.
        if let Some(reason) = blocker(device) {
            ui.weak(reason);
            return false;
        }

        let slots = device.patterns.len();
        let model = device.model;
        let into_choices = slot_choices(device);
        // Captured before the row is borrowed mutably: the caution line below
        // needs to know what is about to be overwritten.
        let busy = blocked || self.pending.is_some();
        let in_flight = self.pending.as_ref().is_some_and(|p| p.device == id);
        let opening = session
            .slot_in_scene(session.current_scene, id)
            .unwrap_or_else(|| PatternRef::new(0, 0));

        // The row is created if this is the first time this box has been drawn,
        // then its values are copied *out* before the layout closure: the closure
        // also needs `self` — the Fetch button starts a worker — and one borrow
        // has to be the copy. It is three `Copy` fields, so this is cheaper than
        // the alternative it avoids.
        self.rows.entry(id).or_insert_with(|| Row {
            // The slot this box plays in the scene on screen, so the default
            // fetch lands where the roll is already looking. A default of A01
            // would silently import somewhere nobody is watching.
            from: opening,
            into: opening,
            into_pinned: false,
            outcome: None,
        });
        let (mut from, mut into, mut into_pinned) = self
            .rows
            .get(&id)
            .map(|r| (r.from, r.into, r.into_pinned))
            .expect("inserted a moment ago");
        let mut fetch_clicked = false;

        // `horizontal_wrapped` rather than `horizontal`: the Setup panel is
        // resizable, and at its narrow end this folds onto two lines instead of
        // pushing the Fetch button off the edge.
        ui.horizontal_wrapped(|ui| {
            ui.weak("from");
            let mut picked = from;
            egui::ComboBox::from_id_salt(("fetch-from", id.0))
                .selected_text(egui::RichText::new(from.label()).color(super::TEXT_DIMMER))
                .width(56.0)
                .show_ui(ui, |ui| {
                    for slot in wire_slots(model) {
                        ui.selectable_value(&mut picked, slot, slot.label());
                    }
                });
            if picked != from {
                from = picked;
                if !into_pinned {
                    into = clamp_into(from, slots);
                }
            }

            ui.weak("into");
            let mut picked = into;
            egui::ComboBox::from_id_salt(("fetch-into", id.0))
                .selected_text(egui::RichText::new(into.label()).color(super::TEXT_DIMMER))
                .width(56.0)
                .show_ui(ui, |ui| {
                    for (slot, text) in &into_choices {
                        ui.selectable_value(&mut picked, *slot, text);
                    }
                });
            if picked != into {
                into = picked;
                into_pinned = true;
            }

            ui.add_enabled_ui(!busy, |ui| {
                fetch_clicked = super::colored_button(
                    ui,
                    "FETCH",
                    super::CYAN_FILL,
                    super::CYAN_TEXT,
                    super::CYAN,
                    super::CYAN,
                    super::CYAN_INK,
                )
                .on_hover_text(
                    "Read-only: asks the box for this pattern and loads it into \
                     the slot beside it.\n\nThe slot's ports, channels, mute and \
                     solo are kept — those are this session's, not the box's. \
                     Everything else in it is replaced.",
                )
                .clicked();
            });
        });

        if let Some(row) = self.rows.get_mut(&id) {
            row.from = from;
            row.into = into;
            row.into_pinned = into_pinned;
        }
        if fetch_clicked {
            self.start(id, session);
        }

        // What is about to be lost. An import replaces the slot wholesale, and
        // "I have just overwritten the pattern I spent an hour on" is the one
        // mistake this panel can make that nothing can undo.
        if let Some(device) = session.device(id) {
            let occupied = notes_in(device, into);
            if occupied > 0 {
                ui.colored_label(
                    super::CAUTION,
                    format!("{} has {occupied} note(s) — a fetch replaces them", into.label()),
                );
            }
        }

        if in_flight {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("fetching …");
            });
            return false;
        }

        self.outcome_ui(ui, session, engine, id)
    }

    /// The last answer on this row, and the one thing it offers. Returns whether
    /// the session changed.
    fn outcome_ui(
        &mut self,
        ui: &mut Ui,
        session: &mut Session,
        engine: &mut EngineLink,
        id: DeviceId,
    ) -> bool {
        let Some(row) = self.rows.get(&id) else { return false };
        let mut edited = false;
        match &row.outcome {
            Some(Outcome::Imported { into, report }) => {
                ui.colored_label(egui::Color32::LIGHT_GREEN, summary(*into, report));
                if report.trimmed_past_len > 0 {
                    ui.weak(format!(
                        "{} trig(s) sat past their track's LEN and were dropped — the \
                         box stores those and does not play them either",
                        report.trimmed_past_len
                    ));
                }
                // Said out loud rather than left for the strip to look broken
                // about: these lanes arrived and cannot be edited, because we
                // cannot say which knob they are. They play back untouched and
                // they survive a write-back byte-exact.
                if report.unnamed_plock_lanes > 0 {
                    ui.weak(format!(
                        "{} of them automate a parameter this app cannot name —                          carried and played, but read-only",
                        report.unnamed_plock_lanes
                    ));
                }
                // Rule 8: this app masters one clock and the box's tempo is not
                // it. Offered, so the number is not simply lost.
                let bpm = report.box_tempo_bpm;
                if (bpm - session.tempo_bpm).abs() >= 0.05 {
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(format!("box was at {bpm:.1} bpm"));
                        if ui
                            .small_button(format!("Use {bpm:.1}"))
                            .on_hover_text(
                                "Set this session's tempo to the one stored in the \
                                 fetched pattern. The session masters the clock, so \
                                 an import never does this on its own.",
                            )
                            .clicked()
                        {
                            session.tempo_bpm = bpm;
                            engine.set_tempo(bpm);
                            edited = true;
                        }
                    });
                }
                // The answer to "why did nothing change in the roll".
                let playing = session.slot_in_scene(session.current_scene, id);
                if playing.is_some_and(|p| p != *into) {
                    ui.weak(format!(
                        "This box is playing {} in the scene on screen",
                        playing.expect("checked").label()
                    ));
                }
            }
            Some(Outcome::ImportedA4 { into, report }) => {
                ui.colored_label(egui::Color32::LIGHT_GREEN, a4_summary(*into, report));
                if report.trigless_dropped > 0 {
                    ui.weak(format!(
                        "{} trigless trig(s) were not imported — this app holds notes, and a \
                         trigless trig is a trig with no note",
                        report.trigless_dropped
                    ));
                }
                // Velocity, length, micro timing and the trig condition were
                // all a CAUTION line here on the morning of 2026-09-01: none of
                // them was in the mapped format, and every note came in at this
                // app's default. All four were measured on the box that day, so
                // the line is gone rather than softened — there is nothing left
                // for it to warn about.
                //
                // What replaces it is not a warning at all. A pattern with
                // conditions on it is worth *mentioning*, because the roll draws
                // them in the trig lanes rather than on the notes and a person
                // may not look there.
                if report.conditions > 0 {
                    ui.weak(format!(
                        "{} trig(s) carry a probability, fill or condition — shown in the trig \
                         lanes under the roll",
                        report.conditions
                    ));
                }
                // The A4's chords: ARP NO2–NO4 offsets, drawn as the upper notes
                // of a chord. Worth a line because whether they *sound* is the
                // kit's business — a polyphonic kit with the arp off plays the
                // chord, a mono kit plays the root alone — and the roll cannot
                // show that.
                if report.chord_notes > 0 {
                    ui.weak(format!(
                        "{} note(s) came in as ARP NO2–NO4 offsets and are drawn as chords — they \
                         sound as chords on a polyphonic kit with the arp MOD off",
                        report.chord_notes
                    ));
                }
                if report.chord_notes_dropped > 0 {
                    ui.weak(format!(
                        "{} ARP offset(s) were not drawn — off the keyboard, or doubling a pitch \
                         the step already holds",
                        report.chord_notes_dropped
                    ));
                }
                // This one *is* a caution, and it is about the format rather
                // than the pattern: the menu's length rests on four labels read
                // off the box, so a byte past its end means the table is short.
                if report.conditions_off_the_menu > 0 {
                    ui.colored_label(
                        super::CAUTION,
                        format!(
                            "{} trig(s) carry a condition past the end of the A4's mapped TRC \
                             menu — they came in without one, and the menu needs re-measuring.",
                            report.conditions_off_the_menu
                        ),
                    );
                }
                // The answer to "why did nothing change in the roll" — the same
                // line the gen-2 arm draws.
                let playing = session.slot_in_scene(session.current_scene, id);
                if playing.is_some_and(|p| p != *into) {
                    ui.weak(format!(
                        "This box is playing {} in the scene on screen",
                        playing.expect("checked").label()
                    ));
                }
            }
            Some(Outcome::Failed(e)) => {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            None => {}
        }
        edited
    }

    /// Put a fetch out. Does nothing if one is already in flight, or if the row
    /// names a slot the wire cannot carry.
    fn start(&mut self, id: DeviceId, session: &Session) {
        if self.pending.is_some() {
            return;
        }
        let Some(device) = session.device(id) else { return };
        let (Some(input), Some(output)) = (device.io.input.clone(), device.io.output.clone())
        else {
            return;
        };
        let Some(row) = self.rows.get_mut(&id) else { return };
        let (from, into) = (row.from, row.into);
        let Some(index) = from.wire_index() else {
            row.outcome = Some(Outcome::Failed(format!(
                "{} is past the last slot a dump request can name",
                from.label()
            )));
            return;
        };
        row.outcome = None;

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch(&binding(&input), &binding(&output), from, index));
        });
        self.pending = Some(Pending { device: id, into, rx });
    }

    /// Take a landed dump and import it. Returns whether the session changed —
    /// which is what gets the new pattern down the channel to the engine.
    fn poll(&mut self, session: &mut Session) -> bool {
        let Some(pending) = &self.pending else { return false };
        let Ok(result) = pending.rx.try_recv() else { return false };
        let (id, into) = (pending.device, pending.into);
        self.pending = None;

        let mut edited = false;
        let outcome = match result {
            Ok(Dump::Gen2 { spec, kit, payload, from, answered }) => {
                // The borrow the header talks about, taken for exactly as long
                // as the import needs it.
                let fetched = Fetched { spec, kit: &kit, payload: &payload, from };
                match session.import_pattern(id, into, &fetched) {
                    Ok(report) => {
                        edited = true;
                        Outcome::Imported { into, report }
                    }
                    // `NotThisBox` reads best with the box that actually spoke
                    // named, since the whole point of the refusal is that it is
                    // not the one this row belongs to.
                    Err(e) => Outcome::Failed(format!("{e} ({answered} answered)")),
                }
            }
            Ok(Dump::A4 { pattern, answered }) => {
                match session.import_a4_pattern(id, into, &pattern) {
                    Ok(report) => {
                        edited = true;
                        Outcome::ImportedA4 { into, report }
                    }
                    Err(e) => Outcome::Failed(format!("{e} ({answered} answered)")),
                }
            }
            Err(e) => Outcome::Failed(e),
        };
        if let Some(row) = self.rows.get_mut(&id) {
            row.outcome = Some(outcome);
        }
        edited
    }
}

/// The blocking half, on a worker thread: open, identify, ask, decode.
///
/// Every error becomes a string here rather than crossing the channel as a type,
/// because five error types from four crates arrive at one label. What matters
/// is that each of them is the box's or the protocol's own words.
fn fetch(
    input: &PortBinding,
    output: &PortBinding,
    from: PatternRef,
    index: u8,
) -> Result<Dump, String> {
    let mut device = ElektronDevice::open(input, output).map_err(|e| e.to_string())?;
    // Not optional: `fetch_pattern_kit` needs the family byte off the identity,
    // and it is also the only evidence of *which* box is on these ports.
    let identity = device.identify().map_err(|e| e.to_string())?;
    let model = model_for_slug(&identity.slug)
        .ok_or_else(|| format!("{} is not a box this build knows how to decode", identity.name))?;

    // Decoded in the format of the box that *answered*, not the row's — the
    // header's decision 2. An A4 answering on a digi's row (or the other way
    // round) therefore parses cleanly here and is refused by the import's own
    // `NotThisBox`, with the answering box named.
    if model.pattern_route() == PatternRoute::RequestGen1 {
        // The wire re-verified the checksum and matched the echoed index, so
        // the pair below is exactly what `parse_pattern` would have produced;
        // the length is `import_a4_pattern`'s to refuse.
        let payload = device.fetch_pattern_kit(index).map_err(|e| e.to_string())?;
        return Ok(Dump::A4 {
            pattern: A4Pattern { slot: index, payload },
            answered: identity.name,
        });
    }
    let spec = model
        .spec()
        .ok_or_else(|| format!("{} plays over MIDI but has no pattern dumps", model.display))?;

    let payload = device.fetch_pattern_kit(index).map_err(|e| e.to_string())?;
    let kit = decode_pattern_kit(spec, &payload).map_err(|e| e.to_string())?;
    Ok(Dump::Gen2 { spec, kit, payload, from, answered: identity.name })
}

pub(crate) fn binding(port: &PortRef) -> PortBinding {
    PortBinding { id: port.id.clone(), name: port.name.clone() }
}

/// Why this box cannot be fetched from, or `None` if it can.
///
/// Both ends are needed and they fail for different reasons, so they are named
/// separately: the request goes out on the output and the ~111 KB answer comes
/// back on the input, and a desk with only one of them wired is a real and
/// confusing state.
fn blocker(device: &Device) -> Option<String> {
    // The route, not `can_sysex`: that field means "has a gen-2 Spec", and the
    // A4 fetches perfectly well without one. `PatternRoute::transfers` is the
    // question actually being asked here.
    if !device.pattern_route().transfers() {
        return Some(format!(
            "{} plays over MIDI but has no pattern dumps to fetch",
            device.model.display
        ));
    }
    match (&device.io.input, &device.io.output) {
        (Some(_), Some(_)) => None,
        (None, None) => Some("No ports set — pick an in and an out for this box above".into()),
        (None, Some(_)) => Some("No in port — the dump comes back on the input".into()),
        (Some(_), None) => Some("No out port — the request goes out on it".into()),
    }
}

/// Every slot a dump request can name on this box: sixteen banks of sixteen on
/// a digi, eight on an Analog Four. The model's own number
/// ([`DeviceModel::wire_slots`]), because a picker offering I01 to a box whose
/// banks stop at H would be offering a request nobody has ever seen answered.
pub(crate) fn wire_slots(model: &DeviceModel) -> impl Iterator<Item = PatternRef> {
    (0..model.wire_slots).map(PatternRef::from_slot)
}

/// This session's slots for one box, each saying what is already in it.
///
/// The note count is the fact that matters at both ends of a transfer — a slot
/// with notes in it is one an import is about to replace, and the one a write has
/// something to send from — and the pattern's own name is shown when it has
/// stopped being the slot label, which is exactly when it came off a box. Shared
/// with `ui::write` rather than copied, so the two pickers cannot come to
/// describe the same slot differently.
pub(crate) fn slot_choices(device: &Device) -> Vec<(PatternRef, String)> {
    (0..device.patterns.len())
        .map(|i| {
            let slot = PatternRef::from_slot(i);
            let label = slot.label();
            let text = match device.pattern(i) {
                Some(p) => {
                    let notes: usize = p.tracks().iter().map(|t| t.notes.len()).sum();
                    match (p.name == label, notes) {
                        (true, 0) => label.clone(),
                        (true, n) => format!("{label}  ({n} notes)"),
                        (false, 0) => format!("{label}  {}", p.name),
                        (false, n) => format!("{label}  {}  ({n} notes)", p.name),
                    }
                }
                None => label.clone(),
            };
            (slot, text)
        })
        .collect()
}

/// Where a source slot lands when the destination is still following it.
///
/// The box has 256 slots and this session has however many the device was made
/// with, so a fetch from D07 cannot land in D07 of a session that stops at A16.
/// It lands in the last slot there is rather than refusing, because the
/// destination picker is right there and the alternative is a disabled button
/// with no explanation.
fn clamp_into(from: PatternRef, slots: usize) -> PatternRef {
    if slots == 0 {
        return from;
    }
    PatternRef::from_slot(from.slot().min(slots - 1))
}

/// Notes already sitting in a slot of this session.
fn notes_in(device: &Device, slot: PatternRef) -> usize {
    device
        .pattern(slot.slot())
        .map(|p| p.tracks().iter().map(|t| t.notes.len()).sum())
        .unwrap_or(0)
}

/// The one-line answer to "what came across".
///
/// The box's own name for the pattern is included when it has one: a pattern
/// that has never been named on the box carries an empty string, and `core`
/// falls back to the slot label for the *pattern*, which is the right answer
/// there and a redundant one here.
fn summary(into: PatternRef, report: &ImportReport) -> String {
    let named = match report.pattern_name.trim() {
        "" => String::new(),
        name => format!(" {name}"),
    };
    let lanes = match report.plock_lanes {
        0 => String::new(),
        n => format!(", {n} p-lock lane(s)"),
    };
    format!(
        "Into {}{} · {} note(s) on {} track(s), swing {}%{}",
        into.label(),
        named,
        report.notes,
        report.tracks_with_notes,
        report.swing,
        lanes,
    )
}

/// [`summary`]'s A4 twin. Still smaller because neither swing nor box tempo is
/// in the mapped format, so a line claiming them would be inventing facts the
/// wire never carried — but **p-lock lanes are, since 2026-09-01**, and they are
/// counted here for the same reason the gen-2 line counts them: an import that
/// brought 8 KB of automation across and said nothing about it reads as one that
/// did not.
///
/// Trigless lanes are named separately when there are any. They arrive
/// read-only — this model has no trigless lock — and a lane the roll cannot let
/// you edit is worth saying so about at the moment it lands, rather than leaving
/// someone to discover it by trying.
fn a4_summary(into: PatternRef, report: &A4ImportReport) -> String {
    let lanes = match (report.plock_lanes, report.trigless_plock_lanes) {
        (0, _) => String::new(),
        (n, 0) => format!(", {n} p-lock lane(s)"),
        (n, t) => format!(", {n} p-lock lane(s) ({t} trigless, read-only)"),
    };
    format!(
        "Into {} · {} note(s) on {} track(s){}",
        into.label(),
        report.notes,
        report.tracks_with_notes,
        lanes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::device::{DeviceIo, DT2};
    use digi_core::model::Note;

    fn port(name: &str) -> PortRef {
        PortRef { id: name.into(), name: name.into() }
    }

    fn wired() -> Device {
        let mut device = Device::new("DT2", &DT2, 16);
        device.io = DeviceIo {
            input: Some(port("in")),
            output: Some(port("out")),
            ..DeviceIo::default()
        };
        device
    }

    #[test]
    fn a_box_with_both_ports_can_be_fetched_from() {
        assert_eq!(blocker(&wired()), None);
    }

    #[test]
    fn each_missing_end_is_named_rather_than_lumped_together() {
        // The two ends fail differently and a desk is often half-wired, so
        // "no ports" is not an acceptable answer to either one on its own.
        let mut device = wired();
        device.io.input = None;
        assert!(blocker(&device).unwrap().contains("in port"));

        let mut device = wired();
        device.io.output = None;
        assert!(blocker(&device).unwrap().contains("out port"));

        let mut device = wired();
        device.io = DeviceIo::default();
        let both = blocker(&device).unwrap();
        assert!(both.contains("in and an out"), "{both}");
    }

    #[test]
    fn the_wire_offers_every_slot_the_box_has_and_not_one_more() {
        let slots: Vec<PatternRef> = wire_slots(&DT2).collect();
        assert_eq!(slots.len(), 256);
        assert_eq!(slots[0].label(), "A01");
        assert_eq!(slots[255].label(), "P16");
        assert!(slots.iter().all(|s| s.wire_index().is_some()));

        // The A4's banks stop at H — its `0x64` index is linear 0–127, and a
        // picker offering I01 would be offering a request nobody has ever seen
        // answered.
        let slots: Vec<PatternRef> = wire_slots(&digi_core::device::A4).collect();
        assert_eq!(slots.len(), 128);
        assert_eq!(slots[127].label(), "H16");
    }

    #[test]
    fn a_destination_says_what_is_already_in_it() {
        // The warning that matters: this is the picker where an hour's work gets
        // replaced, so an occupied slot has to look different from an empty one.
        let mut device = wired();
        let pattern = device.pattern_mut(2).unwrap();
        pattern.track_mut(0).unwrap().notes = vec![Note::new(0.0, 60, 1.0, 100, 0.0)];

        let choices = slot_choices(&device);
        assert_eq!(choices.len(), 16);
        assert_eq!(choices[0].1, "A01", "an untouched slot is just its label");
        assert_eq!(choices[2].1, "A03  (1 notes)");
        assert_eq!(choices[2].0, PatternRef::new(0, 2));
    }

    #[test]
    fn a_slot_that_has_been_named_by_a_box_shows_that_name() {
        let mut device = wired();
        device.pattern_mut(1).unwrap().name = "BD ROOM".into();
        assert_eq!(slot_choices(&device)[1].1, "A02  BD ROOM");
    }

    #[test]
    fn a_destination_following_the_source_stops_at_the_last_slot_this_session_has() {
        // The box has sixteen banks and the session has one, so a fetch from D07
        // has nowhere of the same name to land.
        assert_eq!(clamp_into(PatternRef::new(0, 4), 16), PatternRef::new(0, 4));
        assert_eq!(clamp_into(PatternRef::new(3, 6), 16), PatternRef::new(0, 15));
        assert_eq!(clamp_into(PatternRef::new(3, 6), 64), PatternRef::new(3, 6));
        // A device with no slots at all cannot be imported into; the value is
        // never used, and it must not panic on the way to finding that out.
        assert_eq!(clamp_into(PatternRef::new(1, 0), 0), PatternRef::new(1, 0));
    }

    #[test]
    fn the_summary_names_the_slot_it_landed_in_and_what_arrived() {
        let report = ImportReport {
            pattern_name: "BD ROOM".into(),
            notes: 42,
            tracks_with_notes: 5,
            swing: 53,
            box_tempo_bpm: 124.0,
            trimmed_past_len: 0,
            plock_lanes: 0,
            unnamed_plock_lanes: 0,
        };
        assert_eq!(
            summary(PatternRef::new(0, 2), &report),
            "Into A03 BD ROOM · 42 note(s) on 5 track(s), swing 53%"
        );

        // Lanes are named only when there are any: most patterns have none, and
        // a trailing ", 0 p-lock lane(s)" on every import is noise.
        let with_lanes = ImportReport { plock_lanes: 4, ..report.clone() };
        assert_eq!(
            summary(PatternRef::new(0, 2), &with_lanes),
            "Into A03 BD ROOM · 42 note(s) on 5 track(s), swing 53%, 4 p-lock lane(s)"
        );

        // An unnamed pattern is the common case on a box, and it must not leave
        // a double space where a name would have been.
        let report = ImportReport { pattern_name: String::new(), ..report };
        assert_eq!(
            summary(PatternRef::new(0, 2), &report),
            "Into A03 · 42 note(s) on 5 track(s), swing 53%"
        );
    }
}
