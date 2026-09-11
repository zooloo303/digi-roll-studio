//! What each box in the table is allowed to do.
//!
//! These are safety assertions, not descriptions. "This box's format is mapped"
//! and "this box may be written to" are different claims, the type system
//! carries the difference, and this file is what stops it being lost to a
//! tidy-up.
//!
//! **The Syntakt moved across that line on 2026-09-11**, and it is worth saying
//! how, because the whole point of these assertions is that crossing it costs
//! somebody an edit here: six sends into six empty slots on OS 1.40, each
//! carrying a different edit, each read back and byte-compared, with A02 read
//! before the first and after the last and identical as the control.
//! `dumps/syntakt-2026-09-11/README.md` is the run.

use digi_core::device::{PatternRoute, DN2, DT2, MODELS, SYNTAKT, A4};

#[test]
fn the_syntakt_fetches_and_sends_on_a_route_of_its_own() {
    assert!(SYNTAKT.can_fetch_patterns(), "its layout is mapped and round-trips");
    assert!(SYNTAKT.can_send_patterns(), "verified on hardware 2026-09-11");
    // **Not `Request`.** That route means a gen-2 `Spec` decodes the answer, and
    // this box has none; its flow is `safe_write::syntakt_safe_write_tracks` and
    // the panels dispatch on exactly this variant to find it.
    assert_eq!(SYNTAKT.pattern_route(), PatternRoute::RequestSyntakt);
    assert!(SYNTAKT.spec().is_none(), "there is no gen-2 Spec for this box");
}

/// Nothing is on the read-only route now, and that is the route working.
///
/// The Syntakt sat here from 2026-09-10 until its write was verified. The
/// variant stays for the next box that can be read before it can be written,
/// which is the state every mapped box passes through.
#[test]
fn the_read_only_route_is_empty_and_still_earns_its_keep() {
    assert!(
        MODELS.iter().all(|m| m.pattern_route() != PatternRoute::RequestReadOnly),
        "a box on this route needs its own line in this file saying why"
    );
}

/// A read-only box must not carry a gen-2 `Spec`, because half the app treats
/// having one as permission to write.
#[test]
fn a_fetch_only_box_carries_no_gen2_spec() {
    for model in MODELS.iter().filter(|m| !m.can_send_patterns()) {
        assert!(
            model.spec().is_none(),
            "{} is fetch-only but carries a Spec, which the write paths read as consent",
            model.display
        );
    }
}

/// The boxes that *can* be written to are exactly the ones verified on
/// hardware. A new name appearing here should be a deliberate act with a
/// hardware session behind it, not a diff nobody read.
///
/// Every entry also has to be in `safe_write::WRITE_ALLOWED_BUILDS`, which has
/// a tripwire of its own for the same reason — this list says *which boxes*, and
/// that one says *which firmware*, and a box that passes one and fails the other
/// is a button that turns out not to work.
#[test]
fn only_the_verified_boxes_can_be_written_to() {
    let writable: Vec<&str> =
        MODELS.iter().filter(|m| m.can_send_patterns()).map(|m| m.display).collect();
    assert_eq!(writable, vec![DT2.display, DN2.display, A4.display, SYNTAKT.display]);

    for model in MODELS.iter().filter(|m| m.can_send_patterns()) {
        let slug = model.slug.expect("a writable box is one the handshake can name");
        assert!(
            digi_protocol::safe_write::WRITE_ALLOWED_BUILDS.iter().any(|(s, _)| *s == slug),
            "{} may be written to but no firmware build is allowed for it",
            model.display
        );
    }
}

#[test]
fn every_model_says_which_of_the_two_it_is() {
    for model in MODELS {
        // Nothing may claim to send without also being able to fetch: a write
        // path with no read is a write nobody can verify.
        if model.can_send_patterns() {
            assert!(model.can_fetch_patterns(), "{} sends but cannot fetch", model.display);
        }
    }
}

/// The label is what a person reads when they wonder why a box is not in the
/// send picker, so it has to say something different for the two cases.
#[test]
fn the_route_labels_tell_fetch_only_apart_from_the_rest() {
    assert_eq!(PatternRoute::RequestReadOnly.label(), "fetch only");
    assert_ne!(PatternRoute::RequestReadOnly.label(), PatternRoute::Request.label());
    assert_ne!(PatternRoute::RequestReadOnly.label(), PatternRoute::LiveOnly.label());
}
