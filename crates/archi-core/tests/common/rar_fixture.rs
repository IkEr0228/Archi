//! Minimal RAR4 Store fixture, generated without an external archiver.
use std::io::Write;
use std::path::Path;

fn crc(bytes: &[u8]) -> u32 {
    let mut value = !0_u32;
    for byte in bytes {
        value ^= u32::from(*byte);
        for _ in 0..8 {
            value = (value >> 1) ^ (0xedb88320_u32 & 0_u32.wrapping_sub(value & 1));
        }
    }
    !value
}

fn header(file: &mut std::fs::File, body: &[u8]) {
    file.write_all(&(crc(body) as u16).to_le_bytes()).unwrap();
    file.write_all(body).unwrap();
}

pub fn stored(path: &Path, name: &str, payload: &[u8], corrupt: bool) {
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(b"Rar!\x1a\x07\x00").unwrap();
    header(&mut file, &[0x73, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0]);
    let mut body = vec![0x74, 0, 0x80];
    body.extend_from_slice(&(32_u16 + name.len() as u16).to_le_bytes());
    body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    body.push(2); // Windows host.
    body.extend_from_slice(&(crc(payload) ^ u32::from(corrupt)).to_le_bytes());
    body.extend_from_slice(&0_u32.to_le_bytes());
    body.extend_from_slice(&[20, 0x30]);
    body.extend_from_slice(&(name.len() as u16).to_le_bytes());
    body.extend_from_slice(&0x20_u32.to_le_bytes());
    body.extend_from_slice(name.as_bytes());
    header(&mut file, &body);
    file.write_all(payload).unwrap();
    header(&mut file, &[0x7b, 0, 0, 7, 0]);
}
