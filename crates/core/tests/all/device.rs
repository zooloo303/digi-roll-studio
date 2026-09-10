//! What each box in the table is allowed to do.
//!
//! These are safety assertions, not descriptions. The Syntakt is mapped well
//! enough to read and **has never been written to**, and the difference between
//! those two is a thing the type system now carries; this file is what stops it
//! being lost to a tidy-up.

use digi_core::device::{PatternRoute, DN2, DT2, MODELS, SYNTAKT, A4};

#[test]
fn the_syntakt_can_be_fetched_from_and_not_sent_to() {
    assert!(SYNTAKT.can_fetch_patterns(), "its layout is mapped and round-trips");
    assert!(
        !SYNTAKT.can_send_patterns(),
        "no write to a Syntakt has been verified on hardware, and none is authorised"
    );
    assert_eq!(SYNTAKT.pattern_route(), PatternRoute::RequestReadOnly);
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

/// The boxes that *can* be written to are exactly the three that have been
/// verified on hardware. A fourth appearing here should be a deliberate act
/// with a hardware session behind it, not a diff nobody read.
#[test]
fn only_the_verified_boxes_can_be_written_to() {
    let writable: Vec<&str> =
        MODELS.iter().filter(|m| m.can_send_patterns()).map(|m| m.display).collect();
    assert_eq!(writable, vec![DT2.display, DN2.display, A4.display]);
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
