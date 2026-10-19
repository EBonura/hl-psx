//! The options as one `psx-settings` memory-card record.
//!
//! `Profile<0, 0>` (no action map, no scores) carries what the shared profile
//! already says the way HL does: the two stick deadzones in raw counts (both
//! clamp at 64) and the music and SFX volumes as a percentage. HL's 0..=8 volume
//! steps map out and back exactly: `step * 100 / 8` rounds back to the step.
//! HL's picture options do not fit the shared fields (its screen offset spans
//! +-24 where the profile's spans +-16, its brightness has eight levels where the
//! profile's has eleven), so they live in the game-owned bytes, where 0 means
//! "never written, use the default".
//!
//! Pure arithmetic, no hardware: the host runner (`hl-logic-tests`) includes
//! this file and runs the tests below. The card transport is `save.rs`.

use psx_display::ScreenOffset;
use psx_pad::ActionMap;
use psx_settings::Profile;

/// Music and SFX sliders run 0..=VOL_MAX.
pub const VOL_MAX: u8 = 8;
pub const ANALOG_DEADZONE_DEFAULT: u8 = 28;
pub const ANALOG_DEADZONE_MAX: u8 = 64;
/// Brightness levels, zero-based `0..BRIGHTNESS_LEVELS`.
pub const BRIGHTNESS_LEVELS: u8 = 8;
/// Default to displayed level 2 (zero-based index 1).
pub const DEFAULT_BRIGHTNESS: u8 = 1;
/// Half-Life's television centering range, in pixels each way.
pub const SCREEN_RANGE: i8 = 24;

/// What a card record holds for this game.
pub type Saved = Profile<0, 0>;

/// Game byte 0: brightness level + 1 (0 = never written).
const BRIGHTNESS_BYTE: usize = 0;
/// Game bytes 1 and 2: screen X and Y offset (`ScreenOffset::to_bytes`).
const SCREEN_BYTES: usize = 1;

/// Every option the card remembers, as the game holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub move_deadzone: u8,
    pub look_deadzone: u8,
    pub music: u8,
    pub sfx: u8,
    /// Zero-based level, `0..BRIGHTNESS_LEVELS`.
    pub brightness: u8,
    pub screen: ScreenOffset<SCREEN_RANGE>,
}

impl Options {
    pub const DEFAULT: Options = Options {
        move_deadzone: ANALOG_DEADZONE_DEFAULT,
        look_deadzone: ANALOG_DEADZONE_DEFAULT,
        music: VOL_MAX,
        sfx: VOL_MAX,
        brightness: DEFAULT_BRIGHTNESS,
        screen: ScreenOffset::CENTRE,
    };

    /// The card record for these options.
    pub fn to_record(&self) -> Saved {
        let mut p = Saved::new(ActionMap::new([]));
        p.move_deadzone = self.move_deadzone;
        p.look_deadzone = self.look_deadzone;
        p.music_volume = (self.music as u32 * 100 / VOL_MAX as u32) as u8;
        p.sfx_volume = (self.sfx as u32 * 100 / VOL_MAX as u32) as u8;
        p.game_bytes[BRIGHTNESS_BYTE] = self.brightness + 1;
        let [x, y] = self.screen.to_bytes();
        p.game_bytes[SCREEN_BYTES] = x;
        p.game_bytes[SCREEN_BYTES + 1] = y;
        p
    }

    /// The options a card record holds. Every value is range-checked: a record
    /// that decodes is intact, not necessarily sensible, and a game byte the
    /// record never wrote reads as the default.
    pub fn from_record(p: &Saved) -> Options {
        let level = p.game_bytes[BRIGHTNESS_BYTE];
        Options {
            move_deadzone: p.move_deadzone.min(ANALOG_DEADZONE_MAX),
            look_deadzone: p.look_deadzone.min(ANALOG_DEADZONE_MAX),
            music: volume_step(p.music_volume),
            sfx: volume_step(p.sfx_volume),
            brightness: if (1..=BRIGHTNESS_LEVELS).contains(&level) {
                level - 1
            } else {
                DEFAULT_BRIGHTNESS
            },
            screen: ScreenOffset::from_bytes([
                p.game_bytes[SCREEN_BYTES],
                p.game_bytes[SCREEN_BYTES + 1],
            ]),
        }
    }
}

/// A 0..=100 percentage as the nearest slider step.
fn volume_step(percent: u8) -> u8 {
    ((percent as u32 * VOL_MAX as u32 + 50) / 100).min(VOL_MAX as u32) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn through_the_card(p: &Saved) -> Saved {
        let mut bytes = [0u8; psx_settings::MAX_RECORD_LEN];
        let len = p.encode(&mut bytes).expect("a record fits its block");
        Saved::decode(&bytes[..len]).expect("a record reads back")
    }

    #[test]
    fn every_slider_step_survives_the_card() {
        for step in 0..=VOL_MAX {
            for level in 0..BRIGHTNESS_LEVELS {
                let want = Options {
                    move_deadzone: step * 8,
                    look_deadzone: ANALOG_DEADZONE_MAX - step * 8,
                    music: step,
                    sfx: VOL_MAX - step,
                    brightness: level,
                    screen: ScreenOffset::new(step as i8 * 6 - 24, 24 - step as i8 * 6),
                };
                let back = Options::from_record(&through_the_card(&want.to_record()));
                assert_eq!(back, want, "step {step} level {level}");
            }
        }
    }

    #[test]
    fn the_screen_offset_keeps_its_full_range() {
        for (x, y) in [(-24, 24), (24, -24), (0, 0), (17, -19)] {
            let want = Options {
                screen: ScreenOffset::new(x, y),
                ..Options::DEFAULT
            };
            let back = Options::from_record(&through_the_card(&want.to_record()));
            assert_eq!((back.screen.x(), back.screen.y()), (x, y));
        }
    }

    #[test]
    fn a_fresh_record_reads_as_the_defaults() {
        let fresh = Saved::new(ActionMap::new([]));
        let back = Options::from_record(&fresh);
        // Brightness and screen are the game bytes that read 0 when never
        // written; the shared fields are whatever the profile's own defaults say.
        assert_eq!(back.brightness, DEFAULT_BRIGHTNESS);
        assert_eq!(back.screen, ScreenOffset::CENTRE);
    }

    #[test]
    fn a_damaged_value_is_clamped_not_trusted() {
        let mut p = Saved::new(ActionMap::new([]));
        p.game_bytes[BRIGHTNESS_BYTE] = 200;
        p.game_bytes[SCREEN_BYTES] = 100;
        p.game_bytes[SCREEN_BYTES + 1] = 156; // -100 as a byte
        p.music_volume = 100;
        p.sfx_volume = 100;
        p.move_deadzone = 64;
        p.look_deadzone = 64;
        let back = Options::from_record(&p);
        assert_eq!(back.brightness, DEFAULT_BRIGHTNESS);
        assert_eq!((back.screen.x(), back.screen.y()), (24, -24));
        assert_eq!((back.music, back.sfx), (VOL_MAX, VOL_MAX));
        assert_eq!((back.move_deadzone, back.look_deadzone), (64, 64));
    }

    #[test]
    fn the_default_options_are_the_ones_the_game_boots_with() {
        let d = Options::DEFAULT;
        assert_eq!(d.move_deadzone, ANALOG_DEADZONE_DEFAULT);
        assert_eq!(Options::from_record(&through_the_card(&d.to_record())), d);
    }
}
