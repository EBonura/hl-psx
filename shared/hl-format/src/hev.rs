//! The HEV suit's voice: which sentences the suit speaks in single player and
//! how each one is spoken as a sequence of recorded words.
//!
//! `sentences.txt` defines each sentence as a list of `fvox/` words; this
//! table keeps the ones the game logic (player.cpp `TakeDamage`,
//! `CItemBattery`, the weapons' `HEV_AMO0`, `CItemLongJump`) asks for. Words
//! are stored once and played one after the other, so the leading tone runs
//! ("boop, boop, boop,") and the battery's number words are shared by every
//! sentence that uses them instead of being repeated in each.
//!
//! A map that can use the suit carries a block after its dialogue pack (the
//! same chunk, past the last HSFX sample); the chunk's last word is the
//! block's offset from the chunk start:
//!
//! ```text
//! "HEVB"  u16 word_count  u16 reserved  u32 spu_bytes
//! word_count x (u32 offset, u32 length)   offsets from the block start,
//!                                         length 0 = word not in this map
//! PSAU samples
//! ```
//!
//! `spu_bytes` is the SPU RAM the words take once uploaded one after the
//! other from the end of the dialogue pack, each rounded to a 16 byte block
//! and followed by the 16 byte parking block every other sample has.

pub const MAGIC: [u8; 4] = *b"HEVB";
pub const HEADER_BYTES: usize = 12;
pub const ENTRY_BYTES: usize = 8;
/// SPU RAM taken by a word's parking block.
pub const PARK_BYTES: u32 = 16;
/// Words a sentence can have.
pub const MAX_PARTS: usize = 4;
/// Marks an unused part slot.
pub const NO_WORD: u8 = u8::MAX;

/// Paths below `sound/`, concatenated in order to make one word.
pub type Word = &'static [&'static str];

const BOOP: &str = "fvox/boop.wav";
const BEEP: &str = "fvox/beep.wav";
const BLIP: &str = "fvox/blip.wav";
const COMMA: &str = "fvox/_comma.wav";

pub const W_BOOPS: u8 = 0;
pub const W_BEEPS2: u8 = 1;
pub const W_BEEPS3: u8 = 2;
pub const W_BLIPS3: u8 = 3;
pub const W_BLIP: u8 = 4;
pub const W_BLIPS2: u8 = 5;
pub const W_HISS: u8 = 6;
pub const W_MINOR_LACERATIONS: u8 = 7;
pub const W_MAJOR_LACERATIONS: u8 = 8;
pub const W_INTERNAL_BLEEDING: u8 = 9;
pub const W_BLOOD_TOXINS: u8 = 10;
pub const W_MINOR_FRACTURE: u8 = 11;
pub const W_MAJOR_FRACTURE: u8 = 12;
pub const W_BLOOD_LOSS: u8 = 13;
pub const W_SEEK_MEDIC: u8 = 14;
pub const W_BIOHAZARD: u8 = 15;
pub const W_CHEMICAL: u8 = 16;
pub const W_RADIATION: u8 = 17;
pub const W_AUTOMEDIC: u8 = 18;
pub const W_MORPHINE: u8 = 19;
pub const W_HEALTH_DROPPING: u8 = 20;
pub const W_HEALTH_CRITICAL: u8 = 21;
pub const W_NEAR_DEATH: u8 = 22;
pub const W_AMMO_DEPLETED: u8 = 23;
pub const W_POWERMOVE: u8 = 24;
pub const W_RESTORED: u8 = 25;
pub const W_LEVEL: u8 = 26;
pub const W_TEN: u8 = 27;
pub const W_FIFTEEN: u8 = 28;
pub const W_TWENTY: u8 = 29;
pub const W_THIRTY: u8 = 30;
pub const W_FOURTY: u8 = 31;
pub const W_FIFTY: u8 = 32;
pub const W_SIXTY: u8 = 33;
pub const W_SEVENTY: u8 = 34;
pub const W_EIGHTY: u8 = 35;
pub const W_NINETY: u8 = 36;
pub const W_FIVE: u8 = 37;
pub const W_ONEHUNDRED: u8 = 38;
pub const W_PERCENT: u8 = 39;
pub const WORD_COUNT: usize = 40;

pub const WORDS: [Word; WORD_COUNT] = [
    &[BOOP, COMMA, BOOP, COMMA, BOOP, COMMA],
    &[BEEP, COMMA, BEEP, COMMA],
    &[BEEP, COMMA, BEEP, COMMA, BEEP, COMMA],
    &[BLIP, BLIP, BLIP, COMMA],
    &[BLIP],
    &[BLIP, BLIP],
    &["fvox/hiss.wav"],
    &["fvox/minor_lacerations.wav"],
    &["fvox/major_lacerations.wav"],
    &["fvox/internal_bleeding.wav"],
    &["fvox/blood_toxins.wav"],
    &["fvox/minor_fracture.wav"],
    &["fvox/major_fracture.wav"],
    &["fvox/blood_loss.wav"],
    &["fvox/seek_medic.wav"],
    &["fvox/biohazard_detected.wav"],
    &["fvox/chemical_detected.wav"],
    &["fvox/radiation_detected.wav"],
    &["fvox/automedic_on.wav"],
    &["fvox/morphine_shot.wav"],
    &["fvox/health_dropping2.wav"],
    &["fvox/health_critical.wav"],
    &["fvox/near_death.wav"],
    &["fvox/ammo_depleted.wav"],
    &["fvox/powermove_on.wav"],
    &["fvox/fuzz.wav", COMMA, "fvox/power_restored.wav"],
    &["fvox/fuzz.wav", COMMA, "fvox/power_level_is.wav"],
    &["fvox/ten.wav"],
    &["fvox/fifteen.wav"],
    &["fvox/twenty.wav"],
    &["fvox/thirty.wav"],
    &["fvox/fourty.wav"],
    &["fvox/fifty.wav"],
    &["fvox/sixty.wav"],
    &["fvox/seventy.wav"],
    &["fvox/eighty.wav"],
    &["fvox/ninety.wav"],
    &["fvox/five.wav"],
    &["fvox/onehundred.wav"],
    &["fvox/percent.wav"],
];

// Sentence ids, in the order a map's byte budget keeps them: the ones the
// fall, combat and low-health logic reach most often come first.
pub const S_DMG4: u8 = 0; // minor fracture
pub const S_DMG5: u8 = 1; // major fracture
pub const S_HLTH3: u8 = 2; // near death
pub const S_HLTH2: u8 = 3; // health critical
pub const S_MED1: u8 = 4; // automedic on
pub const S_HEAL7: u8 = 5; // morphine shot
pub const S_DMG6: u8 = 6; // blood loss
pub const S_DMG0: u8 = 7; // minor lacerations
pub const S_DMG1: u8 = 8; // major lacerations
pub const S_AMO0: u8 = 9; // ammunition depleted
pub const S_DMG7: u8 = 10; // seek medical attention
pub const S_HLTH1: u8 = 11; // health dropping
pub const S_DET1: u8 = 12; // hazardous chemicals detected
pub const S_DMG3: u8 = 13; // blood toxins detected
pub const S_DET0: u8 = 14; // biohazard detected
pub const S_DET2: u8 = 15; // radiation detected
pub const S_DMG2: u8 = 16; // internal bleeding
pub const S_A1: u8 = 17; // long jump module
/// Battery levels 1..=19 follow: `S_POWER + n` is HEV_nP.
pub const S_POWER: u8 = 17;
pub const SENTENCE_COUNT: usize = 37;

const fn s2(a: u8, b: u8) -> [u8; MAX_PARTS] {
    [a, b, NO_WORD, NO_WORD]
}

const fn power(tens: u8, five: bool) -> [u8; MAX_PARTS] {
    if five {
        [W_RESTORED, tens, W_FIVE, W_PERCENT]
    } else {
        [W_RESTORED, tens, W_PERCENT, NO_WORD]
    }
}

pub const SENTENCES: [[u8; MAX_PARTS]; SENTENCE_COUNT] = [
    s2(W_BOOPS, W_MINOR_FRACTURE),
    s2(W_BOOPS, W_MAJOR_FRACTURE),
    s2(W_BEEPS3, W_NEAR_DEATH),
    s2(W_BEEPS3, W_HEALTH_CRITICAL),
    s2(W_BOOPS, W_AUTOMEDIC),
    [W_BOOPS, W_HISS, W_MORPHINE, NO_WORD],
    s2(W_BOOPS, W_BLOOD_LOSS),
    s2(W_BOOPS, W_MINOR_LACERATIONS),
    s2(W_BOOPS, W_MAJOR_LACERATIONS),
    s2(W_BLIP, W_AMMO_DEPLETED),
    s2(W_BOOPS, W_SEEK_MEDIC),
    s2(W_BEEPS2, W_HEALTH_DROPPING),
    s2(W_BLIPS3, W_CHEMICAL),
    s2(W_BOOPS, W_BLOOD_TOXINS),
    s2(W_BLIPS3, W_BIOHAZARD),
    s2(W_BLIPS3, W_RADIATION),
    s2(W_BOOPS, W_INTERNAL_BLEEDING),
    s2(W_BLIPS2, W_POWERMOVE),
    // HEV_1P .. HEV_19P
    power(W_TEN, false),
    power(W_FIFTEEN, false),
    power(W_TWENTY, false),
    power(W_TWENTY, true),
    power(W_THIRTY, false),
    power(W_THIRTY, true),
    power(W_FOURTY, false),
    power(W_FOURTY, true),
    power(W_FIFTY, false),
    power(W_FIFTY, true),
    power(W_SIXTY, false),
    power(W_SIXTY, true),
    power(W_SEVENTY, false),
    power(W_SEVENTY, true),
    power(W_EIGHTY, false),
    power(W_EIGHTY, true),
    power(W_NINETY, false),
    power(W_NINETY, true),
    [W_LEVEL, W_ONEHUNDRED, W_PERCENT, NO_WORD],
];

/// How long (seconds) a sentence stays out of the queue after it was asked
/// for (the `SUIT_NEXT_IN_*` constants of player.h).
pub const NEXT_IN_30SEC: u16 = 30;
pub const NEXT_IN_1MIN: u16 = 60;
pub const NEXT_IN_5MIN: u16 = 300;
pub const NEXT_IN_10MIN: u16 = 600;
pub const NEXT_IN_30MIN: u16 = 1800;

/// `HEV_nP` for a battery that now holds `armor` points
/// (items.cpp CItemBattery::MyTouch): the percentage in steps of five, minus
/// one. 0 is "power below five percent", which a pickup never
/// leaves (the smallest battery is 10 points).
pub const fn battery_level(armor: u16) -> u8 {
    // MAX_NORMAL_BATTERY is 100, so the percentage is the armour value.
    let pct = armor as u32 / 5;
    let n = if pct > 0 { pct - 1 } else { 0 };
    if n > 19 {
        19
    } else {
        n as u8
    }
}

/// Sentence id of `HEV_nP`, if it is one this table speaks.
pub const fn battery_sentence(level: u8) -> Option<u8> {
    if level >= 1 && level <= 19 {
        Some(S_POWER + level)
    } else {
        None
    }
}

// ---- trigger_hurt damage kinds ----
//
// A `trigger_hurt` record keeps its damage type in bits 4..6 of the record
// flags (bits 0..3 are the Geiger, healing, armour-bypass and poison flags), so the
// suit can name what hurt the player. 0 is every type it has nothing to say
// about (generic, burn, shock, blast...).
pub const HURT_KIND_SHIFT: u8 = 4;
pub const HURT_KIND_MASK: u8 = 0x70;
pub const HURT_CRUSH: u8 = 1;
pub const HURT_FALL: u8 = 2;
pub const HURT_POISON: u8 = 3;
pub const HURT_ACID: u8 = 4;
pub const HURT_NERVEGAS: u8 = 5;

/// The record-flag bits for a map's `damagetype` (the DMG_* mask, in the order
/// the SDK's `TakeDamage` handles them). Radiation has its own flag.
pub const fn hurt_kind_bits(damage_type: u32) -> u8 {
    const DMG_CRUSH: u32 = 1 << 0;
    const DMG_FALL: u32 = 1 << 5;
    const DMG_PARALYZE: u32 = 1 << 15;
    const DMG_NERVEGAS: u32 = 1 << 16;
    const DMG_POISON: u32 = 1 << 17;
    const DMG_ACID: u32 = 1 << 20;
    let code = if damage_type & (DMG_FALL | DMG_CRUSH) != 0 {
        if damage_type & DMG_FALL != 0 {
            HURT_FALL
        } else {
            HURT_CRUSH
        }
    } else if damage_type & (DMG_POISON | DMG_PARALYZE) != 0 {
        HURT_POISON
    } else if damage_type & DMG_ACID != 0 {
        HURT_ACID
    } else if damage_type & DMG_NERVEGAS != 0 {
        HURT_NERVEGAS
    } else {
        0
    };
    code << HURT_KIND_SHIFT
}

/// Sizes of the block header and directory for `words` entries.
pub const fn directory_bytes(words: usize) -> usize {
    HEADER_BYTES + words * ENTRY_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sentence_names_real_words() {
        assert_eq!(SENTENCES.len(), SENTENCE_COUNT);
        for parts in SENTENCES {
            assert_ne!(parts[0], NO_WORD);
            let mut ended = false;
            for part in parts {
                if part == NO_WORD {
                    ended = true;
                } else {
                    assert!(!ended, "a hole inside a sentence");
                    assert!((part as usize) < WORD_COUNT);
                }
            }
        }
    }

    #[test]
    fn hurt_kinds_pack_into_the_record_flags() {
        assert_eq!(hurt_kind_bits(0), 0);
        assert_eq!(hurt_kind_bits(1 << 17) >> HURT_KIND_SHIFT, HURT_POISON);
        assert_eq!(hurt_kind_bits(1 << 5) >> HURT_KIND_SHIFT, HURT_FALL);
        assert_eq!(
            hurt_kind_bits(1 << 20 | 1 << 18) >> HURT_KIND_SHIFT,
            HURT_ACID
        );
        assert_eq!(hurt_kind_bits(1 << 16) & !HURT_KIND_MASK, 0);
    }

    #[test]
    fn battery_levels_follow_the_sdk_rounding() {
        // HEV_1P is "ten percent", HEV_19P "power level is one hundred percent".
        assert_eq!(battery_level(10), 1);
        assert_eq!(battery_level(14), 1);
        assert_eq!(battery_level(15), 2);
        assert_eq!(battery_level(55), 10);
        assert_eq!(battery_level(100), 19);
        assert_eq!(battery_level(4), 0);
        assert_eq!(battery_sentence(1), Some(S_POWER + 1));
        assert_eq!(battery_sentence(0), None);
        let p = SENTENCES[(S_POWER + 4) as usize];
        assert_eq!(p, [W_RESTORED, W_TWENTY, W_FIVE, W_PERCENT]);
        let last = SENTENCES[(S_POWER + 19) as usize];
        assert_eq!(last[0], W_LEVEL);
    }
}
