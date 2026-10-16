//! Decoding the mono XA-ADPCM that `psxavenc` writes, so the cook can work on
//! the movie's sound as samples. The layout is the CD-ROM XA one: a sector
//! carries 18 sound groups of 128 bytes, each a 16-byte header (the unit
//! parameters, repeated) and 28 words of 4 bytes, one sample of every unit per
//! word (eight units of 4 bits, or four of 8).

/// Bytes of a raw sector record: 8-byte subheader and 2328 bytes.
pub const SECTOR: usize = 2336;
pub const SUBMODE_AUDIO: u8 = 0x04;
const GROUPS: usize = 18;
const GROUP_BYTES: usize = 128;
const WORDS: usize = 28;
/// Coding information bits: stereo, 18.9 kHz, 8-bit.
const CODING_STEREO: u8 = 0x01;
const CODING_HALF_RATE: u8 = 0x04;
const CODING_8BIT: u8 = 0x10;

/// Predictor taps in 1/64 for filters 0 to 3 (XA uses four of the five).
const F0: [i32; 4] = [0, 60, 115, 98];
const F1: [i32; 4] = [0, 0, -52, -55];

/// Decodes every audio sector of `raw` (2336-byte records) as one mono
/// 37.8 kHz stream. Anything else (stereo, 18.9 kHz, a missing sector) is an
/// error, since the cook asked the encoder for exactly this.
pub fn decode_mono_37800(raw: &[u8]) -> Result<Vec<i16>, String> {
    if !raw.len().is_multiple_of(SECTOR) {
        return Err(format!(
            "{} bytes is not whole {SECTOR}-byte sectors",
            raw.len()
        ));
    }
    let mut out = Vec::with_capacity(raw.len() / SECTOR * GROUPS * 8 * WORDS);
    let (mut old, mut older) = (0i32, 0i32);
    for (index, sector) in raw.chunks_exact(SECTOR).enumerate() {
        if sector[2] & SUBMODE_AUDIO == 0 {
            return Err(format!("sector {index} is not audio"));
        }
        let coding = sector[3];
        if coding & (CODING_STEREO | CODING_HALF_RATE) != 0 {
            return Err(format!(
                "sector {index}: coding {coding:#04x} is not mono 37.8 kHz"
            ));
        }
        let eight = coding & CODING_8BIT != 0;
        let units = if eight { 4 } else { 8 };
        for group in sector[8..8 + GROUPS * GROUP_BYTES].chunks_exact(GROUP_BYTES) {
            for unit in 0..units {
                // Parameters of units 0..3 at 0..3, of 4..7 at 8..11.
                let param = group[if unit < 4 { unit } else { unit + 4 }];
                let range = u32::from(param & 0x0F).min(12);
                let filter = usize::from((param >> 4) & 3);
                for word in 0..WORDS {
                    let data = &group[16 + word * 4..];
                    let coded = if eight {
                        i32::from(data[unit] as i8) << 8
                    } else {
                        let nibble = (data[unit / 2] >> ((unit % 2) * 4)) & 0x0F;
                        i32::from((u16::from(nibble) << 12) as i16)
                    };
                    let predicted = (old * F0[filter] + older * F1[filter] + 32) >> 6;
                    let sample = ((coded >> range) + predicted).clamp(-32768, 32767);
                    older = old;
                    old = sample;
                    out.push(sample as i16);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sector(coding: u8) -> Vec<u8> {
        let mut s = vec![0u8; SECTOR];
        s[0] = 1;
        s[2] = SUBMODE_AUDIO | 0x20;
        s[3] = coding;
        s
    }

    #[test]
    fn four_bit_samples_scale_by_the_range_with_no_predictor() {
        let mut s = sector(0);
        // First group: unit 0 range 12 filter 0, unit 1 range 8; word 0 holds
        // nibble 0x7 for unit 0 and 0x9 (negative) for unit 1.
        s[8] = 12;
        s[8 + 1] = 8;
        s[8 + 16] = 0x97;
        let pcm = decode_mono_37800(&s).unwrap();
        assert_eq!(pcm.len(), GROUPS * 8 * WORDS);
        assert_eq!(pcm[0], 0x7000 >> 12);
        // Unit 1 starts after unit 0's 28 samples. Nibble 9 is 0x9000 as i16.
        assert_eq!(pcm[WORDS], (0x9000u16 as i16) >> 8);
    }

    #[test]
    fn eight_bit_samples_use_four_units_per_group() {
        let mut s = sector(CODING_8BIT);
        s[8] = 8;
        s[8 + 16] = 0x40;
        let pcm = decode_mono_37800(&s).unwrap();
        assert_eq!(pcm.len(), GROUPS * 4 * WORDS);
        assert_eq!(pcm[0], 0x4000 >> 8);
    }

    #[test]
    fn the_predictor_carries_from_one_sample_to_the_next() {
        let mut s = sector(0);
        // Unit 0 range 12 filter 1: second sample adds 60/64 of the first.
        s[8] = 12 | 0x10;
        s[8 + 16] = 0x04;
        s[8 + 20] = 0x00;
        let pcm = decode_mono_37800(&s).unwrap();
        assert_eq!(pcm[0], 4);
        assert_eq!(pcm[1], (4 * 60 + 32) >> 6);
    }

    #[test]
    fn stereo_and_foreign_sectors_are_refused() {
        assert!(decode_mono_37800(&sector(CODING_STEREO)).is_err());
        assert!(decode_mono_37800(&sector(CODING_HALF_RATE)).is_err());
        let mut video = sector(0);
        video[2] = 0x08;
        assert!(decode_mono_37800(&video).is_err());
        assert!(decode_mono_37800(&[0u8; SECTOR + 1]).is_err());
    }
}
