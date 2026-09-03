// The curated p-lock parameter model.
//
// Ported from `js/elektron/params.js`, `js/elektron/param-tables.js` and the two
// per-box tables `js/elektron/{dt2,dn2}/params.js`. A parameter has **two
// independent mappings**, and keeping them apart is the whole design of this
// file:
//
//   `midi`    how to *hear* it — the CC and NRPN numbers from the boxes' own
//             MIDI implementation appendices (DT2 Appendix B, DN2 Appendix C).
//             Public, published, and confirmable on the box in seconds: send a
//             value and watch the parameter move on screen.
//
//   `plock`   how to *store* it — the `paramId` byte in the pattern's p-lock
//             lane pool, plus the scaling between a display value and the lane's
//             uint16. **Not published anywhere** and different on each box — 74
//             is overdrive on a DT2 and filter frequency on a DN2. Measured on
//             hardware by digi-roll's Phase 0 captures of 2026-08-04, whose
//             fixtures are committed here as
//             `tests/fixtures/digitakt2-A01-plock-*.syx` and
//             `digitone2-A01-plock-final-2026-08-04.syx`.
//
// The split earns its keep in both directions. A parameter with `midi` but no
// `plock` can be **drawn and auditioned** but never written to a pattern, which
// is the safety net that keeps a missing measurement from becoming a wrong byte.
// A lane with a `plock` id we do not recognise can be **carried byte-exact**
// without being drawn as anything we claim to understand.
//
// A lane is identified by this table's canonical `name` when the app authored
// it, and by the raw `paramId` byte when it came off a box. `name` is also what
// cross-device copy translates by — two parameters sharing a name are the same
// knob, and their paramIds never agree between boxes.
//
// ## The value axis
//
// Display values here are on the **MIDI value axis, 0–127** — what the box
// receives, and what NRPN's high byte carries. That is a deliberate choice over
// the labels the box prints on screen (`L32`, `-8.00`): those differ per
// parameter and per machine, and none of them is documented in a form worth
// trusting. 0–127 is honest about what is being sent, and it is the axis every
// measured p-lock scaling maps from (stored word = display × 256). `bipolar`
// marks the parameters whose 64 is the box's centre, so their bars can be drawn
// from the middle rather than the floor.
//
// ## What this module deliberately does not do
//
// It reads and writes no bytes. The p-lock **lane pool** inside a pattern
// payload is `js/elektron/plocks.js` and is not ported yet; when it is, this is
// the table it must consult to turn a `paramId` into something nameable. Until
// then the value functions here are exercised only by the audition path.

/// The bottom of the display axis. See the header: display values are MIDI
/// values, not the labels the box prints.
pub const MIDI_MIN: i32 = 0;

/// The top of the display axis.
pub const MIDI_MAX: i32 = 127;

/// The largest word a p-lock lane can hold. `0xFFFF` is the lane's "nothing
/// stored on this step" sentinel, so a real value stops one short — which is
/// also the ceiling a raw, uncurated lane is drawn against.
///
/// The lane pool that owns this sentinel is unported (`js/elektron/plocks.js`);
/// this constant is here because [`describe_param`] needs the ceiling to draw a
/// lane it cannot name.
pub const RAW_VALUE_MAX: u16 = 0xFFFE;

/// The device kinds with a curated table. For a box with a `Spec` the key is
/// spelled as [`crate::pattern::Spec::device`] spells it; `A4` has no spec —
/// it is live-only — so its key is the model key, which the two spellings
/// agree on for every box that has both.
pub const DEVICE_KINDS: &[&str] = &["DT2", "DN2", "A4"];

/// How to *hear* a parameter. Any of the three may be absent: the DN2's whole
/// LFO3 has no CC at all, and the DT2's appendix gives no NRPN for the FX page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MidiMap {
    pub cc: Option<u8>,
    /// The low half of a 14-bit CC pair, where the appendix gives one. Sending
    /// `cc` alone on such a parameter silently drops the bottom seven bits.
    pub cc_lsb: Option<u8>,
    /// `(msb, lsb)`.
    pub nrpn: Option<(u8, u8)>,
}

/// The two shapes a measured p-lock scaling takes.
///
/// Phase 0 measured every curated parameter on both boxes as
/// [`PLockScaling::Scaled`] by 256. [`PLockScaling::Plain`] stays for the next
/// parameter that turns out differently, and is what a raw lane passes through.
/// For a new entry the method stands: lock known min/centre/max values and read
/// the words back. **Do not assume; capture.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PLockScaling {
    /// The display value *is* the stored word.
    Plain,
    /// The stored word is the display value scaled by a constant — the shape a
    /// high-resolution parameter takes when the lane holds more than 7 bits.
    Scaled(u16),
}

/// How to *store* a parameter: its `paramId` byte and its scaling. Only ever
/// present once measured on hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PLock {
    /// The box's own page-ordered parameter index — **not** the NRPN LSB. A
    /// `u16` rather than a `u8` because a lane read off a box arrives as one and
    /// need not be a value any table holds; curated entries are validated to
    /// 0–254, since `0xFF` marks a free lane.
    pub id: u16,
    pub scaling: PLockScaling,
}

/// A measured scaling where the word is the display value.
pub const fn plain_plock(id: u16) -> PLock {
    PLock { id, scaling: PLockScaling::Plain }
}

/// A measured scaling where the word is the display value times `factor`.
pub const fn scaled_plock(id: u16, factor: u16) -> PLock {
    PLock { id, scaling: PLockScaling::Scaled(factor) }
}

/// One curated parameter. Everything in the two tables below is one of these,
/// and everything in them is `'static` — they are hardware facts, not state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param {
    /// Canonical cross-device key, e.g. `filter.cutoff`. Two parameters sharing
    /// a name on different boxes are the same knob.
    pub name: &'static str,
    /// What the box's own UI calls it, e.g. `FLTR CUTOFF`.
    pub label: &'static str,
    /// Gutter label for the lane strip, which is 52 px wide.
    pub short: &'static str,
    /// 64 is the centre of the range, so draw bars from the middle.
    pub bipolar: bool,
    pub midi: MidiMap,
    /// `None` until the paramId and scaling have been measured on hardware.
    pub plock: Option<PLock>,
}

impl Param {
    /// Can it be *heard*? A CC or an NRPN from the published chart is enough.
    pub const fn auditable(&self) -> bool {
        self.midi.cc.is_some() || self.midi.nrpn.is_some()
    }

    /// Can it be *written into a pattern*? Only once its paramId is measured.
    pub const fn writable(&self) -> bool {
        self.plock.is_some()
    }

    /// The JS constructor throws on each of these; a `const` table cannot, so
    /// the check moved here and a test walks both tables through it. Same three
    /// rules: a parameter needs a name and a label, needs to do *something*, and
    /// cannot claim the free-lane sentinel as its paramId.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() || self.label.is_empty() {
            return Err("a p-lock param needs a name and a label".into());
        }
        if !self.auditable() && !self.writable() {
            return Err(format!(
                "p-lock param {}: no CC, no NRPN and no p-lock id — it would do nothing",
                self.name
            ));
        }
        if let Some(p) = self.plock {
            if p.id > 0xFE {
                return Err(format!(
                    "p-lock param {}: paramId must be 0–254 (0xFF marks a free lane)",
                    self.name
                ));
            }
        }
        Ok(())
    }

    /// This parameter as a lane descriptor. See [`ParamDesc`] for why the two
    /// types exist.
    pub fn describe(&self, device_kind: Option<&'static str>) -> ParamDesc {
        ParamDesc {
            name: Some(self.name),
            label: self.label.into(),
            short: self.short.into(),
            bipolar: self.bipolar,
            min: MIDI_MIN,
            max: MIDI_MAX,
            midi: self.midi,
            plock: self.plock,
            curated: true,
            device_kind,
        }
    }
}

// --- The per-box tables -------------------------------------------------------

/// Digitakt II curated p-lock parameters.
///
/// The `midi` half of every entry is **from Elektron's own MIDI implementation
/// chart** — Appendix B of the Digitakt II User Manual (OS 1.15A, independently
/// matching midi.guide's DT2 table value for value). Public, checkable, and
/// confirmable on the box in seconds.
///
/// The `plock` half is **measured on hardware** — digi-roll's Phase 0
/// experiments of 2026-08-04, run on a DT2 at OS 1.15B (build 0070), one knob
/// locked per capture and the paramId read back off the dump. The old NRPN-LSB
/// hypothesis was **wrong**: cutoff's NRPN LSB is 20 but its paramId is 44.
/// paramId is the box's own internal parameter index, page-ordered —
/// FREQ/RESO/ENV DEPTH at 44/45/46, CHO/DEL/REV/PAN at 62–65 — and it differs
/// from the DN2's for the same knob. Translate by name, never by paramId.
///
/// **Retrig is deliberately absent**: no CC and no NRPN on either box, and it is
/// not one knob (RATE/LEN/VEL/on-off on TRIG page 2), so there is nothing to
/// audition and no reason to assume it is a single lane.
pub static DT2_PARAMS: &[Param] = &[
    Param {
        name: "filter.cutoff", label: "FLTR CUTOFF", short: "CUTOFF", bipolar: false,
        midi: MidiMap { cc: Some(74), cc_lsb: None, nrpn: Some((1, 20)) },
        plock: Some(scaled_plock(44, 256)),
    },
    // The DT2's filter is multi-mode, so the manual calls this "Data entry knob
    // F (machine dependent)" rather than naming it — on every filter machine
    // that has one, knob F is resonance.
    Param {
        name: "filter.resonance", label: "FLTR RESO", short: "RESO", bipolar: false,
        midi: MidiMap { cc: Some(75), cc_lsb: None, nrpn: Some((1, 21)) },
        plock: Some(scaled_plock(45, 256)),
    },
    // The DT2 appendix prints NRPN 1/23 for both Env. Depth and Env. Delay,
    // which cannot both be right; the DN2 lists depth at 1/26 and delay at 1/23,
    // so 1/26 is the likelier value here too. The CC (77) is unambiguous, and
    // this is one to confirm on the box before trusting the NRPN.
    Param {
        name: "filter.envDepth", label: "FLTR ENV DEPTH", short: "ENV D", bipolar: true,
        midi: MidiMap { cc: Some(77), cc_lsb: None, nrpn: Some((1, 26)) },
        plock: Some(scaled_plock(46, 256)),
    },
    // Pan is 90 here and 89 on the DN2, and the DN2's 90 is something else
    // again — a table shared between the boxes would quietly move the wrong
    // knob.
    Param {
        name: "amp.pan", label: "PAN", short: "PAN", bipolar: true,
        midi: MidiMap { cc: Some(90), cc_lsb: None, nrpn: Some((1, 38)) },
        plock: Some(scaled_plock(65, 256)),
    },
    // The DT2 appendix lists no NRPN for the FX-page parameters (bit reduction,
    // overdrive, SRR) — an omission rather than a statement, most likely, but CC
    // is what we have here.
    Param {
        name: "fx.overdrive", label: "OVERDRIVE", short: "DRIVE", bipolar: false,
        midi: MidiMap { cc: Some(57), cc_lsb: None, nrpn: None },
        plock: Some(scaled_plock(74, 256)),
    },
    Param {
        name: "fx.delaySend", label: "DELAY SEND", short: "DELAY", bipolar: false,
        midi: MidiMap { cc: Some(84), cc_lsb: None, nrpn: Some((1, 36)) },
        plock: Some(scaled_plock(63, 256)),
    },
    Param {
        name: "fx.reverbSend", label: "REVERB SEND", short: "REVERB", bipolar: false,
        midi: MidiMap { cc: Some(85), cc_lsb: None, nrpn: Some((1, 37)) },
        plock: Some(scaled_plock(64, 256)),
    },
    Param {
        name: "fx.chorusSend", label: "CHORUS SEND", short: "CHORUS", bipolar: false,
        midi: MidiMap { cc: Some(12), cc_lsb: None, nrpn: Some((1, 35)) },
        plock: Some(scaled_plock(62, 256)),
    },
    // The LFO depths are high-resolution on the DT2: the appendix gives each a
    // CC LSB as well, so CC alone would lose the bottom 7 bits. NRPN carries all
    // 14, which is the reason the audition path prefers NRPN.
    Param {
        name: "lfo1.depth", label: "LFO1 DEPTH", short: "LFO1", bipolar: true,
        midi: MidiMap { cc: Some(109), cc_lsb: Some(59), nrpn: Some((1, 49)) },
        plock: Some(scaled_plock(29, 256)),
    },
    Param {
        name: "lfo2.depth", label: "LFO2 DEPTH", short: "LFO2", bipolar: true,
        midi: MidiMap { cc: Some(119), cc_lsb: Some(61), nrpn: Some((1, 57)) },
        plock: Some(scaled_plock(30, 256)),
    },
    Param {
        name: "lfo3.depth", label: "LFO3 DEPTH", short: "LFO3", bipolar: true,
        midi: MidiMap { cc: Some(86), cc_lsb: Some(63), nrpn: Some((1, 72)) },
        plock: Some(scaled_plock(31, 256)),
    },
];

/// Digitone II curated p-lock parameters.
///
/// The same eleven knobs as [`DT2_PARAMS`] under the same canonical names —
/// which is what lets cross-device copy translate them — but **different CC
/// numbers**, from Appendix C of the Digitone II User Manual (OS 1.10D, the
/// exact build on the write allowlist).
///
/// The `plock` half is **measured on hardware** — Phase 0, 2026-08-04, on a DN2
/// at OS 1.10D (build 0049). The boxes number their parameters differently —
/// filter frequency is 74 here and 44 on the DT2, and 74 *on the DT2* means
/// overdrive — but the blocks line up (FREQ/RESO/ENV DEPTH at 74/75/76 against
/// the DT2's 44/45/46, CHO/DEL/REV/PAN at 92–95 against 62–65), and the three
/// LFO depths share 29/30/31 on both boxes.
///
/// Two DN2-specific facts that shaped this table:
///
/// * **The whole of LFO3 has no CC.** The appendix's CC column is blank for all
///   eight LFO3 parameters — only NRPN is given. So LFO3 depth is auditionable
///   on a DN2 over NRPN alone, which is one reason NRPN is the default.
/// * The appendix says outright that because the machines share CC values, "it
///   is not possible to control high-resolution parameters using CC. Instead,
///   you should use NRPN messages for this purpose."
pub static DN2_PARAMS: &[Param] = &[
    Param {
        name: "filter.cutoff", label: "FLTR FREQ", short: "CUTOFF", bipolar: false,
        midi: MidiMap { cc: Some(16), cc_lsb: None, nrpn: Some((1, 20)) },
        plock: Some(scaled_plock(74, 256)),
    },
    // "Data entry knob F (machine dependent)" in the appendix — resonance on the
    // multi-mode and Lowpass 4 filters.
    Param {
        name: "filter.resonance", label: "FLTR RESO", short: "RESO", bipolar: false,
        midi: MidiMap { cc: Some(17), cc_lsb: None, nrpn: Some((1, 21)) },
        plock: Some(scaled_plock(75, 256)),
    },
    Param {
        name: "filter.envDepth", label: "FLTR ENV DEPTH", short: "ENV D", bipolar: true,
        midi: MidiMap { cc: Some(24), cc_lsb: None, nrpn: Some((1, 26)) },
        plock: Some(scaled_plock(76, 256)),
    },
    // 89 here, 90 on the DT2 — and the DT2's 89 is Volume, so getting these two
    // tables mixed up would turn a pan sweep into a volume ride.
    Param {
        name: "amp.pan", label: "PAN", short: "PAN", bipolar: true,
        midi: MidiMap { cc: Some(89), cc_lsb: None, nrpn: Some((1, 38)) },
        plock: Some(scaled_plock(95, 256)),
    },
    // Unlike the DT2's appendix, the DN2's does give the FX page an NRPN.
    Param {
        name: "fx.overdrive", label: "OVERDRIVE", short: "DRIVE", bipolar: false,
        midi: MidiMap { cc: Some(81), cc_lsb: None, nrpn: Some((1, 8)) },
        plock: Some(scaled_plock(104, 256)),
    },
    Param {
        name: "fx.delaySend", label: "DELAY SEND", short: "DELAY", bipolar: false,
        midi: MidiMap { cc: Some(30), cc_lsb: None, nrpn: Some((1, 36)) },
        plock: Some(scaled_plock(93, 256)),
    },
    Param {
        name: "fx.reverbSend", label: "REVERB SEND", short: "REVERB", bipolar: false,
        midi: MidiMap { cc: Some(31), cc_lsb: None, nrpn: Some((1, 37)) },
        plock: Some(scaled_plock(94, 256)),
    },
    Param {
        name: "fx.chorusSend", label: "CHORUS SEND", short: "CHORUS", bipolar: false,
        midi: MidiMap { cc: Some(29), cc_lsb: None, nrpn: Some((1, 35)) },
        plock: Some(scaled_plock(92, 256)),
    },
    Param {
        name: "lfo1.depth", label: "LFO1 DEPTH", short: "LFO1", bipolar: true,
        midi: MidiMap { cc: Some(109), cc_lsb: None, nrpn: Some((1, 49)) },
        plock: Some(scaled_plock(29, 256)),
    },
    Param {
        name: "lfo2.depth", label: "LFO2 DEPTH", short: "LFO2", bipolar: true,
        midi: MidiMap { cc: Some(118), cc_lsb: None, nrpn: Some((1, 57)) },
        plock: Some(scaled_plock(30, 256)),
    },
    // No CC at all — NRPN only, per the appendix.
    Param {
        name: "lfo3.depth", label: "LFO3 DEPTH", short: "LFO3", bipolar: true,
        midi: MidiMap { cc: None, cc_lsb: None, nrpn: Some((1, 72)) },
        plock: Some(scaled_plock(31, 256)),
    },
];

/// Analog Four curated p-lock parameters — thirteen, all hearable and, since
/// 2026-09-01, all writable.
///
/// The `midi` half is from **two of Elektron's own manuals, agreeing value for
/// value**: the Analog Four mk1 manual (OS 1.0, Appendix D) and the Analog
/// Keys manual (OS 1.51C, same appendix — the AK is the same engine and OS
/// line, and 1.51 is the generation a 2026 mk1 will actually be running).
/// Cross-checked against midi.guide's A4 tables, same as the two above.
///
/// **Every `plock` here was `None` until 2026-09-01, and what replaced that is
/// the method working rather than the rule relaxing.** A paramId byte only ever
/// comes from locking a knob on hardware and reading the dump back (Phase 0's
/// method), and for as long as no A4 dump had ever been read this table shipped
/// audition-only: its lanes could be drawn and *heard* over CC/NRPN, and the
/// write path refused every one of them. Both halves then arrived — the box
/// answered `0x60`–`0x6d` dump requests on 2026-08-31, and
/// `app/examples/a4_scale_probe` read all thirteen scalings off the box against
/// its own screen on 2026-09-01, one knob at a time. Each entry below carries
/// what its run saw. The A4 model still ships `sysex: None`; that is the *gen-2*
/// pattern layout and was never what a paramId waited on.
///
/// **Two things a reader must not take from that.** These are the **synth
/// tracks'** ids — SYN1–SYN4 — and the FX and CV tracks number their parameters
/// differently, which is why [`plock_id_identifies_parameter`] answers false
/// here and why an id alone is not admissible evidence on this box. And an id in
/// [`A4_SYNTH_PLOCKS`] is still not a `plock` in this table: 92 ids are
/// measured against these 13 scalings, and the 79 with no scaling stay
/// unwritable. The "missing measurement must not become a wrong byte" split this
/// file opens with is untouched — it is the measurement that moved.
///
/// Naming: entries share canonical names with the DT2/DN2 tables wherever the
/// knob is the same idea — `filter.cutoff` here is Filter1, the analog
/// four-pole, and `fx.overdrive` is the overdrive circuit between the two
/// filters (bipolar on this box: 0 is clean, negative drives the filter
/// itself, positive clips after it — the one entry here whose DT2/DN2
/// namesakes are unipolar). `lfoN.depth` is Depth A of the two-destination
/// LFOs. Three entries have no namesake because the DT2/DN2 have no such
/// knob: `osc1.level`, `osc2.level` and `amp.volume` — and note CC 7 *works*
/// here, on the AMP page, unlike on the two digis where the appendices omit
/// it entirely (see [`track_level_midi`]).
pub static A4_PARAMS: &[Param] = &[
    // High-resolution: the appendix gives a CC LSB, so CC alone would drop
    // the bottom seven bits. NRPN carries all 14 — same story as the DT2 LFO
    // depths, and the same reason the audition path prefers NRPN.
    Param {
        name: "filter.cutoff", label: "FLTR1 FREQ", short: "CUTOFF", bipolar: false,
        midi: MidiMap { cc: Some(18), cc_lsb: Some(50), nrpn: Some((1, 40)) },
        // Measured 2026-09-01: screen 0..=127 across coarse 0..=127.
        plock: Some(scaled_plock(0x22, 256)),
    },
    Param {
        name: "filter.resonance", label: "FLTR1 RESO", short: "RESO", bipolar: false,
        midi: MidiMap { cc: Some(89), cc_lsb: None, nrpn: Some((1, 41)) },
        // Measured 2026-09-01: screen 0..=127 across coarse 0..=127.
        plock: Some(scaled_plock(0x23, 256)),
    },
    Param {
        name: "filter.envDepth", label: "FLTR1 ENV DEPTH", short: "ENV D", bipolar: true,
        midi: MidiMap { cc: Some(102), cc_lsb: None, nrpn: Some((1, 44)) },
        // Measured 2026-09-01: the box's screen reads -64..=63 across coarse
        // 0..=127 — bipolar, as the appendix says. **No offset in the scaling**:
        // this app's display axis is the raw byte on every box (`Param::describe`
        // gives every curated parameter MIDI_MIN..MIDI_MAX), and a DT2's own
        // ENV DEPTH screen reads -64..+63 against `scaled_plock(46, 256)` too.
        plock: Some(scaled_plock(0x26, 256)),
    },
    Param {
        name: "amp.pan", label: "PAN", short: "PAN", bipolar: true,
        midi: MidiMap { cc: Some(10), cc_lsb: None, nrpn: Some((1, 58)) },
        // Measured 2026-09-01: the screen reads L64..CEN..R63 across coarse
        // 0..=127. Bipolar, and no offset in the scaling for the reason
        // `filter.envDepth` gives.
        plock: Some(scaled_plock(0x30, 256)),
    },
    // No CC at all — the appendix's CC column is blank for overdrive, keytrack
    // and filter2's type, so NRPN is the only way to hear this one.
    Param {
        name: "fx.overdrive", label: "FLTR OVERDRIVE", short: "DRIVE", bipolar: true,
        midi: MidiMap { cc: None, cc_lsb: None, nrpn: Some((1, 42)) },
        // Measured 2026-09-01, and the run was set up expecting this flag to be
        // *wrong*: the appendix says bipolar and the box agreed, -64..=63 across
        // coarse 0..=127. See `filter.envDepth` for why that is still no offset.
        plock: Some(scaled_plock(0x24, 256)),
    },
    Param {
        name: "fx.delaySend", label: "DELAY SEND", short: "DELAY", bipolar: false,
        midi: MidiMap { cc: Some(92), cc_lsb: None, nrpn: Some((1, 56)) },
        // Measured 2026-09-01: screen OFF..=127 across coarse 0..=127.
        plock: Some(scaled_plock(0x2e, 256)),
    },
    Param {
        name: "fx.reverbSend", label: "REVERB SEND", short: "REVERB", bipolar: false,
        midi: MidiMap { cc: Some(93), cc_lsb: None, nrpn: Some((1, 57)) },
        // Measured 2026-09-01: screen OFF..=127 across coarse 0..=127.
        plock: Some(scaled_plock(0x2f, 256)),
    },
    Param {
        name: "fx.chorusSend", label: "CHORUS SEND", short: "CHORUS", bipolar: false,
        midi: MidiMap { cc: Some(91), cc_lsb: None, nrpn: Some((1, 55)) },
        // Measured 2026-09-01: screen OFF..=127 across coarse 0..=127. It took
        // three runs, and the last two failures were the probe's rather than the
        // box's — see PLAN.md for both, and for the rule they produced: a knob
        // already sitting at the end-stop being asked for allocates no lane and
        // moves no byte, so it has to be nudged off it first.
        plock: Some(scaled_plock(0x2d, 256)),
    },
    Param {
        name: "lfo1.depth", label: "LFO1 DEPTH A", short: "LFO1", bipolar: true,
        midi: MidiMap { cc: Some(24), cc_lsb: Some(56), nrpn: Some((1, 87)) },
        // Measured 2026-09-01: coarse 0..=127 across a screen of -128..=127, so
        // the box shows **two display units per coarse count** and this app
        // addresses every other one. Not a defect and not new — the digis' own
        // LFO depths have the same screen and the same `scaled_plock(_, 256)`.
        plock: Some(scaled_plock(0x5c, 256)),
    },
    Param {
        name: "lfo2.depth", label: "LFO2 DEPTH A", short: "LFO2", bipolar: true,
        midi: MidiMap { cc: Some(26), cc_lsb: Some(58), nrpn: Some((1, 97)) },
        // As `lfo1.depth` — coarse 0..=127, screen -128..=127.
        plock: Some(scaled_plock(0x5e, 256)),
    },
    Param {
        name: "osc1.level", label: "OSC1 LEVEL", short: "OSC1", bipolar: false,
        midi: MidiMap { cc: Some(69), cc_lsb: None, nrpn: Some((1, 4)) },
        // Measured 2026-09-01: screen 0..=127 across coarse 0..=127.
        plock: Some(scaled_plock(0x06, 256)),
    },
    Param {
        name: "osc2.level", label: "OSC2 LEVEL", short: "OSC2", bipolar: false,
        midi: MidiMap { cc: Some(78), cc_lsb: None, nrpn: Some((1, 24)) },
        // Measured 2026-09-01: screen 0..=127 across coarse 0..=127.
        plock: Some(scaled_plock(0x07, 256)),
    },
    Param {
        name: "amp.volume", label: "AMP VOLUME", short: "VOL", bipolar: false,
        midi: MidiMap { cc: Some(7), cc_lsb: None, nrpn: Some((1, 59)) },
        // Measured 2026-09-01: screen 0..=127 across coarse 0..=127.
        plock: Some(scaled_plock(0x31, 256)),
    },
];

// --- The Analog Four's p-lock parameter ids ------------------------------------

/// What one `param_id` byte means in an Analog Four **synth track's** p-lock
/// pool.
///
/// Deliberately **not** a [`Param`]. A `Param`'s `plock` field is documented as
/// `None` "until the paramId *and scaling* have been measured on hardware", and
/// only the ids are measured here — so putting these in [`A4_PARAMS`] would
/// assert a scaling nobody has read off the box. This carries the fact that was
/// actually established and no more: a byte, and what the front panel calls it.
pub struct A4PlockParam {
    /// The box's own parameter byte, as a pool lane header carries it.
    pub id: u8,
    /// The page it lives on, as the box labels the page.
    pub page: &'static str,
    /// The knob's own four-character name, as the box prints it.
    pub label: &'static str,
}

/// Every p-lock parameter id measured on an Analog Four **synth** track,
/// 2026-09-01.
///
/// # How this was measured, and why none of it is interpolated
///
/// One hand on the box, one knob at a time, against
/// `examples/a4_param_probe.rs` watching the working pattern — no save, no slot
/// written. A knob turn on a held trig allocates a pool lane, the probe names the
/// id, and the operator names the knob. **Ninety-two entries, ninety-two knob
/// turns.**
///
/// It was measured exhaustively because every attempt to shortcut it failed, and
/// each failure is worth recording since all of them looked reasonable first:
///
/// * **`param_id = nrpn_lsb − 6`** fits FLTR1 FREQ, RESO and OVERDRIVE exactly,
///   and `osc1.level`'s NRPN of 4 sends it to −2. The offset is regional: −6 in
///   the filter block, −10 across the FX sends. And the LFOs break it
///   structurally rather than numerically — the published NRPNs lay LFO1 and
///   LFO2 out ten apart while the pool **interleaves** them two apart, so the two
///   numberings are not one order with an offset.
/// * **"ids run consecutively in screen order"** holds for the ten filter knobs
///   and fails on the very next page: the AMP page's envelope row is stride 3,
///   because three envelopes are interleaved.
/// * **"a page is laid out by rows"** and **"by columns"** are both wrong for
///   OSC page 2, which pairs AM1 and AM2 first and then takes the rest of each
///   row in turn.
///
/// Four layout schemes appear here — sequential, interleaved by 2, interleaved
/// by 3, and paired-then-sequential — and no page predicts the next.
///
/// # What it does not cover
///
/// **Synth tracks only.** The id space is *per track kind*: an FX-track lock was
/// measured landing on `0x1a` and `0x29`, both of which are synth parameters
/// here. Labelling an FX or CV lane from this table would produce a confident
/// wrong name, which is why the lookup is
/// [`a4_synth_plock_label`] and takes the caller's word for it.
///
/// **Ids, not scalings.** What a stored word means on screen is a second
/// measurement, and only OSC TUNE has had it — see [`crate::a4_plocks`], where
/// tune turned out to be bipolar with a borrow, refuting "the coarse byte is the
/// displayed value" for the general case.
///
/// **Nine ids in range are unmapped** and stay that way rather than being
/// guessed: `0x12`–`0x14`, `0x2c`, `0x32`, `0x60`–`0x63`. A lane arriving on one
/// of those has no name here and will show as its hex byte, which is the honest
/// answer and is visible if it ever happens.
pub static A4_SYNTH_PLOCKS: &[A4PlockParam] = &[
    // OSC1 and OSC2 interleave: OSC1 on even ids, OSC2 on odd. Confirmed at both
    // ends of the page — TUNE (0x00/0x01) and PWM (0x10/0x11) — rather than from
    // one anchor and a stride.
    //
    // **TUN and FIN are one parameter, not two.** They are the coarse and fine
    // halves of a single lane, which is why the front panel locks both when
    // either is turned and why a ten-knob page yields nine ids.
    A4PlockParam { id: 0x00, page: "OSC1", label: "TUN/FIN" },
    A4PlockParam { id: 0x02, page: "OSC1", label: "DET" },
    A4PlockParam { id: 0x04, page: "OSC1", label: "TRK" },
    A4PlockParam { id: 0x06, page: "OSC1", label: "LEV" },
    A4PlockParam { id: 0x08, page: "OSC1", label: "WAV" },
    A4PlockParam { id: 0x0a, page: "OSC1", label: "SUB" },
    A4PlockParam { id: 0x0c, page: "OSC1", label: "PW" },
    A4PlockParam { id: 0x0e, page: "OSC1", label: "SPD" },
    A4PlockParam { id: 0x10, page: "OSC1", label: "PWM" },
    A4PlockParam { id: 0x01, page: "OSC2", label: "TUN/FIN" },
    A4PlockParam { id: 0x03, page: "OSC2", label: "DET" },
    A4PlockParam { id: 0x05, page: "OSC2", label: "TRK" },
    A4PlockParam { id: 0x07, page: "OSC2", label: "LEV" },
    A4PlockParam { id: 0x09, page: "OSC2", label: "WAV" },
    A4PlockParam { id: 0x0b, page: "OSC2", label: "SUB" },
    A4PlockParam { id: 0x0d, page: "OSC2", label: "PW" },
    A4PlockParam { id: 0x0f, page: "OSC2", label: "SPD" },
    A4PlockParam { id: 0x11, page: "OSC2", label: "PWM" },
    // 0x12-0x14 unmapped.
    //
    // The noise section, on OSC1's second page. Consecutive rather than
    // interleaved because there is one noise generator, not one per oscillator —
    // the same reason the shared OSC page below is consecutive.
    A4PlockParam { id: 0x15, page: "NOISE", label: "SnH" },
    A4PlockParam { id: 0x16, page: "NOISE", label: "FAD" },
    A4PlockParam { id: 0x17, page: "NOISE", label: "LEV" },
    // OSC page 2 — settings the manual says affect *both* oscillators, which is
    // why there is one set of them. AM1 and AM2 are paired first, then the rest
    // of the top row, then the rest of the bottom row: a layout neither
    // row-major nor column-major, and both guesses were made and refuted.
    A4PlockParam { id: 0x18, page: "OSC", label: "AM1" },
    A4PlockParam { id: 0x19, page: "OSC", label: "AM2" },
    A4PlockParam { id: 0x1a, page: "OSC", label: "SMD" },
    A4PlockParam { id: 0x1b, page: "OSC", label: "SNC" },
    A4PlockParam { id: 0x1c, page: "OSC", label: "BND" },
    A4PlockParam { id: 0x1d, page: "OSC", label: "SLI" },
    A4PlockParam { id: 0x1e, page: "OSC", label: "TRG" },
    A4PlockParam { id: 0x1f, page: "OSC", label: "FAD" },
    A4PlockParam { id: 0x20, page: "OSC", label: "SPD" },
    A4PlockParam { id: 0x21, page: "OSC", label: "VIB" },
    // The two filters, sequential and in screen order — the one page where the
    // obvious layout is the real one.
    A4PlockParam { id: 0x22, page: "FLTR1", label: "FRQ" },
    A4PlockParam { id: 0x23, page: "FLTR1", label: "RES" },
    A4PlockParam { id: 0x24, page: "FLTR1", label: "OVR" },
    A4PlockParam { id: 0x25, page: "FLTR1", label: "TRK" },
    A4PlockParam { id: 0x26, page: "FLTR1", label: "DEP" },
    A4PlockParam { id: 0x27, page: "FLTR2", label: "FRQ" },
    A4PlockParam { id: 0x28, page: "FLTR2", label: "RES" },
    A4PlockParam { id: 0x29, page: "FLTR2", label: "TYP" },
    A4PlockParam { id: 0x2a, page: "FLTR2", label: "TRK" },
    A4PlockParam { id: 0x2b, page: "FLTR2", label: "DEP" },
    // 0x2c unmapped.
    //
    // The AMP page's bottom row: sends, pan and volume.
    A4PlockParam { id: 0x2d, page: "AMP", label: "CHO" },
    A4PlockParam { id: 0x2e, page: "AMP", label: "DEL" },
    A4PlockParam { id: 0x2f, page: "AMP", label: "REV" },
    A4PlockParam { id: 0x30, page: "AMP", label: "PAN" },
    A4PlockParam { id: 0x31, page: "AMP", label: "VOL" },
    // 0x32 unmapped.
    //
    // **Three envelopes interleaved at stride 3** — ENV1, ENV2, then AMP, in
    // that order. The AMP page's top row is what exposed the stride; ENV1 and
    // ENV2 placed AMP third, where the prediction had put it first.
    A4PlockParam { id: 0x33, page: "ENV1", label: "ATK" },
    A4PlockParam { id: 0x34, page: "ENV2", label: "ATK" },
    A4PlockParam { id: 0x35, page: "AMP", label: "ATK" },
    A4PlockParam { id: 0x36, page: "ENV1", label: "DEC" },
    A4PlockParam { id: 0x37, page: "ENV2", label: "DEC" },
    A4PlockParam { id: 0x38, page: "AMP", label: "DEC" },
    A4PlockParam { id: 0x39, page: "ENV1", label: "SUS" },
    A4PlockParam { id: 0x3a, page: "ENV2", label: "SUS" },
    A4PlockParam { id: 0x3b, page: "AMP", label: "SUS" },
    A4PlockParam { id: 0x3c, page: "ENV1", label: "REL" },
    A4PlockParam { id: 0x3d, page: "ENV2", label: "REL" },
    A4PlockParam { id: 0x3e, page: "AMP", label: "REL" },
    A4PlockParam { id: 0x3f, page: "ENV1", label: "SHP" },
    A4PlockParam { id: 0x40, page: "ENV2", label: "SHP" },
    A4PlockParam { id: 0x41, page: "AMP", label: "SHP" },
    // Only ENV1 and ENV2 have LEN, so the interleave drops to two here and the
    // AMP envelope has no entry.
    A4PlockParam { id: 0x42, page: "ENV1", label: "LEN" },
    A4PlockParam { id: 0x43, page: "ENV2", label: "LEN" },
    // Destinations and depths are *blocked* rather than interleaved: each
    // envelope's two sit together.
    A4PlockParam { id: 0x44, page: "ENV1", label: "DST1" },
    A4PlockParam { id: 0x45, page: "ENV1", label: "DST2" },
    A4PlockParam { id: 0x46, page: "ENV2", label: "DST1" },
    A4PlockParam { id: 0x47, page: "ENV2", label: "DST2" },
    A4PlockParam { id: 0x48, page: "ENV1", label: "DEP1" },
    A4PlockParam { id: 0x49, page: "ENV1", label: "DEP2" },
    A4PlockParam { id: 0x4a, page: "ENV2", label: "DEP1" },
    A4PlockParam { id: 0x4b, page: "ENV2", label: "DEP2" },
    // The two LFOs interleave at stride 2 for the first six knobs, then block
    // for destinations and depths — the same shape as the envelopes' tail.
    A4PlockParam { id: 0x4c, page: "LFO1", label: "SPD" },
    A4PlockParam { id: 0x4d, page: "LFO2", label: "SPD" },
    A4PlockParam { id: 0x4e, page: "LFO1", label: "MUL" },
    A4PlockParam { id: 0x4f, page: "LFO2", label: "MUL" },
    A4PlockParam { id: 0x50, page: "LFO1", label: "FAD" },
    A4PlockParam { id: 0x51, page: "LFO2", label: "FAD" },
    A4PlockParam { id: 0x52, page: "LFO1", label: "SPH" },
    A4PlockParam { id: 0x53, page: "LFO2", label: "SPH" },
    A4PlockParam { id: 0x54, page: "LFO1", label: "MOD" },
    A4PlockParam { id: 0x55, page: "LFO2", label: "MOD" },
    A4PlockParam { id: 0x56, page: "LFO1", label: "WAV" },
    A4PlockParam { id: 0x57, page: "LFO2", label: "WAV" },
    A4PlockParam { id: 0x58, page: "LFO1", label: "DST1" },
    A4PlockParam { id: 0x59, page: "LFO1", label: "DST2" },
    A4PlockParam { id: 0x5a, page: "LFO2", label: "DST1" },
    A4PlockParam { id: 0x5b, page: "LFO2", label: "DST2" },
    A4PlockParam { id: 0x5c, page: "LFO1", label: "DEP1" },
    A4PlockParam { id: 0x5d, page: "LFO1", label: "DEP2" },
    A4PlockParam { id: 0x5e, page: "LFO2", label: "DEP1" },
    A4PlockParam { id: 0x5f, page: "LFO2", label: "DEP2" },
    // 0x60-0x63 unmapped.
    //
    // **Noise COL sits 77 ids from the rest of its own page.** Measured twice,
    // in two independent sweeps, in the same position in the turn order — it was
    // the reading that looked like an artefact and was not. A parameter added in
    // a later OS would land here, since appending costs nothing where inserting
    // at 0x16 would renumber everything above it and break every saved pattern;
    // that is a hypothesis and an early-OS box would be needed to test it.
    A4PlockParam { id: 0x64, page: "NOISE", label: "COL" },
];

/// What an Analog Four **synth** track's pool lane is called, or `None` for an
/// id nobody has measured.
///
/// **The caller asserts the track kind, and must be right.** The id space
/// differs per track kind — an FX-track lock was measured on `0x1a` and `0x29`,
/// both synth parameters in [`A4_SYNTH_PLOCKS`] — so calling this for an FX or
/// CV lane produces a confident wrong name. There is no FX or CV table yet
/// because neither has been swept.
pub fn a4_synth_plock_label(param_id: u8) -> Option<&'static str> {
    A4_SYNTH_PLOCKS.iter().find(|p| p.id == param_id).map(|p| p.label)
}

/// `FLTR1 FRQ` — page and knob, as the box prints them, for a synth track.
pub fn a4_synth_plock_full_label(param_id: u8) -> Option<String> {
    A4_SYNTH_PLOCKS
        .iter()
        .find(|p| p.id == param_id)
        .map(|p| format!("{} {}", p.page, p.label))
}

// --- The track's own level -----------------------------------------------------

/// The track LEVEL fader, per box: the one on the box's mixer, not the AMP
/// page's VOL.
///
/// **Deliberately not one of the [`Param`]s above.** The two tables model
/// *p-lock* parameters, and every entry in them carries a `plock` measured on
/// hardware in Phase 0. Nobody has measured this one's paramId, so an entry here
/// would be a lane the "+ add lane…" picker offers and the write path then
/// refuses — the split at the top of this file exists to keep a missing
/// measurement from becoming a wrong byte, and the honest place for a parameter
/// with a published CC and no measured lane is its own function. Move it into
/// the tables the day someone locks LEVEL on a box and reads the paramId back.
///
/// The numbers are from the boxes' own charts (DT2 Appendix B, DN2 Appendix C,
/// cross-checked against midi.guide, which is where the tables above came from
/// too). **The two boxes agree on the CC and disagree on the NRPN**, which is
/// exactly the trap this file is built around: 95 on both, but NRPN 1/100 on a
/// DT2 and 1/110 on a DN2.
///
/// **CC 7 is not this, on either box.** Channel Volume is absent from both
/// appendices — an audio track does not answer it — so a fader sending 7 would
/// move nothing at all. Worth writing down because 7 is the obvious guess.
pub fn track_level_midi(device_kind: &str) -> Option<MidiMap> {
    match device_kind {
        "DT2" => Some(MidiMap { cc: Some(95), cc_lsb: None, nrpn: Some((1, 100)) }),
        "DN2" => Some(MidiMap { cc: Some(95), cc_lsb: None, nrpn: Some((1, 110)) }),
        // Not in the OS 1.0 appendix — TRACK CC/NRPN arrived in a later OS and
        // is documented in the Analog Keys OS 1.51C manual's Appendix D, which
        // is the generation a current mk1 runs. Note it sides with the DT2 on
        // the NRPN, not the DN2.
        "A4" => Some(MidiMap { cc: Some(95), cc_lsb: None, nrpn: Some((1, 100)) }),
        _ => None,
    }
}

/// What the box calls it on screen, for a UI that has to name what it is
/// sending.
pub const TRACK_LEVEL_LABEL: &str = "TRACK LEVEL";

// --- Table lookup -------------------------------------------------------------
//
// Every lookup goes through these, so a table with nothing in it behaves like a
// table with nothing matching rather than like a bug.

/// The curated set for a device kind, keyed as [`crate::pattern::Spec::device`]
/// spells it. An unknown kind — none today, but a lane loaded from an old
/// project file could name one — gets an **empty table**, which reads as
/// "nothing curated" everywhere and so degrades to read-only rather than to a
/// panic.
pub fn param_table_for(kind: &str) -> &'static [Param] {
    match kind {
        "DT2" => DT2_PARAMS,
        "DN2" => DN2_PARAMS,
        "A4" => A4_PARAMS,
        _ => &[],
    }
}

/// **Does a bare `param_id` say which parameter a lane automates?**
///
/// True on the digis and **false on the Analog Four**, where the id space is per
/// *track kind*: measured 2026-09-01, an FX-track lock landed on `0x1a` and
/// `0x29`, both of which name synth parameters in [`A4_SYNTH_PLOCKS`]. A
/// `PLockLane` does not carry its track, so on that box an id alone is not
/// evidence — and once [`A4_PARAMS`] gained measured `plock` ids on 2026-09-01,
/// letting [`param_by_plock_id`] answer for an A4 lane would have made every
/// FX-track lane on `0x22` resolve to `filter.cutoff`: curated, editable, drawn
/// on the 0..127 axis instead of the raw one, and **written back as a corrupted
/// FX lane**. That is the concrete bug this function exists to prevent.
///
/// The import knows the track kind (`a4_transfer::a4_lane_to_model`) and records
/// the answer as the lane's canonical `name`. Name is admissible evidence on
/// every box; id is admissible on all but this one.
pub fn plock_id_identifies_parameter(kind: &str) -> bool {
    kind != "A4"
}

/// The `'static` spelling of a device kind, or `None` for one with no table.
///
/// [`param_table_for`] takes any `&str` and answers with an empty table, but
/// [`describe_param`] needs a `'static` kind to label a lane with — and a lane
/// carries its kind as an owned `String`. This is the one place that crossing
/// happens, so a caller cannot spell `"DT2"` a second time and drift.
pub fn device_kind_key(kind: &str) -> Option<&'static str> {
    DEVICE_KINDS.iter().copied().find(|k| *k == kind)
}

/// By canonical name — how a lane this app authored identifies its parameter.
pub fn param_by_name(table: &'static [Param], name: &str) -> Option<&'static Param> {
    table.iter().find(|p| p.name == name)
}

/// By the p-lock `paramId` byte read out of a pattern. Only ever matches once a
/// parameter's `plock` has been measured — which is what stops this app from
/// claiming it knows what a lane in an imported pattern is.
pub fn param_by_plock_id(table: &'static [Param], id: u16) -> Option<&'static Param> {
    table.iter().find(|p| p.plock.is_some_and(|pl| pl.id == id))
}

/// Parameters that can be *heard* on a given box. All eleven, on both, today.
pub fn auditable_params_for(kind: &str) -> Vec<&'static Param> {
    param_table_for(kind).iter().filter(|p| p.auditable()).collect()
}

/// Parameters that can be *written into a pattern*: their paramId is measured.
/// Also all eleven since Phase 0 — the gap between this and
/// [`auditable_params_for`] is exactly what is missing from the feature, and the
/// UI should read it rather than hard-code what works.
pub fn writable_params_for(kind: &str) -> Vec<&'static Param> {
    param_table_for(kind).iter().filter(|p| p.writable()).collect()
}

/// Is anything writable on any box? False would mean the whole p-lock write path
/// has nothing to say, and the UI should show that rather than an empty menu.
pub fn any_writable_params() -> bool {
    DEVICE_KINDS.iter().any(|k| !writable_params_for(k).is_empty())
}

// --- Describing a lane --------------------------------------------------------

/// How a lane should be labelled, drawn and scaled.
///
/// Two types where the JS has one shape: [`Param`] is a `'static` hardware fact
/// and lookups hand one back without allocating, while a `ParamDesc` is what a
/// *lane* resolves to — including a lane whose paramId is in no table, whose
/// label has to be built. Everything that draws or edits a lane wants this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDesc {
    /// `None` for a lane that matched nothing: we do not know which knob it is.
    pub name: Option<&'static str>,
    pub label: std::borrow::Cow<'static, str>,
    pub short: std::borrow::Cow<'static, str>,
    pub bipolar: bool,
    pub min: i32,
    pub max: i32,
    pub midi: MidiMap,
    pub plock: Option<PLock>,
    /// False for a lane drawn over the raw word range because we cannot honestly
    /// draw it over anything narrower.
    pub curated: bool,
    /// The box whose numbering `plock.id` belongs to, which is why a DT2 lane is
    /// never read as a DN2 one.
    pub device_kind: Option<&'static str>,
}

impl ParamDesc {
    pub fn auditable(&self) -> bool {
        self.midi.cc.is_some() || self.midi.nrpn.is_some()
    }

    pub fn writable(&self) -> bool {
        self.plock.is_some()
    }

    /// Clamp a display value into this parameter's range, on its own resolution.
    /// The step is 1 everywhere today, so this rounds and clamps.
    pub fn clamp_value(&self, v: f64) -> i32 {
        let rounded = v.round();
        if rounded < self.min as f64 {
            self.min
        } else if rounded > self.max as f64 {
            self.max
        } else {
            rounded as i32
        }
    }

    /// Display value → the lane's stored word. `None` when the scaling has not
    /// been measured, so a missing measurement can never silently become a wrong
    /// byte — a caller must check [`ParamDesc::writable`] first and mean it.
    pub fn stored_from_display(&self, v: f64) -> Option<u16> {
        let plock = self.plock?;
        let clamped = self.clamp_value(v) as f64;
        let word = match plock.scaling {
            PLockScaling::Plain => clamped,
            PLockScaling::Scaled(factor) => clamped * factor as f64,
        }
        .round();
        Some(word.clamp(0.0, RAW_VALUE_MAX as f64) as u16)
    }

    /// The lane's stored word → a display value. `None` on the same rule as
    /// [`ParamDesc::stored_from_display`].
    ///
    /// The box keeps sub-MIDI fine resolution in the low byte, so an imported
    /// lock can carry a fraction this integer axis rounds on the way in;
    /// re-sending such a lane quantises it to the nearest MIDI step. That is a
    /// known and accepted loss, not a bug — and it is why a lane is carried
    /// byte-exact rather than round-tripped through here when nothing edits it.
    pub fn display_from_stored(&self, w: u16) -> Option<i32> {
        let plock = self.plock?;
        let v = match plock.scaling {
            PLockScaling::Plain => w as f64,
            PLockScaling::Scaled(factor) => w as f64 / factor as f64,
        };
        Some(self.clamp_value(v))
    }
}

/// The curated parameter a lane's two identities point at, or `None` when
/// neither lands in this table.
///
/// **Name wins.** When this app authored the lane we know exactly which knob it
/// is; a paramId is the weaker evidence, and on the wrong box it is actively
/// misleading — 74 is filter frequency on a DN2 and overdrive on a DT2.
///
/// Split out so the precedence exists once: [`describe_param`] resolves a lane
/// for drawing, the audition path resolves one for sending, and the two must not
/// be able to disagree about which parameter a lane *is*.
pub fn curated_param(
    table: &'static [Param],
    name: Option<&str>,
    param_id: Option<u16>,
) -> Option<&'static Param> {
    name.and_then(|n| param_by_name(table, n))
        .or_else(|| param_id.and_then(|id| param_by_plock_id(table, id)))
}

/// What a lane's parameter is, given whichever of the two identities it carries.
///
/// `name` wins when this app authored the lane, because then we know exactly
/// which parameter it is. Otherwise we fall back to the raw `paramId` byte — and
/// a raw lane is deliberately **not** curated: it is drawn over the whole uint16
/// range because we have no idea what its real range is, and it is never
/// auditioned, because we have no CC to audition it with.
///
/// The pass-through scaling on a raw lane is the only honest one available for
/// something we cannot scale, and it is what keeps an imported lane byte-exact
/// on the way back out.
///
/// **One deliberate deviation from the oracle:** the JS's raw branch builds its
/// descriptor as a fresh literal and drops `deviceKind` on the floor, keeping it
/// only inside the label text. Here it is carried on both branches. Nothing in
/// the JS reads it off a raw descriptor — every consumer reads `lane.deviceKind`
/// instead — so this fixes an inconsistency rather than changing a behaviour,
/// and a raw lane is the one case where which box's numbering the id belongs to
/// matters most.
pub fn describe_param(
    table: &'static [Param],
    name: Option<&str>,
    param_id: Option<u16>,
    device_kind: Option<&'static str>,
) -> ParamDesc {
    if let Some(p) = curated_param(table, name, param_id) {
        return p.describe(device_kind);
    }
    raw_param_desc(param_id, device_kind)
}

/// The descriptor a lane gets when it matched nothing in a curated table: drawn
/// over the whole uint16 range, never auditioned, and passed through byte-exact.
///
/// Split out of [`describe_param`] on 2026-09-01 so a caller that resolves
/// curation on its own terms can still land on *this* descriptor rather than
/// building a second one. `PLockLane::param` is that caller: on the A4 it
/// refuses to curate from a bare id (see [`plock_id_identifies_parameter`]) and
/// then needs exactly the fallback below — including the hex stand-in, which is
/// the only thing a person can act on for a lane nothing has named.
pub fn raw_param_desc(param_id: Option<u16>, device_kind: Option<&'static str>) -> ParamDesc {
    let hex = match param_id {
        Some(id) => format!("0x{id:02x}"),
        None => "??".to_string(),
    };
    let prefix = device_kind.map(|k| format!("{k} ")).unwrap_or_default();
    ParamDesc {
        name: None,
        label: format!("{prefix}param {hex}").into(),
        short: format!("p {hex}").into(),
        bipolar: false,
        min: 0,
        max: RAW_VALUE_MAX as i32,
        midi: MidiMap { cc: None, cc_lsb: None, nrpn: None },
        plock: param_id.map(plain_plock),
        curated: false,
        device_kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field of every entry, in the shape the JS oracle prints it. The
    /// expected block below was **derived by running the JS**, not typed off the
    /// tables by eye:
    ///
    /// ```text
    /// cd ../digi-roll && node --input-type=module -e "
    /// import { paramTableFor, DEVICE_KINDS } from './js/elektron/param-tables.js';
    /// for (const k of DEVICE_KINDS) for (const p of paramTableFor(k))
    ///   console.log([p.name,p.label,p.short,p.bipolar,p.midi.cc,p.midi.ccLsb,
    ///     JSON.stringify(p.midi.nrpn),p.plock&&p.plock.id,p.plock.toStored(1),
    ///     p.min,p.max,p.step,p.auditable,p.writable].join('|'));"
    /// ```
    ///
    /// One assertion rather than eleven, because the failure this guards is a
    /// transcription slip in a table of 22 hand-copied entries, and a slip can
    /// land in any field.
    fn render(table: &[Param]) -> String {
        table
            .iter()
            .map(|p| {
                let num = |n: Option<u8>| n.map(|v| v.to_string()).unwrap_or_default();
                let nrpn = match p.midi.nrpn {
                    Some((m, l)) => format!("[{m},{l}]"),
                    None => "null".to_string(),
                };
                // `toStored(1)` is how the oracle line below reports the
                // scaling, and it is the whole of what a wrong factor would
                // change: 256 means the word is the display value × 256.
                let (id, factor) = match p.plock {
                    Some(pl) => (
                        pl.id.to_string(),
                        match pl.scaling {
                            PLockScaling::Plain => 1,
                            PLockScaling::Scaled(f) => f,
                        }
                        .to_string(),
                    ),
                    None => (String::new(), String::new()),
                };
                format!(
                    "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|1|{}|{}",
                    p.name, p.label, p.short, p.bipolar,
                    num(p.midi.cc), num(p.midi.cc_lsb), nrpn, id, factor,
                    MIDI_MIN, MIDI_MAX, p.auditable(), p.writable(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_dt2_table_matches_the_js_oracle() {
        assert_eq!(
            render(DT2_PARAMS),
            "\
filter.cutoff|FLTR CUTOFF|CUTOFF|false|74||[1,20]|44|256|0|127|1|true|true
filter.resonance|FLTR RESO|RESO|false|75||[1,21]|45|256|0|127|1|true|true
filter.envDepth|FLTR ENV DEPTH|ENV D|true|77||[1,26]|46|256|0|127|1|true|true
amp.pan|PAN|PAN|true|90||[1,38]|65|256|0|127|1|true|true
fx.overdrive|OVERDRIVE|DRIVE|false|57||null|74|256|0|127|1|true|true
fx.delaySend|DELAY SEND|DELAY|false|84||[1,36]|63|256|0|127|1|true|true
fx.reverbSend|REVERB SEND|REVERB|false|85||[1,37]|64|256|0|127|1|true|true
fx.chorusSend|CHORUS SEND|CHORUS|false|12||[1,35]|62|256|0|127|1|true|true
lfo1.depth|LFO1 DEPTH|LFO1|true|109|59|[1,49]|29|256|0|127|1|true|true
lfo2.depth|LFO2 DEPTH|LFO2|true|119|61|[1,57]|30|256|0|127|1|true|true
lfo3.depth|LFO3 DEPTH|LFO3|true|86|63|[1,72]|31|256|0|127|1|true|true"
        );
    }

    #[test]
    fn the_dn2_table_matches_the_js_oracle() {
        assert_eq!(
            render(DN2_PARAMS),
            "\
filter.cutoff|FLTR FREQ|CUTOFF|false|16||[1,20]|74|256|0|127|1|true|true
filter.resonance|FLTR RESO|RESO|false|17||[1,21]|75|256|0|127|1|true|true
filter.envDepth|FLTR ENV DEPTH|ENV D|true|24||[1,26]|76|256|0|127|1|true|true
amp.pan|PAN|PAN|true|89||[1,38]|95|256|0|127|1|true|true
fx.overdrive|OVERDRIVE|DRIVE|false|81||[1,8]|104|256|0|127|1|true|true
fx.delaySend|DELAY SEND|DELAY|false|30||[1,36]|93|256|0|127|1|true|true
fx.reverbSend|REVERB SEND|REVERB|false|31||[1,37]|94|256|0|127|1|true|true
fx.chorusSend|CHORUS SEND|CHORUS|false|29||[1,35]|92|256|0|127|1|true|true
lfo1.depth|LFO1 DEPTH|LFO1|true|109||[1,49]|29|256|0|127|1|true|true
lfo2.depth|LFO2 DEPTH|LFO2|true|118||[1,57]|30|256|0|127|1|true|true
lfo3.depth|LFO3 DEPTH|LFO3|true|||[1,72]|31|256|0|127|1|true|true"
        );
    }

    #[test]
    fn every_curated_entry_passes_the_rules_the_js_throws_on() {
        for kind in DEVICE_KINDS {
            for p in param_table_for(kind) {
                assert!(p.validate().is_ok(), "{kind} {}: {:?}", p.name, p.validate());
            }
        }
    }

    #[test]
    fn validate_catches_what_the_js_constructor_catches() {
        let base = DT2_PARAMS[0];

        let nameless = Param { name: "", ..base };
        assert!(nameless.validate().unwrap_err().contains("needs a name"));

        let inert = Param {
            midi: MidiMap::default(),
            plock: None,
            ..base
        };
        assert!(inert.validate().unwrap_err().contains("it would do nothing"));

        // 0xFF is the free-lane sentinel, so no parameter may claim it.
        let sentinel = Param { plock: Some(scaled_plock(0xFF, 256)), ..base };
        assert!(sentinel.validate().unwrap_err().contains("0xFF marks a free lane"));
    }

    #[test]
    fn eleven_parameters_on_both_boxes_and_every_one_of_them_measured() {
        // The whole state of this feature in one test. `auditable` comes from the
        // published CC/NRPN charts; `writable` comes from the Phase 0 hardware
        // experiments of 2026-08-04 (DT2 build 0070, DN2 build 0049), which
        // measured the paramId and scaling for all eleven knobs on both boxes.
        for kind in ["DT2", "DN2"] {
            assert_eq!(param_table_for(kind).len(), 11, "{kind}");
            assert_eq!(auditable_params_for(kind).len(), 11, "{kind}");
            assert_eq!(writable_params_for(kind).len(), 11, "{kind}");
        }
        // **All thirteen of the A4's are hearable and all thirteen are
        // writable**, closed 2026-09-01 — the box now stands with the digis
        // rather than beside them. This said "none writable" that morning;
        // `a4_scale_probe` read every scaling off the box against its own
        // screen, four runs, one knob at a time.
        assert_eq!(param_table_for("A4").len(), 13);
        assert_eq!(auditable_params_for("A4").len(), 13);
        assert_eq!(writable_params_for("A4").len(), 13);
        assert!(any_writable_params());
    }

    #[test]
    fn the_measured_param_ids_are_the_ones_the_captures_gave() {
        // Hardware facts, not derivable from any chart — the NRPN hypothesis
        // failed (cutoff's NRPN LSB is 20, its paramId is 44). Same knob,
        // different number per box; 74 is overdrive on a DT2 and filter
        // frequency on a DN2, which is why lanes translate by name.
        const MEASURED: &[(&str, u16, u16)] = &[
            ("filter.cutoff", 44, 74),
            ("filter.resonance", 45, 75),
            ("filter.envDepth", 46, 76),
            ("fx.chorusSend", 62, 92),
            ("fx.delaySend", 63, 93),
            ("fx.reverbSend", 64, 94),
            ("amp.pan", 65, 95),
            ("fx.overdrive", 74, 104),
            ("lfo1.depth", 29, 29),
            ("lfo2.depth", 30, 30),
            ("lfo3.depth", 31, 31),
        ];
        let id = |kind, name| param_by_name(param_table_for(kind), name).unwrap().plock.unwrap().id;
        for (name, dt2, dn2) in MEASURED {
            assert_eq!(id("DT2", name), *dt2, "DT2 {name}");
            assert_eq!(id("DN2", name), *dn2, "DN2 {name}");
        }
    }

    #[test]
    fn one_scaling_law_covers_every_measured_parameter() {
        // stored = display × 256 on the MIDI 0–127 axis, both boxes, every
        // parameter: cutoff 127 → 0x7F00, pan hard left → 0, LFO depth +16
        // (MIDI 72) → 0x4800 — all read back off the box byte-for-byte.
        for kind in DEVICE_KINDS {
            for p in writable_params_for(kind) {
                let d = p.describe(Some(kind));
                assert_eq!(d.stored_from_display(0.0), Some(0), "{kind} {}", p.name);
                assert_eq!(d.stored_from_display(127.0), Some(0x7F00), "{kind} {}", p.name);
                assert_eq!(d.display_from_stored(0x4800), Some(72), "{kind} {}", p.name);
            }
        }
    }

    #[test]
    fn both_boxes_name_the_same_knobs_so_a_copy_can_translate() {
        let names = |kind| {
            let mut v: Vec<_> = param_table_for(kind).iter().map(|p| p.name).collect();
            v.sort_unstable();
            v
        };
        assert_eq!(names("DT2"), names("DN2"));
    }

    #[test]
    fn the_two_boxes_give_the_same_knob_different_ccs() {
        // Not a curiosity — pan is CC 90 on a DT2 and CC 89 on a DN2, and 89 is
        // Volume on the DT2. One shared table would ride the wrong fader.
        let cc = |kind, name| param_by_name(param_table_for(kind), name).unwrap().midi.cc;
        assert_eq!(cc("DT2", "amp.pan"), Some(90));
        assert_eq!(cc("DN2", "amp.pan"), Some(89));
        assert_eq!(cc("DT2", "filter.cutoff"), Some(74));
        assert_eq!(cc("DN2", "filter.cutoff"), Some(16));
    }

    #[test]
    fn the_two_boxes_mostly_give_the_same_knob_the_same_nrpn() {
        // The observation behind the failed Phase 0 hypothesis: NRPN LSB looks
        // like an internal parameter index, so it might have *been* the paramId.
        // It is not, and the table records both so nobody re-derives one from the
        // other.
        let nrpn = |kind, name| param_by_name(param_table_for(kind), name).unwrap().midi.nrpn;
        for name in [
            "filter.cutoff", "filter.resonance", "amp.pan", "fx.delaySend",
            "fx.reverbSend", "fx.chorusSend", "lfo1.depth", "lfo2.depth", "lfo3.depth",
        ] {
            assert_eq!(nrpn("DT2", name), nrpn("DN2", name), "{name}");
        }
    }

    #[test]
    fn track_level_is_cc_95_on_both_boxes_and_a_different_nrpn_on_each() {
        // The trap this file exists for, in one parameter: shared CC, different
        // NRPN. A single number copied from one appendix to the other would ride
        // the wrong thing on a DN2.
        assert_eq!(track_level_midi("DT2").unwrap().cc, Some(95));
        assert_eq!(track_level_midi("DN2").unwrap().cc, Some(95));
        assert_eq!(track_level_midi("DT2").unwrap().nrpn, Some((1, 100)));
        assert_eq!(track_level_midi("DN2").unwrap().nrpn, Some((1, 110)));
        // A box with no chart gets nothing, not a guess — same rule as
        // `param_table_for`'s empty table.
        assert!(track_level_midi("DT1").is_none());
    }

    #[test]
    fn track_level_is_not_in_the_p_lock_tables_and_nothing_claims_cc_7() {
        // It has no measured paramId, so it must not be offered as a lane; and
        // on the digis CC 7 is in neither appendix, so nothing may map to it.
        for kind in DEVICE_KINDS {
            assert!(param_by_name(param_table_for(kind), "track.level").is_none());
            assert_ne!(track_level_midi(kind).unwrap().cc, Some(7));
        }
        for kind in ["DT2", "DN2"] {
            assert!(
                param_table_for(kind).iter().all(|p| p.midi.cc != Some(7)),
                "{kind}: CC 7 is Channel Volume and neither digi answers it"
            );
        }
        // The A4 is the exception that proves the entry: its AMP page's VOL
        // *is* CC 7, straight from its appendix — one box's dead controller is
        // another's published knob, which is the whole argument for per-box
        // tables over a shared one.
        assert_eq!(
            param_by_name(param_table_for("A4"), "amp.volume").unwrap().midi.cc,
            Some(7)
        );
    }

    #[test]
    fn an_unknown_device_kind_gets_an_empty_table_rather_than_a_panic() {
        assert!(param_table_for("DT1").is_empty());
        assert!(auditable_params_for("").is_empty());
        assert!(param_by_name(param_table_for("DT1"), "filter.cutoff").is_none());
    }

    #[test]
    fn a_paramid_only_matches_once_it_has_been_measured() {
        assert_eq!(
            param_by_plock_id(DT2_PARAMS, 44).map(|p| p.name),
            Some("filter.cutoff")
        );
        // The DN2's cutoff id, read against the DT2's table, is overdrive — the
        // whole reason a lane remembers which box it came off.
        assert_eq!(
            param_by_plock_id(DT2_PARAMS, 74).map(|p| p.name),
            Some("fx.overdrive")
        );
        assert_eq!(param_by_plock_id(DT2_PARAMS, 0x2A), None);
    }

    // --- describe_param, against the same JS oracle ---------------------------

    fn shape(d: &ParamDesc) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            d.name.unwrap_or_default(), d.label, d.short, d.bipolar, d.min, d.max,
            d.curated, d.auditable(), d.writable(),
            d.plock.map(|p| p.id.to_string()).unwrap_or_default(),
            d.device_kind.unwrap_or_default(),
        )
    }

    #[test]
    fn a_lane_resolves_by_name_or_by_measured_id() {
        // Expectations from the JS: describeParam(paramTableFor('DT2'), {...}).
        assert_eq!(
            shape(&describe_param(DT2_PARAMS, Some("filter.cutoff"), None, Some("DT2"))),
            "filter.cutoff|FLTR CUTOFF|CUTOFF|false|0|127|true|true|true|44|DT2"
        );
        assert_eq!(
            shape(&describe_param(DT2_PARAMS, None, Some(44), Some("DT2"))),
            "filter.cutoff|FLTR CUTOFF|CUTOFF|false|0|127|true|true|true|44|DT2"
        );
        // Name wins over paramId: when this app authored the lane it knows
        // exactly which knob it is, and the id is the weaker evidence.
        assert_eq!(
            shape(&describe_param(DT2_PARAMS, Some("amp.pan"), Some(44), Some("DT2"))),
            "amp.pan|PAN|PAN|true|0|127|true|true|true|65|DT2"
        );
    }

    #[test]
    fn a_lane_in_no_table_is_named_by_its_byte_and_drawn_over_the_raw_range() {
        // The trailing `DT2` is this port's one deviation from the JS, which
        // drops deviceKind on the raw branch — see `describe_param`.
        assert_eq!(
            shape(&describe_param(DT2_PARAMS, None, Some(0x2A), Some("DT2"))),
            "|DT2 param 0x2a|p 0x2a|false|0|65534|false|false|true|42|DT2"
        );
        // No device kind: no prefix, and still not curated.
        assert_eq!(
            shape(&describe_param(DT2_PARAMS, None, Some(0x2A), None)),
            "|param 0x2a|p 0x2a|false|0|65534|false|false|true|42|"
        );
        // Neither identity. `writable` goes false with the paramId: there is no
        // byte to write it as.
        assert_eq!(
            shape(&describe_param(DT2_PARAMS, None, None, None)),
            "|param ??|p ??|false|0|65534|false|false|false||"
        );
        // A name we do not know falls through to the raw path rather than
        // inventing a parameter.
        assert_eq!(
            shape(&describe_param(DT2_PARAMS, Some("filter.morph"), None, Some("DT2"))),
            "|DT2 param ??|p ??|false|0|65534|false|false|false||DT2"
        );
    }

    #[test]
    fn a_raw_lane_passes_its_word_through_untouched() {
        // The only honest scaling for something we cannot scale, and what keeps
        // an imported lane byte-exact on the way back out.
        let raw = describe_param(DT2_PARAMS, None, Some(0x2A), Some("DT2"));
        assert_eq!(raw.stored_from_display(40000.0), Some(40000));
        assert_eq!(raw.display_from_stored(40000), Some(40000));
        // Never the 0xFFFF free-lane sentinel, whatever it is handed.
        assert_eq!(raw.stored_from_display(70000.0), Some(RAW_VALUE_MAX));
        assert!(!raw.auditable(), "a lane we cannot name has no CC to audition it with");
    }

    #[test]
    fn a_display_value_is_clamped_and_rounded_onto_the_midi_axis() {
        let cutoff = DT2_PARAMS[0].describe(Some("DT2"));
        assert_eq!(cutoff.stored_from_display(64.0), Some(16384));
        assert_eq!(cutoff.stored_from_display(-5.0), Some(0));
        assert_eq!(cutoff.stored_from_display(200.0), Some(0x7F00));
        // A drag lands on a fraction; it rounds before it scales, so a lane can
        // never hold a value off the axis it was drawn on.
        assert_eq!(cutoff.clamp_value(63.5), 64);
        assert_eq!(cutoff.stored_from_display(63.5), Some(16384));
        // The box's sub-MIDI fine resolution rounds on the way in, and this is
        // where that loss happens: 0x4880 is between MIDI 72 and 73.
        assert_eq!(cutoff.display_from_stored(0x4880), Some(73));
    }

    #[test]
    fn a_scaling_that_would_overrun_the_word_stops_short_of_the_sentinel() {
        // Every measured factor is 256 today, and 127 × 256 is 0x7F00 — well
        // inside the word — so the ceiling in `stored_from_display` is a guard
        // for a factor nobody has met yet. Deliberately breaking it proved
        // nothing: the descriptor's own `max` was doing the clamping in every
        // case the other tests reach. This is the case that needs the guard, and
        // it is the one that matters most, because an unclamped word saturates
        // to 0xFFFF — the "no lock on this step" sentinel. A lock loud enough to
        // overflow would erase itself.
        let hypothetical = Param {
            plock: Some(scaled_plock(44, 1000)),
            ..DT2_PARAMS[0]
        };
        let d = hypothetical.describe(Some("DT2"));
        assert_eq!(d.stored_from_display(127.0), Some(RAW_VALUE_MAX));
        assert_ne!(d.stored_from_display(127.0), Some(0xFFFF));
    }

    #[test]
    fn an_unmeasured_parameter_can_be_heard_but_never_stored() {
        // The safety net: a parameter with `midi` and no `plock` draws and
        // auditions, and returns `None` rather than a plausible byte. There is no
        // such entry in either table today, which is exactly why this is
        // constructed rather than looked up — the rule has to outlive the tables.
        let unmeasured = Param { plock: None, ..DT2_PARAMS[0] };
        assert!(unmeasured.validate().is_ok());
        let d = unmeasured.describe(Some("DT2"));
        assert!(d.auditable());
        assert!(!d.writable());
        assert_eq!(d.stored_from_display(64.0), None);
        assert_eq!(d.display_from_stored(0x4000), None);
    }

    // --- the Analog Four's p-lock parameter ids ------------------------------

    /// The count, and that every id is unique and in range.
    ///
    /// Ninety-two entries, one per knob turned on the box on 2026-09-01. The
    /// number is asserted because this table's whole claim is that it was
    /// measured exhaustively rather than extrapolated — a silently shorter table
    /// would mean somebody filled a hole in by hand.
    #[test]
    fn the_a4_synth_plock_table_is_ninety_two_measured_ids() {
        assert_eq!(A4_SYNTH_PLOCKS.len(), 92);
        let mut ids: Vec<u8> = A4_SYNTH_PLOCKS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "an id appears twice");
        // 0xFF is a free lane header and 0x80 is an extension marker; neither can
        // be a parameter.
        assert!(A4_SYNTH_PLOCKS.iter().all(|p| p.id != 0xFF && p.id != 0x80));
    }

    /// The nine ids inside the measured range that nobody has mapped.
    ///
    /// Pinned as a *list* rather than left implicit, because the temptation with
    /// a hole between two known neighbours is to assume it continues the page.
    /// A lane arriving on one of these has no name and shows its hex byte.
    #[test]
    fn the_unmapped_ids_are_exactly_the_nine_known_holes() {
        let holes: Vec<u8> = (0x00..=0x64u8)
            .filter(|id| a4_synth_plock_label(*id).is_none())
            .collect();
        assert_eq!(holes, [0x12, 0x13, 0x14, 0x2c, 0x32, 0x60, 0x61, 0x62, 0x63]);
    }

    /// The three ids named by a hand on the box before the full sweep, which are
    /// the ones every later reading was calibrated against.
    #[test]
    fn the_three_originally_named_ids_still_read_the_same() {
        assert_eq!(a4_synth_plock_full_label(0x22).unwrap(), "FLTR1 FRQ");
        assert_eq!(a4_synth_plock_full_label(0x23).unwrap(), "FLTR1 RES");
        assert_eq!(a4_synth_plock_full_label(0x24).unwrap(), "FLTR1 OVR");
    }

    /// The structural shapes, each of which refuted a prediction when it was
    /// measured. They are asserted so that a later edit cannot quietly
    /// regularise the table into the layout it "should" have.
    #[test]
    fn the_four_layout_schemes_are_all_present() {
        // Oscillators interleave on even/odd, anchored at both ends of the page.
        assert_eq!(a4_synth_plock_full_label(0x00).unwrap(), "OSC1 TUN/FIN");
        assert_eq!(a4_synth_plock_full_label(0x01).unwrap(), "OSC2 TUN/FIN");
        assert_eq!(a4_synth_plock_full_label(0x10).unwrap(), "OSC1 PWM");
        assert_eq!(a4_synth_plock_full_label(0x11).unwrap(), "OSC2 PWM");

        // Three envelopes at stride 3, with AMP third rather than first.
        for (i, page) in ["ENV1", "ENV2", "AMP"].iter().enumerate() {
            assert_eq!(
                a4_synth_plock_full_label(0x33 + i as u8).unwrap(),
                format!("{page} ATK")
            );
            assert_eq!(
                a4_synth_plock_full_label(0x36 + i as u8).unwrap(),
                format!("{page} DEC")
            );
        }

        // Two LFOs at stride 2 for the knobs, then blocked for DST and DEP.
        assert_eq!(a4_synth_plock_full_label(0x4c).unwrap(), "LFO1 SPD");
        assert_eq!(a4_synth_plock_full_label(0x4d).unwrap(), "LFO2 SPD");
        assert_eq!(a4_synth_plock_full_label(0x58).unwrap(), "LFO1 DST1");
        assert_eq!(a4_synth_plock_full_label(0x59).unwrap(), "LFO1 DST2");
        assert_eq!(a4_synth_plock_full_label(0x5a).unwrap(), "LFO2 DST1");

        // OSC page 2 pairs AM1/AM2 first — neither row-major nor column-major.
        assert_eq!(a4_synth_plock_full_label(0x18).unwrap(), "OSC AM1");
        assert_eq!(a4_synth_plock_full_label(0x19).unwrap(), "OSC AM2");
        assert_eq!(a4_synth_plock_full_label(0x1a).unwrap(), "OSC SMD");
    }

    /// **The published NRPN numbering does not predict these ids**, which is why
    /// all 92 were measured. `nrpn - 6` fits the filter block exactly and is
    /// wrong everywhere else — this pins the two counter-examples that killed it
    /// so nobody re-derives the rule from the three that fit.
    #[test]
    fn the_nrpn_offset_rule_does_not_hold() {
        // Fits: FLTR1 FREQ is NRPN 40, and 40 - 6 = 34 = 0x22.
        let freq = param_by_name(param_table_for("A4"), "filter.cutoff").unwrap();
        assert_eq!(freq.midi.nrpn.unwrap().1, 40);
        assert_eq!(a4_synth_plock_label(40 - 6).unwrap(), "FRQ");

        // Fails numerically: osc1.level is NRPN 4, and 4 - 6 is not a byte.
        let osc1 = param_by_name(param_table_for("A4"), "osc1.level").unwrap();
        assert_eq!(osc1.midi.nrpn.unwrap().1, 4);

        // Fails structurally: the NRPNs put the two LFO depths ten apart, the
        // pool puts them two apart. Not one ordering with an offset.
        let l1 = param_by_name(param_table_for("A4"), "lfo1.depth").unwrap();
        let l2 = param_by_name(param_table_for("A4"), "lfo2.depth").unwrap();
        assert_eq!(l2.midi.nrpn.unwrap().1 - l1.midi.nrpn.unwrap().1, 10);
        assert_eq!(0x5e - 0x5c, 2, "LFO1 DEP1 is 0x5c and LFO2 DEP1 is 0x5e");
        assert_eq!(a4_synth_plock_full_label(0x5c).unwrap(), "LFO1 DEP1");
        assert_eq!(a4_synth_plock_full_label(0x5e).unwrap(), "LFO2 DEP1");
    }

    /// **These are ids, not scalings**, and the two are still measured
    /// separately — which is why [`A4_SYNTH_PLOCKS`] has 92 entries and
    /// [`A4_PARAMS`] has 13 writable ones.
    ///
    /// This asserted that naming a parameter made *none* of them writable.
    /// Thirteen scalings were then read off the box (`a4_scale_probe`,
    /// 2026-09-01, four runs), so the rule it protects has to be stated the
    /// other way round: an id in this table
    /// does **not** put a `plock` on a `Param`, and the 79 ids with no measured
    /// scaling are still unwritable.
    #[test]
    fn an_id_alone_still_does_not_make_a_parameter_writable() {
        // 0x25 is FLTR1 TRK — measured as an id, never as a scaling.
        assert_eq!(a4_synth_plock_full_label(0x25).unwrap(), "FLTR1 TRK");
        assert!(
            param_by_plock_id(param_table_for("A4"), 0x25).is_none(),
            "an id in A4_SYNTH_PLOCKS is not a curated parameter",
        );
        assert!(
            writable_params_for("A4").len() < A4_SYNTH_PLOCKS.len(),
            "the two tables measure different things and are not converging by accident",
        );
    }

    /// **A bare id must not identify a parameter on the A4**, because its id
    /// space is per track kind. With all thirteen `A4_PARAMS` entries now
    /// carrying measured ids, `param_by_plock_id` would otherwise answer for an
    /// FX-track lane on `0x22` and hand it `filter.cutoff` — curated, editable,
    /// and written back on the wrong axis.
    #[test]
    fn an_a4_id_is_not_admissible_evidence_on_its_own() {
        assert!(!plock_id_identifies_parameter("A4"));
        for kind in ["DT2", "DN2"] {
            assert!(plock_id_identifies_parameter(kind), "{kind}");
        }
        // The table lookup itself still works — it is the *caller* that must
        // decline to use it, which is `PLockLane::param`'s job.
        assert_eq!(
            param_by_plock_id(param_table_for("A4"), 0x22).map(|p| p.name),
            Some("filter.cutoff"),
        );
    }
}
