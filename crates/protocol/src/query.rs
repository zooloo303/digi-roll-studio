//! The `0x09` Query API: a key → value read, keyed by a string rather than a
//! dump index.
//!
//! Everything else read-only in this crate is either a dump (one byte of
//! index, checksum, addressed by slot) or a directory listing (`drive.rs`,
//! addressed by path). Query is neither: it is a flat namespace of named
//! values, and the only key anyone has documented anywhere is
//! `sample_file.interleaved_stereo_support`, from a TODO comment in elk-herd's
//! own source. Nothing here claims to know the key space — this module only
//! claims to know the wire format around it, so a probe can ask a wordlist of
//! guesses and read back whatever comes.
//!
//! Wire format (reverse-engineered, not from elk-herd — it does not implement
//! this opcode):
//!
//! ```text
//!   request:  key, NUL-terminated Windows-1252 string
//!   response: tag byte, then:
//!     0  none     — no value bytes follow. Still a *reply*: the key exists
//!                   (or at least is recognised) but has nothing to report.
//!     1  bool     — one byte, 0 or 1
//!     2  int      — two u32be words, high then low, forming a 64-bit signed value
//!     3  uint     — two u32be words, high then low, forming a 64-bit unsigned value
//!     4  string   — NUL-terminated Windows-1252 string
//! ```
//!
//! # Read-only by construction
//!
//! This module only builds a request and parses a reply. There is no writer
//! here and the API this is part of has no documented "set" counterpart to
//! build one for.

use crate::device::cstring;
use crate::pattern::u32_be;

/// Query request. Response comes back as `0x89`, per the API's
/// request-plus-0x80 convention.
pub const API_QUERY: u8 = 0x09;

/// A decoded Query reply value.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryValue {
    /// Tag 0. The key was recognised (or at least answered) but carries no
    /// value — still evidence the key *exists*, unlike a plain timeout.
    None,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Str(String),
}

#[derive(Debug, PartialEq)]
pub enum QueryError {
    /// The reply was too short to hold even the tag byte.
    Empty,
    /// A tag byte other than 0-4 — the encoding is not what this module thinks.
    UnknownTag(u8),
    /// The tag promised more bytes than the reply carried.
    Truncated { tag: u8, need: usize, got: usize },
    /// A bool tag whose byte was neither 0 nor 1.
    NotABooleanByte(u8),
    /// The key has a byte this API cannot carry (see [`crate::drive::dir_list_args`]
    /// for the same restriction on paths, for the same reason).
    UnsendableKey(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::Empty => write!(f, "empty query reply — no tag byte"),
            QueryError::UnknownTag(t) => write!(f, "unknown query value tag {t}"),
            QueryError::Truncated { tag, need, got } => {
                write!(f, "query value tag {tag} needs {need} bytes, reply has {got}")
            }
            QueryError::NotABooleanByte(b) => write!(f, "bool tag's byte is {b}, not 0 or 1"),
            QueryError::UnsendableKey(k) => write!(f, "key {k:?} has bytes this API cannot carry"),
        }
    }
}

impl std::error::Error for QueryError {}

/// Build the argument bytes for a Query request: the key as a NUL-terminated
/// Windows-1252 string. Same ASCII-only restriction as
/// [`crate::drive::dir_list_args`], for the same reason: every key this crate
/// sends is a literal it wrote itself, so refusing a byte it cannot encode is
/// safer than encoding it wrongly and asking for the wrong key.
pub fn query_args(key: &str) -> Result<Vec<u8>, QueryError> {
    if !key.is_ascii() || key.contains('\0') {
        return Err(QueryError::UnsendableKey(key.to_string()));
    }
    let mut args = key.as_bytes().to_vec();
    args.push(0);
    Ok(args)
}

/// Parse a Query response's arguments into a [`QueryValue`].
pub fn parse_query_reply(args: &[u8]) -> Result<QueryValue, QueryError> {
    let Some(&tag) = args.first() else { return Err(QueryError::Empty) };
    match tag {
        0 => Ok(QueryValue::None),
        1 => {
            if args.len() < 2 {
                return Err(QueryError::Truncated { tag, need: 2, got: args.len() });
            }
            match args[1] {
                0 => Ok(QueryValue::Bool(false)),
                1 => Ok(QueryValue::Bool(true)),
                b => Err(QueryError::NotABooleanByte(b)),
            }
        }
        2 => {
            if args.len() < 9 {
                return Err(QueryError::Truncated { tag, need: 9, got: args.len() });
            }
            let hi = u32_be(args, 1) as i64;
            let lo = u32_be(args, 5) as i64;
            Ok(QueryValue::Int((hi << 32) | lo))
        }
        3 => {
            if args.len() < 9 {
                return Err(QueryError::Truncated { tag, need: 9, got: args.len() });
            }
            let hi = u32_be(args, 1) as u64;
            let lo = u32_be(args, 5) as u64;
            Ok(QueryValue::UInt((hi << 32) | lo))
        }
        4 => {
            let (s, _) = cstring(args, 1);
            Ok(QueryValue::Str(s))
        }
        t => Err(QueryError::UnknownTag(t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_becomes_a_nul_terminated_string() {
        assert_eq!(
            query_args("sample_file.interleaved_stereo_support").expect("ascii"),
            b"sample_file.interleaved_stereo_support\0"
        );
    }

    #[test]
    fn a_non_ascii_key_is_refused_rather_than_mis_encoded() {
        assert!(matches!(query_args("k\u{e9}y"), Err(QueryError::UnsendableKey(_))));
        assert!(matches!(query_args("a\0b"), Err(QueryError::UnsendableKey(_))));
    }

    #[test]
    fn tag_0_is_none() {
        assert_eq!(parse_query_reply(&[0]).unwrap(), QueryValue::None);
    }

    #[test]
    fn tag_1_is_bool() {
        assert_eq!(parse_query_reply(&[1, 0]).unwrap(), QueryValue::Bool(false));
        assert_eq!(parse_query_reply(&[1, 1]).unwrap(), QueryValue::Bool(true));
        assert!(matches!(parse_query_reply(&[1, 7]), Err(QueryError::NotABooleanByte(7))));
    }

    #[test]
    fn tag_2_and_3_are_64_bit_ints_from_two_u32be_words() {
        let mut args = vec![2u8];
        args.extend_from_slice(&0u32.to_be_bytes());
        args.extend_from_slice(&42u32.to_be_bytes());
        assert_eq!(parse_query_reply(&args).unwrap(), QueryValue::Int(42));

        let mut args = vec![3u8];
        args.extend_from_slice(&1u32.to_be_bytes());
        args.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(parse_query_reply(&args).unwrap(), QueryValue::UInt(1u64 << 32));
    }

    #[test]
    fn tag_4_is_a_nul_terminated_string() {
        let mut args = vec![4u8];
        args.extend_from_slice(b"hello\0");
        assert_eq!(parse_query_reply(&args).unwrap(), QueryValue::Str("hello".into()));
    }

    #[test]
    fn an_unknown_tag_is_an_error_not_a_guess() {
        assert!(matches!(parse_query_reply(&[9]), Err(QueryError::UnknownTag(9))));
    }

    #[test]
    fn an_empty_reply_is_an_error_not_a_panic() {
        assert!(matches!(parse_query_reply(&[]), Err(QueryError::Empty)));
    }
}
