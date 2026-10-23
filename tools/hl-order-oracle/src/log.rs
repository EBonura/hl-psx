//! Parser for the guest's ordering stream (`game/src/order_trace.rs`).

use std::collections::HashMap;

/// Emission class carried by an `OP` record.
pub mod kind {
    pub const LOOP_FACE: u8 = 1;
    pub const RAW_TRI: u8 = 2;
    pub const PATCH: u8 = 3;
    pub const SUBMODEL: u8 = 4;
    pub const BACKSTOP: u8 = 5;
    pub const UNDERLAY: u8 = 6;
    pub const DEPTH_SPLIT: u8 = 7;
    pub const BEAM: u8 = 8;
    pub const MODEL: u8 = 9;
    pub const SKIRT: u8 = 10;
}

/// Depth information the guest recorded for one packet at its table insertion.
#[derive(Clone, Copy, Debug)]
pub struct Info {
    pub kind: u8,
    pub mode: u8,
    pub n: u8,
    pub key: u16,
    pub face: u16,
    pub policy: u8,
    pub z: [u16; 4],
    /// Submodel number of the brush entity being drawn (0 = world) and its draw offset.
    pub brush: u16,
    pub offset: [i16; 3],
}

/// One packet of the table walk, in draw order.
#[derive(Clone, Debug)]
pub struct Walked {
    pub addr: u32,
    /// Packet words after the tag word.
    pub words: Vec<u32>,
    pub info: Option<Info>,
}

/// One streamed frame.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub index: u32,
    pub eye: [i32; 3],
    /// View rotation rows (Q12) and projection distance.
    pub rot: [[f64; 3]; 3],
    pub h: f64,
    pub walk: Vec<Walked>,
    /// The `OE` count the guest reported (must equal `walk.len()`).
    pub reported: u32,
    /// Label of the pose (the nearest preceding `probe` tp log), when the log has one.
    pub label: String,
}

fn hex(field: &str) -> Option<u32> {
    u32::from_str_radix(field, 16).ok()
}

/// Parse every frame in a debug log. Lines that are not oracle records are ignored.
pub fn parse(text: &str) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut current: Option<Frame> = None;
    let mut infos: HashMap<u32, Info> = HashMap::new();
    let mut label = String::new();
    for raw in text.lines() {
        if let Some(pos) = raw.find("probe event=tp") {
            label = raw[pos..].trim().to_string();
        }
        let Some(pos) = find_record(raw) else {
            continue;
        };
        let line = &raw[pos..];
        let fields: Vec<&str> = line.trim_end().split('|').collect();
        match fields[0] {
            "OF" if fields.len() >= 15 => {
                let q = |i: usize| f64::from(hex(fields[i]).unwrap_or(0) as u16 as i16) / 4096.0;
                infos.clear();
                current = Some(Frame {
                    index: hex(fields[1]).unwrap_or(0),
                    eye: [
                        hex(fields[2]).unwrap_or(0) as i32,
                        hex(fields[3]).unwrap_or(0) as i32,
                        hex(fields[4]).unwrap_or(0) as i32,
                    ],
                    rot: [
                        [q(5), q(6), q(7)],
                        [q(8), q(9), q(10)],
                        [q(11), q(12), q(13)],
                    ],
                    h: f64::from(hex(fields[14]).unwrap_or(256)),
                    label: label.clone(),
                    ..Frame::default()
                });
            }
            "OP" if fields.len() >= 16 => {
                let (Some(addr), Some(kind), Some(mode), Some(n), Some(key), Some(face), Some(pol)) = (
                    hex(fields[1]),
                    hex(fields[2]),
                    hex(fields[3]),
                    hex(fields[4]),
                    hex(fields[5]),
                    hex(fields[6]),
                    hex(fields[7]),
                ) else {
                    continue;
                };
                let mut z = [0u16; 4];
                for (slot, field) in z.iter_mut().zip(&fields[8..12]) {
                    *slot = hex(field).unwrap_or(0) as u16;
                }
                infos.insert(
                    addr,
                    Info {
                        kind: kind as u8,
                        mode: mode as u8,
                        n: n as u8,
                        key: key as u16,
                        face: face as u16,
                        policy: pol as u8,
                        z,
                        brush: hex(fields[12]).unwrap_or(0) as u16,
                        offset: [
                            hex(fields[13]).unwrap_or(0) as u16 as i16,
                            hex(fields[14]).unwrap_or(0) as u16 as i16,
                            hex(fields[15]).unwrap_or(0) as u16 as i16,
                        ],
                    },
                );
            }
            "OW" if fields.len() >= 3 => {
                let Some(frame) = current.as_mut() else {
                    continue;
                };
                let Some(addr) = hex(fields[1]) else {
                    continue;
                };
                let body = fields[2];
                let words = (0..body.len() / 8)
                    .filter_map(|i| hex(&body[i * 8..i * 8 + 8]))
                    .collect();
                frame.walk.push(Walked {
                    addr,
                    words,
                    info: infos.get(&addr).copied(),
                });
            }
            "OE" if fields.len() >= 2 => {
                if let Some(mut frame) = current.take() {
                    frame.reported = hex(fields[1]).unwrap_or(0);
                    frames.push(frame);
                }
            }
            _ => {}
        }
    }
    frames
}

/// Position of an oracle record tag (`OF|`, `OP|`, `OW|`, `OE|`) in a log line, tolerating a
/// prefix the frontend adds to guest debug lines.
fn find_record(line: &str) -> Option<usize> {
    for tag in ["OF|", "OP|", "OW|", "OE|"] {
        if let Some(pos) = line.find(tag) {
            if pos == 0 || !line.as_bytes()[pos - 1].is_ascii_alphanumeric() {
                return Some(pos);
            }
        }
    }
    None
}

/// Rewrite every packet's cooked face number to the source BSP's, with the cooker's
/// `source_face,cooked_face` provenance report (`HLPSX_FACE_TOPOLOGY_REPORT`).
pub fn remap_faces(frames: &mut [Frame], csv: &str) {
    let mut cooked_to_source: Vec<u32> = Vec::new();
    for line in csv.lines().skip(1) {
        let mut f = line.split(',');
        let (Some(src), Some(cooked)) = (f.next(), f.next()) else {
            continue;
        };
        let (Ok(src), Ok(cooked)) = (src.parse::<u32>(), cooked.parse::<usize>()) else {
            continue;
        };
        if cooked_to_source.len() <= cooked {
            cooked_to_source.resize(cooked + 1, u32::MAX);
        }
        cooked_to_source[cooked] = src;
    }
    for frame in frames {
        for walked in &mut frame.walk {
            if let Some(info) = walked.info.as_mut() {
                if let Some(src) = cooked_to_source
                    .get(usize::from(info.face))
                    .filter(|s| **s != u32::MAX)
                {
                    info.face = *src as u16;
                }
            }
        }
    }
}
