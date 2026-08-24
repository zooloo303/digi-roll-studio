// Phase 3's exit criteria, end to end and without the GUI: enumerate the ports,
// identify every box that looks like an Elektron, and bind each reply to the
// device in the session it belongs to.
//
// **Read-only.** It sends two API requests per box — "who are you" and "what
// OS" — and nothing else. It constructs no dump opcode of any kind, so it cannot
// reach the store path `digi_midi` gained on 2026-08-18 (that path is
// `safe_write_track`'s, and this example does not call it).
//
// Run with:  cargo run -p digi_roll_studio --example identify_into_session

use digi_core::device::PortRef;
use digi_core::{two_box_session, BindError};
use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};

fn main() {
    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let mut session = two_box_session();

    println!("session: {} devices, none identified yet", session.devices.len());

    // Elektron boxes expose an input and an output under the same name, so the
    // port list can pair them. The name is only a guess at which box it is; the
    // handshake is what actually answers, and the binding keys off that.
    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
            println!("\n{}: no matching output port", input.name);
            continue;
        };
        println!("\n{} — identifying …", input.name);

        let mut device = match ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output)) {
            Ok(d) => d,
            Err(e) => {
                println!("  could not open: {e}");
                continue;
            }
        };
        let identity = match device.identify() {
            Ok(id) => id,
            Err(e) => {
                println!("  no identity: {e}");
                continue;
            }
        };
        println!(
            "  {} — slug {}, build {}, version {}, dumps {}",
            identity.name,
            identity.slug,
            identity.build,
            identity.version,
            if identity.supported() { "supported" } else { "unknown" },
        );

        let in_ref = PortRef { id: input.id.clone(), name: input.name.clone() };
        let out_ref = PortRef { id: output.id.clone(), name: output.name.clone() };
        match session.bind_identity(&identity, in_ref, out_ref) {
            Ok(id) => {
                let d = session.device(id).expect("just bound");
                println!("  bound to {} ({}), {} tracks", d.name, d.model.display, d.model.num_tracks);
            }
            Err(e @ BindError::Ambiguous { .. }) => {
                println!("  not bound: {e} — the UI asks; a headless run does not guess");
            }
            Err(e) => println!("  not bound: {e}"),
        }
    }

    println!("\nsession after identifying:");
    for d in &session.devices {
        let port = d.io.output.as_ref().map(|p| p.name.as_str()).unwrap_or("—");
        let build = d.io.build.as_deref().unwrap_or("—");
        let version = d.io.version.as_deref().unwrap_or("—");
        println!("  {:<4} {:<14} port {:<24} build {:<6} version {}", d.name, d.model.display, port, build, version);
    }
}
