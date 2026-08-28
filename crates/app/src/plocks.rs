// The `PLockMap` the engine plays through: p-lock lanes, resolved and turned
// into wire messages.
//
// This is `js/main.js` injecting `plockMessagesForStep` into `js/midi.js`, and
// it sits in `app` for the reason that seam exists at all. `PLAN.md` §3 forbids
// `engine` from depending on `protocol`, so the scheduler cannot know which CC a
// knob lives at; `core::audition` does the resolving, `engine` owns the trait,
// and this is the only place both are in scope.
//
// **Playing a session with lanes now moves the box's own parameters**, and
// nothing puts them back afterwards — the same as turning the knob by hand. That
// is what auditioning means and it is what the JS does; the UI is the thing that
// has to say so.

use std::collections::HashMap;

use digi_core::audition::plock_messages_for_step;
use digi_core::device::DeviceId;
use digi_core::model::PLockLane;
use digi_core::session::Session;
use digi_engine::event::MidiMsg;
use digi_engine::PLockMap;

/// Resolves each lane against the table of the box it is bound for.
#[derive(Debug, Clone, Default)]
pub struct CuratedPLocks {
    /// Which box each device is, as `params::DEVICE_KINDS` spells it — `DT2` /
    /// `DN2` / `A4`. A device with no curated table is absent, and a lane
    /// bound for it sends nothing: we have no parameters for a box we cannot
    /// name.
    ///
    /// Keyed off the **param tables, not the SysEx spec** — until 2026-08-24
    /// it was `model.spec()?.device`, which was the same set of boxes right up
    /// to the moment the A4 shipped live-only: a box with published CC/NRPN
    /// and no dump format. Hearing a lane and parsing a dump are different
    /// capabilities, and this map is about hearing.
    kinds: HashMap<DeviceId, &'static str>,
}

impl CuratedPLocks {
    /// Built from the session the engine is about to be spawned against.
    ///
    /// A snapshot rather than a live view, and that is safe for one reason worth
    /// stating: a device's model never changes, and a device *added* to the
    /// session only matters here once it has a port to send on — which moves the
    /// port table and rebuilds the transport, and this map with it.
    pub fn new(session: &Session) -> Self {
        Self {
            kinds: session
                .devices
                .iter()
                .filter_map(|d| {
                    Some((d.id, digi_protocol::params::device_kind_key(d.model.key)?))
                })
                .collect(),
        }
    }
}

impl PLockMap for CuratedPLocks {
    fn messages(
        &self,
        device: DeviceId,
        channel: u8,
        lanes: &[PLockLane],
        step: u64,
        out: &mut Vec<MidiMsg>,
    ) {
        let Some(kind) = self.kinds.get(&device) else {
            return;
        };
        for m in plock_messages_for_step(lanes, step as usize, kind) {
            // **NRPN first, CC only as a fallback**, for the three reasons the
            // boxes' own appendices give: NRPN reaches parameters that have no CC
            // at all (every one of the DN2's LFO3 controls), it carries the full
            // 14 bits the high-resolution parameters want, and its numbering is
            // largely shared between the boxes where the CC numbering very much
            // is not. `js/midi.js` chooses in this order for the same reasons.
            if let Some((msb, lsb)) = m.nrpn {
                out.push(MidiMsg::Nrpn { channel, msb, lsb, value14: m.value14 });
            } else if let Some(cc) = m.cc {
                out.push(MidiMsg::ControlChange { channel, controller: cc, value: m.value7 });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::device::{model_for_key, Device};

    fn lane(name: &str, kind: &str, at: &[(usize, u16)]) -> PLockLane {
        let mut values = vec![None; 128];
        for (step, v) in at {
            values[*step] = Some(*v);
        }
        PLockLane::new(Some(name.into()), None, Some(kind.into()), false, values).unwrap()
    }

    /// A session holding one DT2 and one DN2, and the ids to address them by.
    fn two_boxes() -> (Session, DeviceId, DeviceId) {
        let mut session = Session::default();
        let m = |k| model_for_key(k).expect("a model this test names");
        session.devices.push(Device::new("DT2", m("DT2"), 1));
        session.devices.push(Device::new("DN2", m("DN2"), 1));
        let (dt2, dn2) = (session.devices[0].id, session.devices[1].id);
        (session, dt2, dn2)
    }

    fn messages(map: &CuratedPLocks, device: DeviceId, lanes: &[PLockLane]) -> Vec<MidiMsg> {
        let mut out = Vec::new();
        map.messages(device, 3, lanes, 0, &mut out);
        out
    }

    #[test]
    fn a_lane_goes_out_as_nrpn_on_the_tracks_own_channel() {
        let (session, dt2, _) = two_boxes();
        let map = CuratedPLocks::new(&session);
        assert_eq!(
            messages(&map, dt2, &[lane("filter.cutoff", "DT2", &[(0, 64)])]),
            [MidiMsg::Nrpn { channel: 3, msb: 1, lsb: 20, value14: 8192 }]
        );
    }

    #[test]
    fn a_parameter_with_no_nrpn_falls_back_to_its_cc() {
        // The DT2's FX page has no NRPN in the appendix — an omission rather than
        // a statement, most likely, but CC is what we have, and a lane that would
        // otherwise be silent is worth 7 bits.
        let (session, dt2, _) = two_boxes();
        let map = CuratedPLocks::new(&session);
        assert_eq!(
            messages(&map, dt2, &[lane("fx.overdrive", "DT2", &[(0, 100)])]),
            [MidiMsg::ControlChange { channel: 3, controller: 57, value: 100 }]
        );
    }

    #[test]
    fn the_same_knob_goes_to_a_different_number_on_each_box() {
        let (session, dt2, dn2) = two_boxes();
        let map = CuratedPLocks::new(&session);
        // Both are pan, both are NRPN 1/38 — the numbering the boxes agree on.
        assert_eq!(
            messages(&map, dt2, &[lane("amp.pan", "DT2", &[(0, 100)])]),
            [MidiMsg::Nrpn { channel: 3, msb: 1, lsb: 38, value14: 12800 }]
        );
        assert_eq!(
            messages(&map, dn2, &[lane("amp.pan", "DN2", &[(0, 100)])]),
            [MidiMsg::Nrpn { channel: 3, msb: 1, lsb: 38, value14: 12800 }]
        );
        // And a DT2 lane aimed at the DN2 sends nothing, rather than sending the
        // DT2's CC 90 — which on a DN2 is not pan.
        assert!(messages(&map, dn2, &[lane("amp.pan", "DT2", &[(0, 100)])]).is_empty());
    }

    #[test]
    fn a_device_the_map_has_never_heard_of_sends_nothing() {
        // A stale `DeviceId`, or a live-only model with no SysEx spec: either way
        // there is no table, and a table-less guess is how a knob moves that
        // nobody asked to move.
        let (session, _, _) = two_boxes();
        let map = CuratedPLocks::new(&session);
        let stranger = DeviceId::next();
        assert!(messages(&map, stranger, &[lane("filter.cutoff", "DT2", &[(0, 64)])]).is_empty());
    }

    #[test]
    fn an_empty_map_is_what_no_devices_looks_like() {
        let map = CuratedPLocks::new(&Session::default());
        assert!(messages(&map, DeviceId::next(), &[lane("filter.cutoff", "DT2", &[(0, 64)])]).is_empty());
    }
}
