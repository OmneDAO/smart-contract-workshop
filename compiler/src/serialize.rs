//! Deterministic serialization helpers for on-chain storage.
//!
//! Encoding rules:
//! - Integers are little-endian fixed-width.
//! - Bool is 0x00 or 0x01.
//! - Bytes/String are length-prefixed with u32 (little-endian).
//! - Optional is 0x00 for None, 0x01 + payload for Some.
//! - List/Map are length-prefixed, with map entries sorted by raw key bytes.

use std::cmp::Ordering;

pub fn encode_u64(value: u64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn encode_i64(value: i64) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn encode_bool(value: bool) -> Vec<u8> {
    vec![if value { 1 } else { 0 }]
}

pub fn encode_bytes(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + value.len());
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value);
    out
}

pub fn encode_string(value: &str) -> Vec<u8> {
    encode_bytes(value.as_bytes())
}

pub fn encode_optional(value: Option<&[u8]>) -> Vec<u8> {
    match value {
        Some(payload) => {
            let mut out = Vec::with_capacity(1 + payload.len());
            out.push(1);
            out.extend_from_slice(payload);
            out
        }
        None => vec![0],
    }
}

pub fn encode_list(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        out.extend_from_slice(entry);
    }
    out
}

pub fn encode_map(entries: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|(a, _), (b, _)| compare_bytes(a, b));

    let mut out = Vec::new();
    out.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
    for (key, value) in sorted {
        out.extend_from_slice(&encode_bytes(&key));
        out.extend_from_slice(&encode_bytes(&value));
    }
    out
}

fn compare_bytes(left: &[u8], right: &[u8]) -> Ordering {
    let min_len = left.len().min(right.len());
    let prefix_cmp = left[..min_len].cmp(&right[..min_len]);
    if prefix_cmp == Ordering::Equal {
        left.len().cmp(&right.len())
    } else {
        prefix_cmp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_bool_is_deterministic() {
        assert_eq!(encode_bool(false), vec![0]);
        assert_eq!(encode_bool(true), vec![1]);
    }

    #[test]
    fn encode_bytes_prefixes_length() {
        let encoded = encode_bytes(b"hi");
        assert_eq!(&encoded[..4], 2u32.to_le_bytes());
        assert_eq!(&encoded[4..], b"hi");
    }

    #[test]
    fn encode_optional_none_is_single_byte() {
        assert_eq!(encode_optional(None), vec![0]);
    }

    #[test]
    fn encode_optional_some_is_prefixed() {
        let payload = encode_u64(42);
        let encoded = encode_optional(Some(&payload));
        assert_eq!(encoded[0], 1);
        assert_eq!(&encoded[1..], payload);
    }

    #[test]
    fn encode_list_prefixes_length_and_concatenates() {
        let entries = vec![encode_u64(1), encode_u64(2)];
        let encoded = encode_list(&entries);
        assert_eq!(&encoded[..4], 2u32.to_le_bytes());
        assert_eq!(&encoded[4..12], encode_u64(1));
        assert_eq!(&encoded[12..20], encode_u64(2));
    }

    #[test]
    fn encode_map_is_deterministic_for_unordered_entries() {
        let entries_a = vec![(vec![0x02], vec![0xBB]), (vec![0x01], vec![0xAA])];
        let entries_b = vec![(vec![0x01], vec![0xAA]), (vec![0x02], vec![0xBB])];
        assert_eq!(encode_map(&entries_a), encode_map(&entries_b));
    }

    #[test]
    fn encode_map_sorts_by_raw_key_bytes() {
        let entries = vec![
            (vec![0x02], vec![0xBB]),
            (vec![0x01], vec![0xAA]),
            (vec![0x01, 0x01], vec![0xCC]),
        ];
        let encoded = encode_map(&entries);
        let len = u32::from_le_bytes(encoded[0..4].try_into().unwrap());
        assert_eq!(len, 3);
        let first_key_len = u32::from_le_bytes(encoded[4..8].try_into().unwrap()) as usize;
        let first_key = &encoded[8..8 + first_key_len];
        assert_eq!(first_key, &[0x01]);
        let second_offset = 8 + first_key_len + 4 + 1;
        let second_key_len = u32::from_le_bytes(encoded[second_offset..second_offset + 4].try_into().unwrap()) as usize;
        let second_key = &encoded[second_offset + 4..second_offset + 4 + second_key_len];
        assert_eq!(second_key, &[0x01, 0x01]);
    }
}
