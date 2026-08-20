//! Measure how far the engine thread misses its deadlines, per port.
//!
//! PLAN.md §4 targets ~1 ms jitter, and §8 flags the two-port case as *worse*
//! than twice as hard: clock and note events go to two USB endpoints from one
//! thread, and a slow `send()` on one port delays the other. It also says this
//! must be **measured with both boxes connected, not extrapolated from one** —
//! which is what this example is for.
//!
//! ```text
//! # No hardware: measures the thread itself against a recording sink.
//! cargo run -p digi_engine --example jitter --release
//!
//! # One box.
//! cargo run -p digi_engine --example jitter --release -- "Elektron Digitakt II"
//!
//! # The case that matters — both boxes, two ports, one queue.
//! cargo run -p digi_engine --example jitter --release -- "Elektron Digitakt II" "Elektron Digitone II"
//! ```
//!
//! **Run it `--release`.** A debug build measures the debug build, not the
//! engine.
//!
//! This sends MIDI clock and notes to whatever ports it is given, so it is *not*
//! read-only the way `identify_into_session` is: a box listening to external
//! clock will start playing. It writes no SysEx and touches no pattern data — it
//! holds nothing that could reach the store path `digi_midi` gained on 2026-08-18,
//! since `engine` does not depend on `digi_protocol` at all (PLAN.md §3) — but it
//! is a "make sure you meant to" rather than a "safe to run at any time".

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use digi_core::device::{Device, DeviceIo, PortRef, DN2, DT2};
use digi_core::model::Note;
use digi_core::session::Session;
use digi_engine::event::PortTable;
use digi_engine::scheduler::Scheduler;
use digi_engine::sink::MidirSink;
use digi_engine::transport::{
    PortSink, RecordingSink, Transport, TransportCommand, TransportState,
};

const BPM: f64 = 120.0;
const RUN_FOR: Duration = Duration::from_secs(10);

fn main() {
    let names: Vec<String> = std::env::args().skip(1).collect();

    // A busy session: sixteenths on four tracks per box, at three different
    // lengths so the tracks are genuinely polymetric and the queue is never
    // trivially ordered.
    let mut session = Session { tempo_bpm: BPM, ..Session::default() };
    let models = [&DT2, &DN2];
    for (i, name) in names.iter().enumerate() {
        let model = models[i.min(models.len() - 1)];
        let mut device = Device::new(model.display, model, 16);
        device.io = DeviceIo {
            output: Some(PortRef { id: name.clone(), name: name.clone() }),
            takes_clock: true,
            ..device.io
        };
        let id = session.add_device(device);
        fill(&mut session, id);
    }
    if names.is_empty() {
        // No ports named: measure the thread against a recording sink, on a
        // fabricated port so there is still something to schedule.
        let mut device = Device::new("DT2", &DT2, 16);
        device.io = DeviceIo {
            output: Some(PortRef { id: "none".into(), name: "no hardware".into() }),
            takes_clock: true,
            ..device.io
        };
        let id = session.add_device(device);
        fill(&mut session, id);
    }

    let mut ports = PortTable::new();
    let mut scheduler = Scheduler::new(BPM);
    scheduler.prepare(&session, &mut ports);

    let sink: Box<dyn PortSink> = if names.is_empty() {
        println!("No port names given — measuring the engine thread against a recording sink.");
        println!("Pass port names to measure real output; `cargo run -p digi_midi --example list_ports` lists them.\n");
        Box::new(RecordingSink::default())
    } else {
        let (sink, failed) = MidirSink::open(&ports);
        for (id, e) in &failed {
            eprintln!("could not open {:?}: {e}", ports.name(*id).unwrap_or("?"));
        }
        if sink.open_count() == 0 {
            eprintln!("no ports opened — nothing to measure");
            std::process::exit(1);
        }
        Box::new(sink)
    };

    let state = Arc::new(TransportState::with_ports(ports.len()));
    let transport = Transport::spawn(
        Arc::new(session),
        scheduler,
        sink,
        Arc::clone(&state),
        // This example measures send timing, not parameter changes, and it must
        // stay the safety class it advertises: clock and notes, nothing that
        // moves a knob on the box.
        Box::new(digi_engine::NoPLocks),
    );

    println!("Playing for {} s at {BPM} bpm on {} port(s)…", RUN_FOR.as_secs(), ports.len());
    transport.send(TransportCommand::Start);
    std::thread::sleep(RUN_FOR);
    transport.send(TransportCommand::Stop);
    std::thread::sleep(Duration::from_millis(100));

    println!("\n{:<28} {:>8} {:>10} {:>10} {:>10}", "port", "sends", "mean", "worst", ">1 ms");
    println!("{}", "-".repeat(70));
    for id in ports.ids() {
        let s = &state.jitter[id.0];
        let sends = s.sends.load(Ordering::Relaxed);
        if sends == 0 {
            continue;
        }
        println!(
            "{:<28} {:>8} {:>9.0}µs {:>9}µs {:>10}",
            ports.name(id).unwrap_or("?"),
            sends,
            s.mean_late_us(),
            s.max_late_us.load(Ordering::Relaxed),
            s.over_1ms.load(Ordering::Relaxed),
        );
    }
    println!(
        "\nPLAN.md §4 targets ~1 ms. A worst case well past that on *one* port while the\n\
         other is clean is the signal for the fallback §8 names: a sender thread per port,\n\
         fed from the shared queue. On macOS the other option is the `coremidi` crate,\n\
         whose scheduled packet timestamps move this back into the driver."
    );
}

/// Four tracks of sixteenths at three lengths, so the queue has real work in it.
fn fill(session: &mut Session, device: digi_core::device::DeviceId) {
    let slot = session
        .slot_in_scene(session.current_scene, device)
        .expect("the scene names a slot for every device")
        .slot();
    let pattern = session
        .device_mut(device)
        .expect("just added")
        .pattern_mut(slot)
        .expect("slot exists");
    for (t, length) in [(0usize, 16u16), (1, 12), (2, 15), (3, 16)] {
        let track = pattern.track_mut(t).expect("16-track model");
        track.length_steps = length;
        track.notes = (0..length)
            .map(|s| Note::new(s as f64, 36 + (s % 12) as u8, 0.5, 100, 0.0))
            .collect();
    }
}
