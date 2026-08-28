// Auto-connect: doing the obvious thing without being asked.
//
// Identify has matched a reply to a device since Phase 3, and the manual pickers
// landed 2026-08-17. What was missing is nobody having to press anything: plug a
// DT2 in, and the DT2 row in this session should have its ports.
//
// ## Discovery adds boxes, 2026-08-24
//
// Until now a reply could only land on a device already in the session, and the
// session always opened with a DT2 and a DN2 in it. Both halves changed at once:
// the app now opens on an *empty* session, and a handshake that agrees with its
// port name but matches no free device **adds one** ([`Placement::Add`], landed
// on the UI thread in [`AutoConnect::land`], never on the worker). First launch
// is: plug in whatever Elektron boxes you own, and the desk is them.
//
// The rule that keeps this from being a nuisance: **discovery adds, it never
// removes** — an unplugged box goes amber exactly as before, its patterns
// untouched. And removing a box is a decision the app must not overrule, so the
// remove control tells this module to [`AutoConnect::decline`] the box's port
// pair: it is not asked about again until it leaves the port list, which makes
// unplug-and-replug the gesture that means "I want it back" — the same gesture
// that already clears a spent handshake backoff.
//
// ## The two rules this must not break, and how each is met
//
//  1. **It may never bind a port a user chose by hand.** Met by never *moving*
//     anything: a candidate port pair is one where neither end is bound to any
//     device in the session, and the only device it may be given to is one with
//     **no ports at all**. There is no "chosen by hand" flag anywhere in
//     `DeviceIo` and this deliberately does not add one — an empty port end is
//     the one state nobody can have chosen, so the rule falls out of the model
//     instead of being carried by a field that a later loader could drop.
//
//     Note what this gives up. `Session::bind_identity`'s third preference —
//     "the only device of that model, even if it is on other ports" — is exactly
//     right for a person pressing Identify, who has just told the app which
//     cable to look down. It is exactly wrong here: it would re-point a box the
//     user had aimed somewhere, silently, because something that looked like it
//     appeared on another socket. So this uses [`destination`] and
//     `bind_identity_to`, not `bind_identity`.
//
//  2. **It may never make the app the second clock master on a desk**
//     (PLAN.md §7). The app cannot read a box's CLOCK SEND setting, so it cannot
//     check this directly — what it can do is never grab a port it has not
//     positively identified as a box this build knows. A candidate needs a port
//     name that looks like an Elektron *and* an identity handshake that comes
//     back agreeing, so an IAC bus with somebody else's sequencer on it is never
//     bound and never clocked. And it never touches `takes_clock`: a box the user
//     took off the clock stays off it, including across a replug, because
//     rebinding an existing device leaves everything but the ports alone.
//
//     **One stated exception: a model with `answers_identity: false`.** For
//     such a box the handshake is not pending, it is *never coming*, so
//     waiting for one means auto-connect simply never works for that model.
//     The port name is then the identification: it comes off the USB device
//     descriptor, which only the box itself writes. What rule 2 still
//     guarantees is unchanged for every model that *can* answer — an IAC bus
//     is still never grabbed (no Elektron name, no candidate), and a port
//     named for a DT2 still has to prove it is one.
//
//     **No shipped model takes this exception.** It was written 2026-08-24 for
//     the Analog Four mk1, on the guess that a 2013 box predated the API; on
//     2026-08-28 the box answered 0x01 on the first try and took the ordinary
//     path instead. [`AutoConnect::adopt_by_name`] and its tests are kept
//     against the model that eventually needs them, and the tests reach it by
//     calling it rather than by any desk arriving at it.
//
// ## Shape
//
// [`candidates`] and [`destination`] are pure and hold every rule above, so all
// of it is unit-tested with no port in sight — the same bargain
// `Session::set_device_port` makes. The struct below is the timer, the worker
// thread and one line of UI.
//
// **One handshake at a time, and a fixed budget of them per pair.** The identity
// request blocks for up to 5 s with two retries, and a desk with an IAC bus and
// two soft synths on it would otherwise spend its life asking things that are
// never going to answer. So a pair that fails is put on a backoff
// ([`RETRY_BACKOFF`]) and, once that is spent, not asked again until it
// disappears from the port list.
//
// **It used to be one attempt, and that was a bug.** A box's MIDI ports
// enumerate before the box can answer SysEx, so a DT2 caught mid-boot failed its
// single handshake and was never asked again — not in three seconds, not in an
// hour, not until the cable came out, which is the one thing the person staring
// at a powered-on box has no reason to try. Nothing in the suite could see it
// either: every test drove `candidates` and `destination`, which are pure and
// were never wrong. The fault was entirely in how many times the impure part was
// willing to ask.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use digi_core::device::{model_for_slug, PortEnd, PortRef};
use digi_core::{Device, DeviceId, Session};
use digi_midi::{ElektronDevice, PortBinding, PortInfo};
use digi_protocol::device::DeviceIdentity;
use eframe::egui::{self, Ui};

use crate::ui::ports::PortsPanel;

/// How often the ports are re-enumerated while nothing else is going on.
///
/// Enumeration is cheap but not free — it opens a MIDI client to ask — so this
/// is slow enough to be invisible and quick enough that plugging a box in feels
/// like it just worked.
const SCAN_EVERY: Duration = Duration::from_secs(3);

/// A port pair that looks like a box nobody in this session is using.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// What the *name* says it is. A guess, which the handshake then has to
    /// agree with — see rule 2.
    pub slug: &'static str,
    pub input: PortRef,
    pub output: PortRef,
}

/// Every port pair worth asking "who are you?", given what is plugged in and
/// what the session already has.
///
/// Two conditions, one per rule:
///
/// * the port *name* identifies a box this build has a **model** for (rule 2 —
///   an unknown port is never grabbed; a box the handshake can name but the
///   table cannot seat, like a gen-1 Digitakt, is skipped for the same reason);
/// * neither end is bound to any device in this session (rule 1 — nothing is
///   ever taken off anybody).
///
/// There is deliberately no "is there room" condition any more: a pair with no
/// free device of its model is exactly the pair whose reply will *add* one
/// ([`Placement::Add`]). What keeps a removed box from boomeranging straight
/// back in is not this function — a declined pair is still a candidate — but
/// the spent [`Attempt`] that [`AutoConnect::decline`] parks on it.
pub fn candidates(session: &Session, inputs: &[PortInfo], outputs: &[PortInfo]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    // Two identical boxes give two identically *named* port pairs, so the second
    // input must not pair with the first's output — hence a claim list of its
    // own rather than only asking the session what is taken.
    let mut claimed: HashSet<&str> = HashSet::new();
    for input in inputs {
        let Some(slug) = input.slug else { continue };
        if model_for_slug(slug).is_none() {
            continue;
        }
        if bound(session, &input.id, &input.name) {
            continue;
        }
        // Elektron boxes expose an input and an output with the same name, which
        // is why pairing by name is right here even though *binding* prefers the
        // id: the two ends are separate namespaces and share no id.
        let Some(output) = outputs.iter().find(|o| {
            o.name == input.name
                && !claimed.contains(o.id.as_str())
                && !bound(session, &o.id, &o.name)
        }) else {
            continue;
        };
        claimed.insert(output.id.as_str());
        out.push(Candidate {
            slug,
            input: PortRef { id: input.id.clone(), name: input.name.clone() },
            output: PortRef { id: output.id.clone(), name: output.name.clone() },
        });
    }
    out
}

/// Is either end of any device in this session already on this port?
fn bound(session: &Session, id: &str, name: &str) -> bool {
    let port = PortRef { id: id.to_string(), name: name.to_string() };
    session
        .devices
        .iter()
        .flat_map(|d| [d.io.input.as_ref(), d.io.output.as_ref()])
        .flatten()
        .any(|p| p.same_port(&port))
}

/// Which device an auto-connect reply may be given to, or `None`.
///
/// **Stricter than [`Session::bind_identity`] on purpose**, and the strictness is
/// rule 1: only a device of the reply's model that has *no ports at all*. A box
/// the user aimed at an IAC bus is not re-pointed because something that says it
/// is a Digitakt turned up on another socket.
pub fn destination(session: &Session, identity: &DeviceIdentity) -> Option<DeviceId> {
    let model = model_for_slug(&identity.slug)?;
    session
        .devices
        .iter()
        .find(|d| d.model == model && !d.has_ports())
        .map(|d| d.id)
}

/// What to do with a handshake that came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// Bind it here.
    Bind(DeviceId),
    /// Every box of that model already has ports — so this is a box the session
    /// has never seen, and the desk grows a row for it. Until 2026-08-24 this
    /// arm was `NoRoom`, a silent decline; discovery-first turned it around.
    Add(&'static digi_core::DeviceModel),
    /// The port name said one box and the reply says another. Left for the
    /// person with the Identify button, who can see both — the sentence is the
    /// one shown.
    Disagrees(String),
}

/// The whole decision about a reply, as a value.
///
/// A function rather than three lines inside the worker's landing code, because
/// this is where rule 2's *second* half lives: the port name is a guess and the
/// handshake is the answer, and a reply that does not match what the port
/// claimed is a desk this app should not be quietly rearranging. Testable
/// without a port, a thread or a window.
pub fn placement(
    session: &Session,
    claimed: &str,
    identity: &DeviceIdentity,
) -> Placement {
    if identity.slug != claimed {
        return Placement::Disagrees(format!(
            "{} answered on a port named for a {claimed} — left alone; use Identify",
            identity.name
        ));
    }
    if let Some(device) = destination(session, identity) {
        return Placement::Bind(device);
    }
    match model_for_slug(&identity.slug) {
        Some(model) => Placement::Add(model),
        // Unreachable off a real scan — `candidates` already required a model
        // for the *claimed* slug and the reply agrees with it — but this
        // function is public and pure, so it answers for every input: a box
        // the table cannot seat is left for a human, same as a disagreement.
        None => Placement::Disagrees(format!(
            "{} answered, but this build has no model for it — left alone",
            identity.name
        )),
    }
}

/// How long to wait before asking a pair that did not answer, by how many times
/// it has now failed. A pair that exhausts this list is not asked again until it
/// is unplugged.
///
/// **This list exists because a box's MIDI ports appear before the box can
/// answer SysEx.** The original design asked each pair exactly once and kept the
/// failure until the ports disappeared, which is right for an IAC bus with
/// nothing behind it and wrong for a box that is still booting: it burns the one
/// attempt on a port that would have answered a second later, and no amount of
/// waiting gets another — only pulling the cable does. That is the worst
/// possible shape for a bug report, because the person is holding a box that is
/// plugged in, powered on, and ignored.
///
/// The numbers spend ~65 s of patience across four attempts and then stop, which
/// is long enough to outlast a cold boot and short enough that a desk full of
/// soft synths is not interrogated forever. An attempt costs up to 15 s of a
/// worker thread ([`digi_midi`]'s 5 s timeout with two retries), so these are
/// gaps between attempts rather than a polling rate.
const RETRY_BACKOFF: [Duration; 3] =
    [Duration::from_secs(5), Duration::from_secs(15), Duration::from_secs(45)];

/// What has been asked of one port pair, and whether it may be asked again.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Attempt {
    failures: usize,
    /// When this pair becomes eligible again. `None` once its patience is spent
    /// — the old permanent-failure state, now reached after four tries instead
    /// of one.
    retry_at: Option<Instant>,
}

impl Attempt {
    /// The state a pair is in the moment its handshake is started.
    fn started() -> Self {
        Self { failures: 0, retry_at: None }
    }

    /// Record a handshake that did not answer, and schedule the next attempt.
    fn failed(&mut self, now: Instant) {
        self.retry_at = RETRY_BACKOFF.get(self.failures).map(|wait| now + *wait);
        self.failures += 1;
    }

    /// Whether this pair may be asked again at `now`.
    fn due(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|at| now >= at)
    }
}

/// What one handshake came back with.
struct Reply {
    candidate: Candidate,
    identity: Result<DeviceIdentity, String>,
}

pub struct AutoConnect {
    /// Off is a real setting: a desk being cabled by hand does not want the app
    /// claiming sockets while it is being rearranged.
    enabled: bool,
    pending: Option<Receiver<Reply>>,
    /// Pairs already asked, by `(input id, output id)`, and when each may be
    /// asked again. Dropped when the port disappears, so a replug is still a
    /// fresh chance — it is just no longer the *only* one.
    tried: HashMap<(String, String), Attempt>,
    last_scan: Option<Instant>,
    /// The last thing this did, for the one line the panel shows.
    said: Option<String>,
}

impl Default for AutoConnect {
    fn default() -> Self {
        Self {
            enabled: true,
            pending: None,
            tried: HashMap::new(),
            last_scan: None,
            said: None,
        }
    }
}

impl AutoConnect {
    /// Look for something to connect, and take a landed handshake.
    ///
    /// Returns whether the session changed, which the shell folds into the same
    /// `edited` flag a hand-picked port sets: a routing change has to reach the
    /// engine, or the box would be bound and silent until the next edit.
    ///
    /// `busy` holds it off while a transfer, a write or a sync is running: those
    /// hold the box's ports open, and enumerating underneath one of them is a
    /// thing neither this app nor CoreMIDI has any reason to be good at.
    ///
    /// It takes the [`PortsPanel`] rather than two slices and a refresh closure
    /// because that panel *owns* the enumeration — one list per frame, and every
    /// view of the desk agreeing about what is plugged in, which is the same
    /// invariant `ui::devices` is built on. The rules themselves are in
    /// [`candidates`] and [`destination`], which need none of this.
    pub fn tick(
        &mut self,
        session: &mut Session,
        ports: &mut PortsPanel,
        busy: bool,
        ctx: &egui::Context,
    ) -> bool {
        if let Some(rx) = &self.pending {
            if let Ok(reply) = rx.try_recv() {
                self.pending = None;
                return self.land(session, reply);
            }
            // Nothing wakes the UI thread when the worker moves.
            ctx.request_repaint_after(Duration::from_millis(100));
            return false;
        }
        if !self.enabled || busy {
            return false;
        }

        let due = self.last_scan.is_none_or(|at| at.elapsed() >= SCAN_EVERY);
        if !due {
            // The scan is on a timer rather than every frame, so the frame after
            // the timer expires has to be asked for — an idle window paints when
            // something happens, and nothing is happening.
            ctx.request_repaint_after(SCAN_EVERY);
            return false;
        }
        self.last_scan = Some(Instant::now());
        // Re-enumerate through the panel that owns the cached list, so every view
        // of the desk this frame is still looking at one list.
        ports.refresh();

        let found = candidates(session, ports.inputs(), ports.outputs());
        // Forget failures for ports that have gone: unplugging and replugging is
        // how a person retries, and it should work.
        let present: HashSet<(String, String)> = found
            .iter()
            .map(|c| (c.input.id.clone(), c.output.id.clone()))
            .collect();
        self.tried.retain(|pair, _| present.contains(pair));

        // A pair nobody has asked yet outranks one serving out its backoff, so a
        // box plugged in just now is never left waiting behind an IAC bus that
        // has already declined to answer three times. Within each group the port
        // order decides, as before.
        let now = Instant::now();
        let mut due = None;
        for c in found {
            let key = (c.input.id.clone(), c.output.id.clone());
            match self.tried.get(&key) {
                None => {
                    due = Some((key, c));
                    break;
                }
                Some(a) if a.due(now) && due.is_none() => due = Some((key, c)),
                Some(_) => {}
            }
        }
        let Some((key, candidate)) = due else {
            // Something may still be waiting on a backoff that has not matured,
            // and nothing else will wake this window to notice.
            if self.tried.values().any(|a| a.retry_at.is_some()) {
                ctx.request_repaint_after(SCAN_EVERY);
            }
            return false;
        };
        // A model that cannot answer is not asked — rule 2's stated exception.
        // The port name is the whole identification, so the bind happens right
        // here on the UI thread: no worker, no backoff, and no entry in
        // `tried` (there is no failure mode to remember — a *declined* pair
        // never got this far, because the due-check above skipped it).
        if model_for_slug(candidate.slug).is_some_and(|m| !m.answers_identity) {
            return self.adopt_by_name(session, candidate);
        }
        self.tried.entry(key).or_insert_with(Attempt::started).retry_at = None;
        self.start(candidate);
        false
    }

    /// Bind a box whose model never answers the identity request, on its port
    /// name alone: onto a free device of that model, or onto a row added for
    /// it. Through [`Session::set_device_port`] rather than `bind_identity_to`,
    /// which keeps `build`/`version` empty — nothing answered, and an OS
    /// report beside a name-guessed port would be exactly the plausible lie
    /// that rule is there to prevent.
    fn adopt_by_name(&mut self, session: &mut Session, candidate: Candidate) -> bool {
        let Some(model) = model_for_slug(candidate.slug) else { return false };
        let free = session
            .devices
            .iter()
            .find(|d| d.model == model && !d.has_ports())
            .map(|d| d.id);
        let (device, added) = match free {
            Some(id) => (id, false),
            // One bank of slots, same as a handshaken box gets in `land`.
            None => {
                let name = session.suggested_name(model);
                (session.add_device(Device::new(name, model, 16)), true)
            }
        };
        session.set_device_port(device, PortEnd::Input, Some(candidate.input));
        session.set_device_port(device, PortEnd::Output, Some(candidate.output));
        let name = session.device(device).map(|d| d.name.clone()).unwrap_or_default();
        // "by its port name" is load-bearing copy: it is the one honest
        // difference from the handshaken lines above it, and the reader whose
        // desk has a mislabelled port deserves to know which kind of claim
        // this was.
        self.said = Some(match added {
            true => format!("Added {name} — {} by its port name", model.display),
            false => format!("Connected {name} — {} by its port name", model.display),
        });
        true
    }

    fn start(&mut self, candidate: Candidate) {
        let input = PortBinding { id: candidate.input.id.clone(), name: candidate.input.name.clone() };
        let output =
            PortBinding { id: candidate.output.id.clone(), name: candidate.output.name.clone() };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let identity = ElektronDevice::open(&input, &output)
                .and_then(|mut d| d.identify())
                .map_err(|e| e.to_string());
            let _ = tx.send(Reply { candidate, identity });
        });
        self.pending = Some(rx);
    }

    /// Bind a reply, or say why it was not bound. Returns whether the session
    /// changed.
    fn land(&mut self, session: &mut Session, reply: Reply) -> bool {
        let Ok(identity) = reply.identity else {
            // Silent: a port that does not answer is the normal state of an IAC
            // bus, not news. But it is also the state of a box whose ports have
            // enumerated a moment before its SysEx handler is up, so the pair
            // earns another attempt rather than a life sentence — see
            // `RETRY_BACKOFF`. Only a *handshake* failure retries; `NoRoom` and
            // `Disagrees` below are decisions, and asking again cannot change
            // either one.
            let key = (reply.candidate.input.id.clone(), reply.candidate.output.id.clone());
            self.tried.entry(key).or_insert_with(Attempt::started).failed(Instant::now());
            return false;
        };
        let (device, added) = match placement(session, reply.candidate.slug, &identity) {
            Placement::Bind(device) => (device, false),
            // A box the session has never seen. Adding it here — on the UI
            // thread, off a handshake that answered *and* agreed with its port
            // name — is what makes first launch "plug in what you own". One
            // bank of slots, same as `two_box_session` gives its fixtures.
            Placement::Add(model) => {
                let name = session.suggested_name(model);
                (session.add_device(Device::new(name, model, 16)), true)
            }
            Placement::Disagrees(why) => {
                self.said = Some(why);
                return false;
            }
        };
        match session.bind_identity_to(device, &identity, reply.candidate.input, reply.candidate.output) {
            Ok(()) => {
                let name = session.device(device).map(|d| d.name.clone()).unwrap_or_default();
                self.said = Some(match added {
                    true => format!("Added {name} — {}", identity.name),
                    false => format!("Connected {name} — {}", identity.name),
                });
                true
            }
            Err(e) => {
                // Unreachable for a row this frame just added — its model *is*
                // the reply's — but if it ever happens the row must not stay: a
                // portless ghost box nobody asked for is worse than no bind.
                if added {
                    session.remove_device(device);
                }
                self.said = Some(format!("Could not place {}: {e}", identity.name));
                false
            }
        }
    }

    /// Park a port pair so discovery leaves it alone until it is unplugged.
    ///
    /// Called by the remove control in `ui::devices`: without this, removing a
    /// box whose cable is still in would grow it back on the next scan. A spent
    /// [`Attempt`] is exactly the "asked and answered" state the backoff already
    /// ends in, and it inherits the same release: `tick`'s `retain` drops it
    /// when the ports leave the list, so unplug-and-replug means "I want it
    /// back" — one gesture, both meanings.
    pub fn decline(&mut self, input: &PortRef, output: &PortRef) {
        self.tried.insert(
            (input.id.clone(), output.id.clone()),
            // `retry_at: None` with the failure count spent: never due again.
            Attempt { failures: RETRY_BACKOFF.len() + 1, retry_at: None },
        );
    }

    /// Whether the scan is on — for `ui::setup`'s empty-desk copy, which says
    /// "watching for boxes" and must not say it while this is off.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The checkbox and the one line about what it last did.
    pub fn ui(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.enabled, "Auto-connect")
                .on_hover_text(
                    "Ask any unclaimed Elektron port who it is, and give it to a box in this \
                     session that has no ports. It never moves a port you picked, and never \
                     touches whether a box takes clock.",
                );
            if self.pending.is_some() {
                ui.spinner();
            }
        });
        if let Some(said) = &self.said {
            ui.weak(said);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use digi_core::device::{Device, DeviceIo, A4, DN2, DT2};

    fn info(id: &str, name: &str) -> PortInfo {
        PortInfo {
            id: id.into(),
            name: name.into(),
            slug: digi_protocol::device::slug_from_port_name(name),
        }
    }

    fn port(id: &str, name: &str) -> PortRef {
        PortRef { id: id.into(), name: name.into() }
    }

    fn desk() -> Session {
        let mut session = Session::default();
        session.add_device(Device::new("DT2", &DT2, 16));
        session.add_device(Device::new("DN2", &DN2, 16));
        session
    }

    fn dt2_ports() -> (Vec<PortInfo>, Vec<PortInfo>) {
        (
            vec![info("in-1", "Digitakt II"), info("in-2", "IAC Driver Bus 1")],
            vec![info("out-1", "Digitakt II"), info("out-2", "IAC Driver Bus 1")],
        )
    }

    // The hazard this whole backoff exists for: a box whose ports enumerate
    // before its SysEx handler is up fails once and must still get another turn.
    // Before `Attempt`, the first failure was permanent and only unplugging the
    // cable cleared it.
    #[test]
    fn a_pair_that_did_not_answer_is_asked_again_rather_than_written_off() {
        let t0 = Instant::now();
        let mut a = Attempt::started();
        a.failed(t0);

        assert!(!a.due(t0), "not instantly — that would be a busy loop over a dead port");
        assert!(!a.due(t0 + Duration::from_secs(4)), "and not before its backoff matures");
        assert!(a.due(t0 + RETRY_BACKOFF[0]), "but it does come back around");
    }

    // Each failure waits longer than the last, so a booting box is caught early
    // while a permanently silent port is asked at a decreasing rate rather than
    // every three seconds forever.
    #[test]
    fn each_retry_waits_longer_than_the_one_before() {
        let t0 = Instant::now();
        let mut a = Attempt::started();
        let mut waits = Vec::new();
        for _ in 0..RETRY_BACKOFF.len() {
            a.failed(t0);
            waits.push(a.retry_at.expect("still has patience left") - t0);
        }
        assert_eq!(waits, RETRY_BACKOFF.to_vec());
        assert!(waits.windows(2).all(|w| w[1] > w[0]), "{waits:?} must be increasing");
    }

    // And the other half of the bargain, which is the reason the original code
    // only asked once: an IAC bus with nothing behind it must not be interrogated
    // for the life of the session.
    #[test]
    fn the_patience_runs_out_so_a_silent_port_is_not_asked_forever() {
        let t0 = Instant::now();
        let mut a = Attempt::started();
        for _ in 0..=RETRY_BACKOFF.len() {
            a.failed(t0);
        }

        assert_eq!(a.failures, RETRY_BACKOFF.len() + 1, "one attempt more than there are waits");
        assert_eq!(a.retry_at, None, "spent");
        // A year later it is still spent — only the port disappearing clears it,
        // which is `tried.retain` in `tick` and is what makes a replug work.
        assert!(!a.due(t0 + Duration::from_secs(60 * 60 * 24 * 365)));
    }

    #[test]
    fn an_unclaimed_elektron_port_is_a_candidate_and_an_iac_bus_is_not() {
        // Rule 2: an unnamed port might be anything, including another
        // sequencer's clock, and this never grabs one.
        let (inputs, outputs) = dt2_ports();
        let found = candidates(&desk(), &inputs, &outputs);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slug, "digitakt2");
        assert_eq!(found[0].input.id, "in-1");
        assert_eq!(found[0].output.id, "out-1");
    }

    #[test]
    fn a_port_a_user_already_picked_is_never_a_candidate() {
        // **Rule 1.** The DN2 row has been aimed at the Digitakt's socket by
        // hand — a strange thing to do, and the user's to do. Auto-connect must
        // not take it back off them for the DT2 row.
        let mut session = desk();
        let dn2 = session.devices[1].id;
        session.device_mut(dn2).unwrap().io = DeviceIo {
            input: Some(port("in-1", "Digitakt II")),
            output: Some(port("out-1", "Digitakt II")),
            ..DeviceIo::default()
        };
        let (inputs, outputs) = dt2_ports();
        assert!(candidates(&session, &inputs, &outputs).is_empty());
    }

    #[test]
    fn a_port_matched_only_by_name_still_counts_as_taken() {
        // Ids are not stable across a replug, which is why `PortRef::same_port`
        // falls back to the name — and a rule about not stealing ports has to use
        // the same comparison, or a renumbered port reads as free.
        //
        // **Two DT2s, and the second one is what gives this teeth.** With one,
        // an earlier version of this test passed even with the name comparison
        // deleted: the single DT2 had ports, so there was nowhere for a
        // candidate to go and the "no room" rule refused it instead. The plant
        // that should have broken it could not. Lesson 4 — an assertion that
        // passes before the code under test runs — so the free box is here to
        // make sure only the taken-port rule can be doing the refusing.
        let mut session = Session::default();
        session.add_device(Device::new("DT2 cabled", &DT2, 16));
        session.add_device(Device::new("DT2 spare", &DT2, 16));
        let cabled = session.devices[0].id;
        session.device_mut(cabled).unwrap().io = DeviceIo {
            input: Some(port("stale-id", "Digitakt II")),
            output: Some(port("stale-id-out", "Digitakt II")),
            ..DeviceIo::default()
        };
        assert!(
            session.devices[1].io.input.is_none(),
            "the spare has nowhere to be but this socket, so only the name match can refuse"
        );
        let (inputs, outputs) = dt2_ports();
        assert!(candidates(&session, &inputs, &outputs).is_empty());
    }

    #[test]
    fn a_box_the_session_has_never_seen_is_still_asked() {
        // No DT2 in the session — which until 2026-08-24 meant "not a
        // candidate", because auto-connect could not add boxes and the session
        // always opened with both rows anyway. The app opens empty now, so
        // this pair is exactly the one discovery exists for: ask it, and a
        // reply that agrees becomes a new row ([`Placement::Add`]).
        let mut session = Session::default();
        session.add_device(Device::new("DN2", &DN2, 16));
        let (inputs, outputs) = dt2_ports();
        let found = candidates(&session, &inputs, &outputs);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].slug, "digitakt2");
    }

    #[test]
    fn an_empty_session_finds_every_box_on_the_desk() {
        // First launch: no boxes, three Elektrons plugged in. All three are
        // candidates — the A4 included, whose recognition comes off the
        // name-only table since no A4 has ever answered a handshake — and the
        // IAC bus still is not: rule 2 does not relax just because the desk
        // is empty.
        let session = Session::default();
        let inputs = vec![
            info("in-1", "Digitakt II"),
            info("in-2", "Digitone II"),
            info("in-3", "Elektron Analog Four"),
            info("in-4", "IAC Driver Bus 1"),
        ];
        let outputs = vec![
            info("out-1", "Digitakt II"),
            info("out-2", "Digitone II"),
            info("out-3", "Elektron Analog Four"),
            info("out-4", "IAC Driver Bus 1"),
        ];
        let found = candidates(&session, &inputs, &outputs);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].slug, "digitakt2");
        assert_eq!(found[1].slug, "digitone2");
        assert_eq!(found[2].slug, "analogfour");
    }

    #[test]
    fn a_box_that_cannot_answer_is_adopted_on_its_port_name_alone() {
        // The name-only path end to end: empty desk, a pair named for a box the
        // model table knows. No handshake happens — the row is added and wired
        // right here, six tracks, both ends bound, and *no* OS report, because
        // nothing answered and claiming one would be a lie.
        //
        // It drives the A4 because `adopt_by_name` resolves a *shipped* slug
        // and the A4 is the six-track row that makes the assertions legible.
        // Since 2026-08-28 no desk reaches this path — the A4 answers — so the
        // test calls the function directly and asserts what it would do for the
        // model that one day sets `answers_identity: false`.
        let mut session = Session::default();
        let mut ac = AutoConnect::default();
        let candidate = Candidate {
            slug: "analogfour",
            input: port("in-1", "Elektron Analog Four"),
            output: port("out-1", "Elektron Analog Four"),
        };
        let changed = ac.adopt_by_name(&mut session, candidate);

        assert!(changed);
        assert_eq!(session.devices.len(), 1);
        let a4 = &session.devices[0];
        assert_eq!(a4.name, "A4");
        assert_eq!(a4.model, &A4);
        assert_eq!(a4.model.num_tracks, 6, "four voices, FX and CV");
        assert_eq!(a4.io.input, Some(port("in-1", "Elektron Analog Four")));
        assert_eq!(a4.io.output, Some(port("out-1", "Elektron Analog Four")));
        assert_eq!(a4.io.build, None, "nothing answered, so no OS is claimed");
        assert!(!a4.can_sysex(), "and it is live-only");
        assert_eq!(
            ac.said.as_deref(),
            Some("Added A4 — Analog Four by its port name"),
            "the copy says which kind of claim this was"
        );
    }

    #[test]
    fn a_free_row_is_reused_before_a_second_one_is_added() {
        // Same rule as `destination` on the handshake path: a row someone added
        // by hand (a session sketched before the box arrived) is exactly where
        // its box should land, not beside a duplicate. Reached by calling in,
        // as above.
        let mut session = Session::default();
        session.add_device(Device::new("A4", &A4, 16));
        let planned = session.devices[0].id;
        let mut ac = AutoConnect::default();
        let candidate = Candidate {
            slug: "analogfour",
            input: port("in-1", "Elektron Analog Four"),
            output: port("out-1", "Elektron Analog Four"),
        };
        assert!(ac.adopt_by_name(&mut session, candidate));

        assert_eq!(session.devices.len(), 1, "no second row");
        assert_eq!(session.devices[0].id, planned);
        assert!(session.devices[0].io.input.is_some());
        assert_eq!(
            ac.said.as_deref(),
            Some("Connected A4 — Analog Four by its port name")
        );
    }

    #[test]
    fn an_input_with_no_matching_output_is_not_half_connected() {
        // A write's verify read comes back on the input and the dump goes out on
        // the output; a box with one end is a box that can send and never know.
        let (inputs, _) = dt2_ports();
        assert!(candidates(&desk(), &inputs, &[]).is_empty());
    }

    #[test]
    fn two_identical_boxes_get_a_pair_each_rather_than_the_same_output_twice() {
        let mut session = Session::default();
        session.add_device(Device::new("DT2 A", &DT2, 16));
        session.add_device(Device::new("DT2 B", &DT2, 16));
        let inputs = vec![info("in-1", "Digitakt II"), info("in-2", "Digitakt II")];
        let outputs = vec![info("out-1", "Digitakt II"), info("out-2", "Digitakt II")];
        let found = candidates(&session, &inputs, &outputs);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].output.id, "out-1");
        assert_eq!(found[1].output.id, "out-2");
    }

    fn identity(product_id: u8) -> DeviceIdentity {
        digi_protocol::device::identity_from_responses(
            &digi_protocol::device::DeviceResponse {
                product_id,
                supported_ids: vec![0x60],
                reported_name: String::new(),
            },
            "0070".into(),
            "1.15B".into(),
        )
    }

    #[test]
    fn a_reply_only_lands_on_a_box_that_has_no_ports() {
        // The difference from `Session::bind_identity`, which would happily
        // re-point the only DT2 in the session because it is the only one. Here
        // that is a box someone aimed, and it is left where they put it.
        let mut session = desk();
        let dt2 = session.devices[0].id;
        assert_eq!(destination(&session, &identity(42)), Some(dt2));

        session.device_mut(dt2).unwrap().io = DeviceIo {
            input: Some(port("iac-in", "IAC Driver Bus 1")),
            output: Some(port("iac-out", "IAC Driver Bus 1")),
            ..DeviceIo::default()
        };
        assert_eq!(
            destination(&session, &identity(42)),
            None,
            "the only DT2 already has ports, so a reply has nowhere it may go"
        );
    }

    #[test]
    fn a_reply_for_a_model_this_session_does_not_have_lands_nowhere() {
        let mut session = Session::default();
        session.add_device(Device::new("DN2", &DN2, 16));
        assert_eq!(destination(&session, &identity(42)), None);
    }

    #[test]
    fn a_box_that_is_not_what_its_port_is_named_for_is_left_for_a_human() {
        // **Rule 2's second half.** The port says "Digitakt II" and a Digitone
        // answered on it: something about this desk is not what it looks like,
        // and quietly binding it is how the app ends up clocking a box nobody
        // meant it to.
        let session = desk();
        let Placement::Disagrees(why) = placement(&session, "digitakt2", &identity(43)) else {
            panic!("a DN2 on a port named for a DT2 must not be placed")
        };
        assert!(why.contains("Digitone II"), "{why}");
        assert!(why.contains("use Identify"), "the way out is named: {why}");
    }

    #[test]
    fn a_reply_that_agrees_with_its_port_lands_on_the_free_box() {
        let session = desk();
        let dt2 = session.devices[0].id;
        assert_eq!(placement(&session, "digitakt2", &identity(42)), Placement::Bind(dt2));
    }

    #[test]
    fn a_reply_with_no_free_row_grows_the_desk_instead_of_declining() {
        // Every DT2 in the session already has ports — so whatever just
        // answered is a *second* physical DT2, and a second box deserves a
        // second row. This arm was `NoRoom` (a silent decline) until
        // discovery-first, 2026-08-24.
        let mut session = desk();
        let dt2 = session.devices[0].id;
        session.device_mut(dt2).unwrap().io = DeviceIo {
            input: Some(port("in-1", "Digitakt II")),
            output: Some(port("out-1", "Digitakt II")),
            ..DeviceIo::default()
        };
        assert_eq!(placement(&session, "digitakt2", &identity(42)), Placement::Add(&DT2));
    }

    #[test]
    fn a_reply_on_an_empty_desk_lands_as_a_new_row_with_its_ports_and_os() {
        // The whole first-launch story in one call: empty session, a DT2
        // answers, and `land` adds the row, binds both ends, and keeps the
        // handshake's OS report. `Session::add_device` gives every scene a slot
        // for it, so the new box is playable the same frame.
        let mut session = Session::default();
        let mut ac = AutoConnect::default();
        let candidate = Candidate {
            slug: "digitakt2",
            input: port("in-1", "Digitakt II"),
            output: port("out-1", "Digitakt II"),
        };
        let changed = ac.land(&mut session, Reply { candidate, identity: Ok(identity(42)) });

        assert!(changed, "a grown desk is a session change the engine has to hear about");
        assert_eq!(session.devices.len(), 1);
        let added = &session.devices[0];
        assert_eq!(added.name, "DT2", "the model key while it is free");
        assert_eq!(added.model, &DT2);
        assert_eq!(added.io.input, Some(port("in-1", "Digitakt II")));
        assert_eq!(added.io.output, Some(port("out-1", "Digitakt II")));
        assert_eq!(added.io.build.as_deref(), Some("0070"));
        assert!(added.io.takes_clock, "a new box follows the session's clock");
        assert!(
            session.scenes.iter().all(|s| s.slots.contains_key(&added.id)),
            "every scene can already point at it"
        );
        assert_eq!(ac.said.as_deref(), Some("Added DT2 — Digitakt II"));
    }

    #[test]
    fn a_second_box_of_the_same_model_gets_its_own_name() {
        // Two physical DT2s: the second row must not also be called "DT2", or
        // the scene picker and the port pickers stop telling them apart.
        let mut session = desk();
        let dt2 = session.devices[0].id;
        session.device_mut(dt2).unwrap().io = DeviceIo {
            input: Some(port("in-9", "Elektron Digitakt II")),
            output: Some(port("out-9", "Elektron Digitakt II")),
            ..DeviceIo::default()
        };
        let mut ac = AutoConnect::default();
        let candidate = Candidate {
            slug: "digitakt2",
            input: port("in-1", "Digitakt II"),
            output: port("out-1", "Digitakt II"),
        };
        let changed = ac.land(&mut session, Reply { candidate, identity: Ok(identity(42)) });

        assert!(changed);
        let names: Vec<&str> = session.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["DT2", "DN2", "DT2 2"]);
    }

    #[test]
    fn a_declined_pair_is_never_due_again_until_it_leaves_the_list() {
        // The remove control's half of the bargain: a box removed with its
        // cable still in must not boomerang back on the next scan. A declined
        // pair carries the same spent state a run-out backoff does, and the
        // same release — `tick`'s retain drops it when the ports disappear.
        let mut ac = AutoConnect::default();
        ac.decline(&port("in-1", "Digitakt II"), &port("out-1", "Digitakt II"));

        let attempt = ac
            .tried
            .get(&("in-1".to_string(), "out-1".to_string()))
            .expect("declining parks an attempt on the pair");
        assert_eq!(attempt.retry_at, None, "spent, not waiting");
        assert!(!attempt.due(Instant::now() + Duration::from_secs(60 * 60)));
    }
}
