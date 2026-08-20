// Port enumeration against the real machine, and an optional identity
// handshake. Read-only throughout — it sends nothing unless you name a port.
//
//   cargo run -p digi_midi --example list_ports
//   cargo run -p digi_midi --example list_ports -- "Digitakt II"

use digi_midi::{list_inputs, list_outputs, ports::find_port_pair, ElektronDevice, PortBinding};

fn main() {
    let inputs = list_inputs().expect("enumerate inputs");
    let outputs = list_outputs().expect("enumerate outputs");

    println!("inputs ({}):", inputs.len());
    for p in &inputs {
        println!("  {:<40} slug={:?}  id={}", p.name, p.slug, p.id);
    }
    println!("outputs ({}):", outputs.len());
    for p in &outputs {
        println!("  {:<40} slug={:?}  id={}", p.name, p.slug, p.id);
    }

    let Some(fragment) = std::env::args().nth(1) else {
        println!("\npass a port-name fragment to run an identity handshake against it");
        return;
    };

    let pair = find_port_pair(&fragment).expect("enumerate ports");
    let Some((input, output)) = pair else {
        println!("\nno input+output pair matching {fragment:?}");
        return;
    };
    println!("\nidentifying {:?} ...", input.name);

    let mut device = ElektronDevice::open(&PortBinding::from(&input), &PortBinding::from(&output))
        .expect("open ports");
    match device.identify() {
        Ok(id) => {
            println!("  name    {}", id.name);
            println!("  slug    {}", id.slug);
            println!("  build   {}", id.build);
            println!("  version {}", id.version);
            println!("  family  {:?}", id.family);
            println!("  dumps   {}", if id.supported() { "supported" } else { "unknown protocol" });
            println!("  box supports request opcodes {:02x?}", id.supported_ids);
        }
        Err(e) => println!("  handshake failed: {e}"),
    }
}
