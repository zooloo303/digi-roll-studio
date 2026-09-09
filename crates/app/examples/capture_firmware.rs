//! Read-only firmware compatibility capture. Saves original, checksum-checked
//! replies before decoding, including replies whose struct version is unknown.
//! Usage: cargo run -p digi_roll_studio --example capture_firmware -- OUTPUT_DIR
//! Captures A01 and A16, plus A4 kit 0 and one +Drive preset per device.
use std::path::Path;

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};
use digi_protocol::{a4_kit, a4_pattern};
use digi_protocol::pattern::{decode_pattern_kit, dn2_spec, dt2_spec};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args().nth(1).ok_or("provide an output directory")?;
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    let inputs = list_inputs()?;
    let outputs = list_outputs()?;
    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else { continue };
        let mut device = ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output))?;
        let id = device.identify()?;
        if !matches!(id.slug.as_str(), "digitakt2" | "digitone2" | "analogfour") { continue; }
        let prefix = format!("{}-{}-{}", id.slug, id.version, id.build);
        println!("{prefix}");
        std::fs::write(out.join(format!("{prefix}-identity.txt")), format!("{id:#?}\n"))?;
        let family = id.family.ok_or("unknown dump family")?;
        let a4 = id.slug == "analogfour";
        for index in [0, 15] {
            let dump = device.fetch_dump(family, if a4 { 0x64 } else { 0x60 }, index)?;
            std::fs::write(out.join(format!("{prefix}-A{:02}.syx", index + 1)), &dump.raw)?;
            println!("  A{:02}: {} bytes, header {:02x?}", index + 1, dump.payload.len(), &dump.payload[..dump.payload.len().min(16)]);
            if a4 {
                println!("  decode: {:?}", a4_pattern::parse_pattern(&dump.raw).map(|p| (p.slot, p.payload.len())));
            } else {
                let spec = if id.slug == "digitakt2" { dt2_spec() } else { dn2_spec() };
                println!("  decode: {:?}", decode_pattern_kit(&spec, &dump.payload).map(|p| (p.name, p.version, p.kit.name)));
            }
        }
        if a4 {
            let dump = device.fetch_dump(family, 0x62, 0)?;
            std::fs::write(out.join(format!("{prefix}-kit-0.syx")), &dump.raw)?;
            println!("  kit: {:?}", a4_kit::parse_kit(&dump.raw).map(|k| k.name));
        }
        match device.drive_read_file("/soundbanks/A/1") {
            Ok(bytes) => {
                std::fs::write(out.join(format!("{prefix}-sound-A-1.bin")), &bytes)?;
                println!("  preset: {} bytes", bytes.len());
            }
            Err(e) => println!("  preset read: {e}"),
        }
    }
    Ok(())
}
