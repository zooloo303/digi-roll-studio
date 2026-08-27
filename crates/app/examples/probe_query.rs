// Probe the 0x09 Query API — a key→value read distinct from both the dump
// path and the DirList tree — for anything preset/sound/bank shaped.
//
// **Read-only, structurally.** `digi_protocol::query` implements only the
// request-build and reply-parse for 0x09; there is no "set" here and the API
// this belongs to has no documented write counterpart to have built one for.
// `ElektronDevice::query` round-trips through the same `request()` helper
// `identify()` and `dir_list()` already use — no new wire mechanism, just a
// new API id.
//
// # Why
//
// The DN2 has no file-system API (`probe_drive`: no 0x10 DirList) and its
// preset library is not confirmed reachable through the dump-request opcodes
// either (`sweep_dump_indices`, `probe_dump_args`). Query is the third and
// last read mechanism this protocol is known to have. The only documented key
// anywhere is `sample_file.interleaved_stereo_support`, sourced from a TODO
// comment in elk-herd — not because it is expected to matter here, but because
// it is the one string known in advance to produce *some* reply, which is what
// proves the request/response encoding round-trips before anything else here
// can be trusted.
//
// After that, a wordlist of plausible keys in the same dotted style, aimed at
// counts and enumeration rather than values — a preset browser's first need is
// "how many, and in what banks", not any one preset's contents.
//
// What a reply means — corrected after the first run; see
// `_NONE_MEANS_UNKNOWN` below for the evidence:
//
//   * tag 0 (none) — **the key is unknown.** This was predicted to mean "the
//     key exists but has nothing to report", and it does not: the empty key, a
//     bare ".", and a nonsense key all answer None too.
//   * tag 1/2/3/4 — a real value, and the only outcome that means anything.
//   * a timeout — not observed for any key. The box answers every key.
//
// Because unknown keys are answered rather than timed out, a key probe is
// cheap: no 15 s misses to pay for. The wordlist here is modest because it is a
// probe of the key space's *shape*, but scaling it to thousands would cost
// minutes rather than days — unlike a dump sweep.
//
// Run with:
//   cargo run -p digi_roll_studio --example probe_query

use digi_midi::{list_inputs, list_outputs, ElektronDevice, PortBinding};

/// The one key documented anywhere (elk-herd's own TODO comment), tried first
/// to confirm the encoding works end to end before anything else is trusted.
const KNOWN_KEY: &str = "sample_file.interleaved_stereo_support";

/// Degenerate inputs: does the box enumerate keys off an empty string or a
/// bare separator, rather than just refusing them.
const DEGENERATE: &[&str] = &["", "."];

/// Plausible keys in the same dotted style as `KNOWN_KEY`, aimed at counts and
/// structure — what a preset browser needs to know before it needs any one
/// preset's contents.
const WORDLIST: &[&str] = &[
    "sound.count",
    "sound_file.count",
    "preset.count",
    "preset.banks",
    "preset.bank_count",
    "drive.preset_count",
    "sound.bank_count",
    "sound.library_count",
    "sound_pool.count",
    "drive.sound_count",
    "sound_file.bank_count",
    "preset_pool.count",
    "library.sound_count",
    "library.count",
    "drive.library_count",
    "bank.count",
    "drive.bank_count",
    "project.sound_count",
    "preset_library.count",
    "drive.preset_library.count",
    "sound_file.interleaved_stereo_support.count",
    "drive.free_space",
    "storage.free",
];

/// What a `None` reply means - established on hardware 2026-08-26, and the
/// opposite of what was assumed going in.
///
/// The guess was that an unknown key would draw no reply at all, so a type-0
/// `None` would prove the key *existed*. Both boxes disprove it: the empty key,
/// a bare `"."`, and a deliberately absurd key
/// (`sound_file.interleaved_stereo_support.count`) all return `None`, exactly
/// like the 23 plausible ones. **`None` is this API's "no such key".**
///
/// So a key that returns `None` tells you nothing, and counting `None` as an
/// answer inflates a total miss into a clean sweep. Same shape of trap as the
/// DT2's DirList returning an empty listing for a nonexistent path.
///
/// The one key that does work is `sample_file.interleaved_stereo_support` ->
/// `Bool(true)`, on both boxes. That is the encoding sanity check, and it
/// passing is what makes every `None` above meaningful rather than a symptom of
/// a broken codec.
///
/// Useful corollary: because unknown keys are *answered* rather than timed out,
/// key probing costs milliseconds instead of the 15 s an API miss would. A
/// wordlist of thousands is cheap here, where a dump sweep of thousands would
/// take days.
const _NONE_MEANS_UNKNOWN: () = ();

fn main() {
    let inputs = list_inputs().expect("MIDI would not start");
    let outputs = list_outputs().expect("MIDI would not start");

    for input in inputs.iter().filter(|p| p.slug.is_some()) {
        let Some(output) = outputs.iter().find(|p| p.name == input.name) else {
            continue;
        };
        let mut device =
            match ElektronDevice::open(&PortBinding::from(input), &PortBinding::from(output)) {
                Ok(d) => d,
                Err(e) => {
                    println!("\n{}: could not open: {e}", input.name);
                    continue;
                }
            };
        let identity = match device.identify() {
            Ok(id) => id,
            Err(e) => {
                println!("\n{}: no identity: {e}", input.name);
                continue;
            }
        };
        println!("\n=== {} — build {}, version {} ===", identity.name, identity.build, identity.version);

        println!("\n  known key (confirms the wire encoding):");
        try_key(&mut device, KNOWN_KEY);

        println!("\n  degenerate keys:");
        for key in DEGENERATE {
            try_key(&mut device, key);
        }

        println!("\n  wordlist ({} keys):", WORDLIST.len());
        let mut hits = 0usize;
        for key in WORDLIST {
            if try_key(&mut device, key) {
                hits += 1;
            }
        }
        // `hits` counts replies, and every key gets one - see _NONE_MEANS_UNKNOWN.
        // Saying "answered" here turned a total miss into a clean sweep.
        println!(
            "\n  {hits}/{} wordlist keys replied; a reply of None means the key is unknown",
            WORDLIST.len()
        );
    }
}

/// Try one key, print the result, and return whether the box replied at all.
///
/// A `true` here is nearly meaningless on its own: every key replies, and a
/// `None` reply means the key is unknown. Read the printed value, not the count.
/// See `_NONE_MEANS_UNKNOWN`.
fn try_key(device: &mut ElektronDevice, key: &str) -> bool {
    match device.query(key) {
        Ok(value) => {
            println!("    {key:<48} -> {value:?}");
            true
        }
        Err(e) => {
            println!("    {key:<48} -> {e}");
            false
        }
    }
}
