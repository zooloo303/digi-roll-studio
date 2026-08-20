//! Seven-bit ↔ eight-bit payload packing, Elektron flavour.
//!
//! Wire format: groups of up to 7 data bytes, each preceded by one header byte
//! carrying their high bits — header bit 6 is the MSB of the first data byte,
//! bit 5 the second, and so on. A short final group keeps its header byte with
//! the high bits still packed from the top; absent trailing bytes are omitted.
//!
//! Ported from elk-herd's `src/ByteArray/SevenBit.elm` — BSD-2-Clause, © mzero.
//! See `CREDITS.md`.

pub fn encode7(data: &[u8]) -> Vec<u8> {
    let groups = (data.len() + 6) / 7;
    let mut out = Vec::with_capacity(data.len() + groups);
    for chunk in data.chunks(7) {
        let mut head = 0u8;
        for (i, &b) in chunk.iter().enumerate() {
            if b & 0x80 != 0 {
                // bit 6-i
                head |= 1u8 << (6 - i);
            }
        }
        out.push(head);
        for &b in chunk {
            out.push(b & 0x7f);
        }
    }
    out
}

pub fn decode7(wire: &[u8]) -> Vec<u8> {
    if wire.len() % 8 == 1 {
        panic!("7-bit data ends with a lone header byte");
    }
    // rough capacity estimate
    let mut out = Vec::with_capacity(wire.len());
    for g in (0..wire.len()).step_by(8) {
        let head = wire[g];
        let end = std::cmp::min(g + 8, wire.len());
        // data bytes start at g+1
        for i in g + 1..end {
            let shift = (i - g) as u8;
            let high = ((head << shift) & 0x80) as u8;
            out.push(wire[i] | high);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let data: Vec<u8> = (0u8..=255).collect();
        let encoded = encode7(&data);
        let decoded = decode7(&encoded);
        assert_eq!(data, decoded);
    }

    #[test]
    fn empty() {
        assert_eq!(encode7(&[]), Vec::<u8>::new());
        assert_eq!(decode7(&[]), Vec::<u8>::new());
    }
}
