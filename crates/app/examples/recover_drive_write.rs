// Diagnose and clear a box left mid-write by an abandoned `0x57` WriteOpen.
//
// # Why this exists
//
// `probe_drive_write.rs` opened a write transfer, had its `0x58` refused, and
// exited without a `0x59`. The A4 then stopped answering `0x01` Device — the
// identity handshake every other tool in this workspace starts with — so the
// box was not "off", it was **busy holding a write transfer open** and
// declining unrelated API traffic while it waited for the rest of a file that
// was never coming.
//
// That is worth a tool rather than a power cycle, because the same thing will
// happen to anyone who interrupts a write, and "the box went deaf" is not a
// symptom that suggests its own cause.
//
// # What it does, in order
//
//   1. `0x53` List — does the *file* API answer at all? This separates "the box
//      is wedged" from "the box is off or the cable is out".
//   2. `0x01` Device — does the identity handshake answer? If List works and
//      this does not, the transfer is the thing holding it.
//   3. `0x59` WriteClose against each candidate fd, which is how a transfer is
//      ended from this side.
//
// **Step 3 can commit.** A WriteClose on a handle whose declared length was
// never delivered may land a short or empty file in the slot that WriteOpen
// named. Only run it against a slot you are willing to have written — which,
// for the run this was built for, is an empty `/soundbanks/P/1`.
//
// Run with:
//   cargo run -p digi_roll_studio --example recover_drive_write -- --port "Analog Four"
//   cargo run -p digi_roll_studio --example recover_drive_write -- --port "Analog Four" --close

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::drive::API_FILE_WRITE_CLOSE;

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let port = argv
        .windows(2)
        .find(|w| w[0] == "--port")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "Analog Four".to_string());
    let close = argv.iter().any(|a| a == "--close");

    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");
    let input = inputs.iter().find(|p| p.name.contains(port.as_str())).expect("no such input port");
    let output = outputs.iter().find(|p| p.name == input.name).expect("no matching output port");
    let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))
        .expect("could not open the port");
    println!("=== {} ===", input.name);

    match device.drive_list("/", 0, 0) {
        Ok(reply) => println!("  0x53 List /   : answers, {} entries", reply.count),
        Err(e) => println!("  0x53 List /   : {e}"),
    }
    match device.identify() {
        Ok(id) => println!("  0x01 Device   : answers, {} build {}", id.name, id.build),
        Err(e) => println!("  0x01 Device   : {e}"),
    }

    if !close {
        println!("\n  pass --close to send 0x59 WriteClose against candidate handles");
        return;
    }

    println!("\n  0x59 WriteClose against candidate fds (this can commit a short file):");
    for fd in 1u32..=8 {
        let mut body = fd.to_be_bytes().to_vec();
        body.extend_from_slice(&0u32.to_be_bytes());
        match device.drive_write_request(API_FILE_WRITE_CLOSE, &body) {
            Ok(args) if args.first() == Some(&0x01) => println!("    fd {fd}: closed"),
            Ok(args) => {
                let text: String =
                    args[1..].iter().copied().take_while(|&b| b != 0).map(|b| b as char).collect();
                println!("    fd {fd}: refused — {text:?}");
            }
            Err(e) => println!("    fd {fd}: {e}"),
        }
    }

    println!("\n  after:");
    match device.identify() {
        Ok(id) => println!("    0x01 Device : answers, {} build {}", id.name, id.build),
        Err(e) => println!("    0x01 Device : {e}"),
    }
}
