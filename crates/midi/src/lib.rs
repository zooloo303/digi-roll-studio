use digi_protocol::protocol::split_sysex_stream;

pub mod device;
pub mod ports;
pub mod preset_load;
pub mod preset_scan;
pub mod sysex_stream;

pub use device::{DumpResponse, ElektronDevice, KIT_TRACKS};
pub use ports::{
    capture_sysex, list_inputs, list_outputs, open_output_by_name, PortBinding, PortInfo,
};

/// Re-exported so `engine` can hold an open output without depending on `midir`
/// itself: this crate stays the only one that knows what the backend is.
pub use midir::MidiOutputConnection;

pub trait MidiSender {
    fn send(&mut self, data: &[u8]) -> Result<(), MidiError>;
}

pub trait MidiReceiver {
    fn receive(&mut self, buf: &mut [u8]) -> Result<usize, MidiError>;
}

#[derive(Debug)]
pub enum MidiError {
    Io(String),
    /// The MIDI subsystem itself would not start.
    Init(String),
    /// A remembered binding matched no port present now — device unplugged.
    PortNotFound(String),
    Connect(String),
    Send(String),
    Timeout,
    /// The input connection went away: the device was unplugged mid-request.
    Disconnected,
    /// Every retry of an API request timed out.
    NoReply { api_id: u8, tries: u32, last: Box<MidiError> },
    /// A dump message failed its checksum or count check.
    CorruptDump { dump_type: u8, index: u8 },
    /// The firmware allowlist refused a store before any byte left this machine.
    /// Carries `write_gate`'s own wording, which is written to be shown verbatim.
    WriteRefused(String),
    /// A dump stream went silent before its end-of-stream marker.
    DumpStalled { messages: usize },
    Protocol(digi_protocol::device::DeviceError),
}

impl std::fmt::Display for MidiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiError::Io(m) => write!(f, "{m}"),
            MidiError::Init(m) => write!(f, "could not start MIDI: {m}"),
            MidiError::PortNotFound(name) => write!(f, "MIDI port not found: {name}"),
            MidiError::Connect(m) => write!(f, "could not open MIDI port: {m}"),
            MidiError::Send(m) => write!(f, "could not send MIDI: {m}"),
            MidiError::Timeout => write!(f, "timed out"),
            MidiError::Disconnected => write!(f, "device disconnected"),
            MidiError::NoReply { api_id, tries, .. } => {
                write!(f, "no reply to API request 0x{api_id:02x} ({tries} tries)")
            }
            MidiError::CorruptDump { dump_type, index } => {
                write!(f, "corrupt dump message (type 0x{dump_type:02x}, slot {index})")
            }
            MidiError::WriteRefused(reason) => write!(f, "nothing was sent: {reason}"),
            MidiError::DumpStalled { messages: 0 } => write!(f, "no response to dump request"),
            MidiError::DumpStalled { messages } => {
                write!(f, "dump stream stalled after {messages} messages")
            }
            MidiError::Protocol(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for MidiError {}

impl From<midir::InitError> for MidiError {
    fn from(e: midir::InitError) -> Self {
        MidiError::Init(e.to_string())
    }
}

impl From<digi_protocol::device::DeviceError> for MidiError {
    fn from(e: digi_protocol::device::DeviceError) -> Self {
        MidiError::Protocol(e)
    }
}

pub struct SysExMessage {
    pub bytes: Vec<u8>,
}

pub fn send_sysex<S: MidiSender>(sender: &mut S, msg: &[u8]) -> Result<(), MidiError> {
    sender.send(msg)
}

pub fn parse_incoming(data: &[u8]) -> Vec<digi_protocol::protocol::ParsedSysEx> {
    split_sysex_stream(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummySender;
    impl MidiSender for DummySender {
        fn send(&mut self, _data: &[u8]) -> Result<(), MidiError> {
            Ok(())
        }
    }

    #[test]
    fn send_sysex_ok() {
        let mut d = DummySender;
        send_sysex(&mut d, &[0xf0, 0x7e, 0xf7]).unwrap();
    }
}
