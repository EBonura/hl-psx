//! Putting the movie's sound and picture back together in one sector stream.
//!
//! The encoder writes a picture sector run with one sound sector in
//! sixteen, the sound sector ahead of the picture it belongs to. The sound
//! plays from the moment its sector arrives, while the player shows a
//! picture a frame's reading time plus a decoder headroom after the
//! picture's last sector arrives, so the picture lags the sound by that much.
//! Moving every sound sector later by the same time puts them together; the
//! cook does that here, since the drive plays what it reads when it reads it.

use crate::xa::{SECTOR, SUBMODE_AUDIO};

/// Sectors between two sound sectors of a mono 37.8 kHz stream read at double
/// speed: 150 sectors a second, 9.375 sound sectors a second.
pub const AUDIO_PERIOD: usize = 16;

/// `picture`: the encoder's stream, whose sound sectors are discarded;
/// `sound`: the re-encoded sound sectors. The result has the picture sectors
/// in their original order and a sound sector at every [`AUDIO_PERIOD`]th
/// sector from `delay` on. Sound that would come after the last picture sector
/// is not kept (the player has stopped reading by then; the movie's last
/// moments are silent).
pub fn interleave(picture: &[u8], sound: &[u8], delay: usize) -> Result<Vec<u8>, String> {
    if !picture.len().is_multiple_of(SECTOR) || !sound.len().is_multiple_of(SECTOR) {
        return Err("a stream is not whole sectors".into());
    }
    let video: Vec<&[u8]> = picture
        .chunks_exact(SECTOR)
        .filter(|s| s[2] & SUBMODE_AUDIO == 0)
        .collect();
    let mut audio = sound.chunks_exact(SECTOR);
    let mut out = Vec::with_capacity(picture.len());
    let (mut at, mut shown) = (0usize, 0usize);
    // Every slot takes the next picture sector, except a sound slot while
    // sound lasts. The stream ends with the last picture sector.
    while shown < video.len() {
        let is_sound = at >= delay && (at - delay).is_multiple_of(AUDIO_PERIOD);
        match is_sound.then(|| audio.next()).flatten() {
            Some(sector) => out.extend_from_slice(sector),
            None => {
                out.extend_from_slice(video[shown]);
                shown += 1;
            }
        }
        at += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(kinds: &[(bool, u8)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (audio, tag) in kinds {
            let mut s = vec![*tag; SECTOR];
            s[2] = if *audio { SUBMODE_AUDIO } else { 0x08 };
            out.extend_from_slice(&s);
        }
        out
    }

    fn tags(raw: &[u8]) -> Vec<(bool, u8)> {
        raw.chunks_exact(SECTOR)
            .map(|s| (s[2] & SUBMODE_AUDIO != 0, s[8]))
            .collect()
    }

    #[test]
    fn sound_moves_later_and_the_picture_keeps_its_order() {
        let picture = stream(&[(true, 0), (false, 1), (false, 2), (false, 3), (false, 4)]);
        let sound = stream(&[(true, 100), (true, 101)]);
        let out = interleave(&picture, &sound, 2).unwrap();
        assert_eq!(
            tags(&out),
            vec![(false, 1), (false, 2), (true, 100), (false, 3), (false, 4)]
        );
    }

    #[test]
    fn a_zero_delay_starts_with_sound_as_the_encoder_does() {
        let picture = stream(&[(false, 1), (false, 2)]);
        let sound = stream(&[(true, 100)]);
        let out = interleave(&picture, &sound, 0).unwrap();
        assert_eq!(tags(&out), vec![(true, 100), (false, 1), (false, 2)]);
    }

    #[test]
    fn sound_after_the_last_picture_is_dropped() {
        let picture = stream(&[(false, 1), (false, 2)]);
        let sound = stream(&[(true, 100)]);
        let out = interleave(&picture, &sound, 2).unwrap();
        assert_eq!(tags(&out), vec![(false, 1), (false, 2)]);
    }

    #[test]
    fn sound_repeats_every_sixteenth_sector() {
        let video: Vec<(bool, u8)> = (0..40).map(|i| (false, i as u8)).collect();
        let sound = stream(&[(true, 200), (true, 201), (true, 202)]);
        let out = interleave(&stream(&video), &sound, 3).unwrap();
        let positions: Vec<usize> = tags(&out)
            .iter()
            .enumerate()
            .filter(|(_, (a, _))| *a)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions, vec![3, 19, 35]);
    }
}
