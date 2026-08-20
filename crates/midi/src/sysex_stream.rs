// Reassembles complete F0…F7 frames out of whatever the driver hands us.
//
// Two things make this necessary rather than paranoid. A large dump does not
// arrive as one callback on every backend — ALSA in particular delivers a long
// SysEx in chunks — so a frame has to be accumulated across calls. And MIDI
// real-time bytes (0xF8–0xFF: clock, start, stop, active sensing) are allowed
// to interleave *inside* a SysEx frame on the wire; splicing them into the
// payload would corrupt the seven-bit decode and fail the checksum.

/// Feed it bytes, take out whole SysEx messages.
#[derive(Debug, Default)]
pub struct SysExReassembler {
    pending: Vec<u8>,
}

impl SysExReassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push one driver callback's worth of bytes; returns every frame that
    /// completed. Bytes outside a frame are discarded, as in `splitSysExStream`.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for &b in bytes {
            match b {
                0xf0 => {
                    // A new frame with one still open means the previous one was
                    // truncated. Drop it rather than emit a corrupt message.
                    self.pending.clear();
                    self.pending.push(b);
                }
                0xf7 if !self.pending.is_empty() => {
                    self.pending.push(b);
                    out.push(std::mem::take(&mut self.pending));
                }
                // Real-time bytes are legal mid-frame and are never part of it.
                0xf8..=0xff => {}
                _ if !self.pending.is_empty() => self.pending.push(b),
                // Channel/other traffic outside a frame: not ours.
                _ => {}
            }
        }
        out
    }

    /// True when a frame is open — a dump is still arriving.
    pub fn is_mid_frame(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_a_whole_frame_delivered_at_once() {
        let mut r = SysExReassembler::new();
        assert_eq!(r.push(&[0xf0, 0x01, 0x02, 0xf7]), vec![vec![0xf0, 0x01, 0x02, 0xf7]]);
        assert!(!r.is_mid_frame());
    }

    // The ALSA case: one message split across several driver callbacks.
    #[test]
    fn reassembles_a_frame_split_across_callbacks() {
        let mut r = SysExReassembler::new();
        assert!(r.push(&[0xf0, 0x01]).is_empty());
        assert!(r.is_mid_frame());
        assert!(r.push(&[0x02, 0x03]).is_empty());
        assert_eq!(r.push(&[0x04, 0xf7]), vec![vec![0xf0, 0x01, 0x02, 0x03, 0x04, 0xf7]]);
    }

    #[test]
    fn emits_several_frames_from_one_callback() {
        let mut r = SysExReassembler::new();
        let got = r.push(&[0xf0, 0x01, 0xf7, 0xf0, 0x02, 0xf7]);
        assert_eq!(got, vec![vec![0xf0, 0x01, 0xf7], vec![0xf0, 0x02, 0xf7]]);
    }

    // Clock bytes stream continuously while a dump crosses the wire. Splicing
    // one into the payload shifts every following byte and breaks the decode.
    #[test]
    fn strips_realtime_bytes_spliced_into_a_frame() {
        let mut r = SysExReassembler::new();
        let got = r.push(&[0xf0, 0x01, 0xf8, 0x02, 0xfe, 0x03, 0xf7]);
        assert_eq!(got, vec![vec![0xf0, 0x01, 0x02, 0x03, 0xf7]]);
    }

    #[test]
    fn ignores_traffic_outside_a_frame() {
        let mut r = SysExReassembler::new();
        // A note-on, then a real message.
        let got = r.push(&[0x90, 0x40, 0x7f, 0xf0, 0x01, 0xf7]);
        assert_eq!(got, vec![vec![0xf0, 0x01, 0xf7]]);
    }

    #[test]
    fn drops_a_truncated_frame_rather_than_emitting_it_corrupt() {
        let mut r = SysExReassembler::new();
        // Box is unplugged mid-dump, then a fresh message starts.
        let got = r.push(&[0xf0, 0x01, 0x02, 0xf0, 0x09, 0xf7]);
        assert_eq!(got, vec![vec![0xf0, 0x09, 0xf7]]);
    }

    #[test]
    fn a_stray_terminator_emits_nothing() {
        let mut r = SysExReassembler::new();
        assert!(r.push(&[0xf7]).is_empty());
        assert!(!r.is_mid_frame());
    }
}
