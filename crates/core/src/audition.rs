// The p-lock audition path: what the engine should send so a lane can be
// *heard*, before a byte of it is written to a box.
//
// Ported from the audition half of `js/roll-bridge.js` — `plockMessagesForStep`
// and `hasAuditableLanes`. This is the half of the p-lock feature that needed no
// reverse engineering at all: the CC and NRPN numbers come from the boxes' own
// published charts, which is why it can ship long before anything writes a
// pattern.
//
// **Why this lives in `core` and not in `engine`.** `PLAN.md` §3 forbids the
// engine from depending on `protocol`, and the whole point of `PLockMap` being
// an injected trait is that the scheduler never learns which CC a knob lives at.
// So the resolution happens here, where the model and the tables already meet,
// and `app` supplies the thin `PLockMap` that turns these into wire messages —
// exactly as `js/main.js` injects `plockMessagesForStep` into `js/midi.js`.
//
// **The values in a lane are display values, not stored words.** MIDI 0–127 for
// a curated parameter, the raw uint16 for a lane off a box whose parameter we
// cannot name. `js/state.js` gives the reason and it is a good one: a lane can be
// authored and auditioned before anyone knows what its uint16 would be. The
// stored word only appears at the pattern's lane pool, which is unported.

use digi_protocol::params::{curated_param, param_table_for};

use crate::model::PLockLane;

/// One parameter change to send, resolved down to wire terms so the engine stays
/// device-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamMessage {
    /// What the box's own UI calls this knob. For the UI to name what is being
    /// sent; nothing on the wire uses it.
    pub label: &'static str,
    pub nrpn: Option<(u8, u8)>,
    pub cc: Option<u8>,
    /// The display value, for a plain 7-bit CC.
    pub value7: u8,
    /// The display value in the top 7 bits, which puts the box's parameter where
    /// a plain CC of that value would. The lane's axis is 0–127, so the low bits
    /// are unused; a future high-resolution lane can fill them without changing
    /// anything here.
    pub value14: u16,
}

/// Every lane holding a value at `step`, resolved against `device_kind`'s table.
///
/// Three kinds of lane produce nothing, all deliberately:
///
/// * **A lane for a different box.** A DT2 lane sent to a DN2 would ride the
///   wrong fader — pan is CC 90 on a DT2 and CC 89 on a DN2, where 89 is Volume.
///   The JS cannot reach this case, because the browser drives one box; this app
///   drives several, so the mismatch is real and refusing is the only safe
///   answer. `js/roll-bridge.js` refuses the same mismatch on the *write* path,
///   which is where the JS does meet it.
/// * **A lane with no `device_kind` at all.** `js/state.js`: "a lane without this
///   is meaningless", because the two boxes disagree on both numbering schemes.
/// * **A lane whose parameter is in no curated table** — one captured off a box
///   holding a knob we cannot name. It is carried byte-exact and never sent,
///   because there is no CC to send it on.
pub fn plock_messages_for_step(
    lanes: &[PLockLane],
    step: usize,
    device_kind: &str,
) -> Vec<ParamMessage> {
    let table = param_table_for(device_kind);
    let mut out = Vec::new();
    for lane in lanes {
        if lane.device_kind.as_deref() != Some(device_kind) {
            continue;
        }
        let Some(value) = lane.values.get(step).copied().flatten() else {
            continue;
        };
        let Some(param) = curated_param(table, lane.name.as_deref(), lane.param_id) else {
            continue;
        };
        if !param.auditable() {
            continue;
        }
        let display = value.min(127) as u8;
        out.push(ParamMessage {
            label: param.label,
            nrpn: param.midi.nrpn,
            cc: param.midi.cc,
            value7: display,
            value14: (display as u16) << 7,
        });
    }
    out
}

/// Would playing this pattern move anything on the box?
///
/// The UI asks so it can warn — and only when it actually would. Auditioning a
/// lane genuinely moves the destination's parameters and nothing puts them back
/// afterwards, exactly as turning the knob by hand would.
pub fn has_auditable_lanes(lanes: &[PLockLane], device_kind: &str) -> bool {
    let table = param_table_for(device_kind);
    lanes.iter().any(|lane| {
        lane.device_kind.as_deref() == Some(device_kind)
            && lane.values.iter().any(Option::is_some)
            && curated_param(table, lane.name.as_deref(), lane.param_id)
                .is_some_and(|p| p.auditable())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expectations derived by running the JS:
    ///
    /// ```text
    /// cd ../digi-roll && node --input-type=module -e "
    /// import { plockMessagesForStep } from './js/roll-bridge.js';
    /// import { makePLockLane } from './js/state.js';
    /// const cut = makePLockLane({name:'filter.cutoff', deviceKind:'DT2',
    ///   values:{0:0,1:64,2:127}});
    /// console.log(JSON.stringify(plockMessagesForStep([cut], 1)));"
    /// ```
    fn lane(name: Option<&str>, param_id: Option<u16>, kind: Option<&str>, at: &[(usize, u16)]) -> PLockLane {
        let mut values = vec![None; 128];
        for (step, v) in at {
            values[*step] = Some(*v);
        }
        PLockLane::new(
            name.map(str::to_string),
            param_id,
            kind.map(str::to_string),
            false,
            values,
        )
        .unwrap()
    }

    fn shape(ms: &[ParamMessage]) -> Vec<(&'static str, Option<(u8, u8)>, Option<u8>, u8, u16)> {
        ms.iter().map(|m| (m.label, m.nrpn, m.cc, m.value7, m.value14)).collect()
    }

    #[test]
    fn a_lane_sends_its_parameter_at_the_steps_it_holds() {
        let cut = lane(Some("filter.cutoff"), None, Some("DT2"), &[(0, 0), (1, 64), (2, 127)]);
        assert_eq!(
            shape(&plock_messages_for_step(&[cut.clone()], 0, "DT2")),
            [("FLTR CUTOFF", Some((1, 20)), Some(74), 0, 0)]
        );
        assert_eq!(
            shape(&plock_messages_for_step(&[cut.clone()], 1, "DT2")),
            [("FLTR CUTOFF", Some((1, 20)), Some(74), 64, 8192)]
        );
        assert_eq!(
            shape(&plock_messages_for_step(&[cut.clone()], 2, "DT2")),
            [("FLTR CUTOFF", Some((1, 20)), Some(74), 127, 16256)]
        );
        // A step the lane has no lock on sends nothing at all — not a value of
        // zero, which would slam the filter shut on every unlocked step.
        assert!(plock_messages_for_step(&[cut], 3, "DT2").is_empty());
    }

    #[test]
    fn a_lane_off_a_box_resolves_by_its_paramid() {
        let by_id = lane(None, Some(44), Some("DT2"), &[(0, 64)]);
        assert_eq!(
            shape(&plock_messages_for_step(&[by_id], 0, "DT2")),
            [("FLTR CUTOFF", Some((1, 20)), Some(74), 64, 8192)]
        );
    }

    #[test]
    fn a_parameter_with_no_cc_still_goes_out_over_nrpn() {
        // The DN2's whole LFO3 has no CC in the appendix. This lane is the reason
        // the audition path prefers NRPN rather than treating it as a fallback.
        let lfo3 = lane(Some("lfo3.depth"), None, Some("DN2"), &[(0, 72)]);
        assert_eq!(
            shape(&plock_messages_for_step(&[lfo3], 0, "DN2")),
            [("LFO3 DEPTH", Some((1, 72)), None, 72, 9216)]
        );
    }

    #[test]
    fn a_lane_for_the_other_box_is_refused_rather_than_translated() {
        // The case the JS cannot reach and this app can. A DT2 pan lane sent to a
        // DN2 on CC 90 would move something else entirely; the DN2's pan is 89,
        // and the DT2's 89 is Volume.
        let dt2_pan = lane(Some("amp.pan"), None, Some("DT2"), &[(0, 100)]);
        assert!(plock_messages_for_step(&[dt2_pan.clone()], 0, "DN2").is_empty());
        assert!(!has_auditable_lanes(&[dt2_pan.clone()], "DN2"));
        // ...and it is not that the parameter is unknown on a DN2. It is known,
        // under the same name, at a different number. Refusing is a choice.
        assert!(!plock_messages_for_step(&[dt2_pan], 0, "DT2").is_empty());
    }

    #[test]
    fn a_lane_with_no_device_kind_is_meaningless_and_silent() {
        let orphan = lane(Some("filter.cutoff"), None, None, &[(0, 64)]);
        assert!(plock_messages_for_step(&[orphan.clone()], 0, "DT2").is_empty());
        assert!(!has_auditable_lanes(&[orphan], "DT2"));
    }

    #[test]
    fn a_lane_whose_knob_we_cannot_name_is_carried_but_never_sent() {
        // 0x2A is in neither table. It survives in the pattern byte-exact; there
        // is simply no CC to audition it on, and inventing one would move a knob
        // nobody asked for.
        let raw = lane(None, Some(0x2A), Some("DT2"), &[(0, 40000)]);
        assert!(plock_messages_for_step(&[raw.clone()], 0, "DT2").is_empty());
        assert!(!has_auditable_lanes(&[raw], "DT2"));
    }

    #[test]
    fn several_lanes_on_one_step_all_go_out_in_lane_order() {
        let lanes = vec![
            lane(Some("filter.cutoff"), None, Some("DT2"), &[(4, 30)]),
            lane(Some("amp.pan"), None, Some("DT2"), &[(4, 64)]),
            lane(Some("fx.delaySend"), None, Some("DT2"), &[(9, 100)]),
        ];
        assert_eq!(
            shape(&plock_messages_for_step(&lanes, 4, "DT2")),
            [
                ("FLTR CUTOFF", Some((1, 20)), Some(74), 30, 3840),
                ("PAN", Some((1, 38)), Some(90), 64, 8192),
            ]
        );
    }

    #[test]
    fn an_empty_lane_is_not_something_to_warn_about() {
        // The warning exists so playing does not silently move the box's own
        // parameters. A lane with no values moves nothing, and a warning that
        // cries wolf is worse than none.
        let empty = lane(Some("filter.cutoff"), None, Some("DT2"), &[]);
        assert!(!has_auditable_lanes(&[empty], "DT2"));
        let held = lane(Some("filter.cutoff"), None, Some("DT2"), &[(7, 1)]);
        assert!(has_auditable_lanes(&[held], "DT2"));
    }

    #[test]
    fn a_value_off_the_display_axis_is_clamped_rather_than_wrapped() {
        // Nothing should author one — the strip clamps, and an import scales into
        // range — but a wrapped value would send a *different* parameter value
        // rather than a wrong one, and 14-bit NRPN would carry the mistake.
        let wild = lane(Some("filter.cutoff"), None, Some("DT2"), &[(0, 9000)]);
        let msgs = plock_messages_for_step(&[wild], 0, "DT2");
        assert_eq!(msgs[0].value7, 127);
        assert_eq!(msgs[0].value14, 16256);
    }
}
