//! The HEV suit voice block of each map's voice chunk (see
//! `hl_format::hev` for the layout and the sentence table).
//!
//! The words are cooked once, at the lowest rate that still keeps the energy
//! of the recording (the fvox clips are band-limited radio voice), and every
//! map takes the sentences its entities can make the game ask for, in
//! priority order, until the block reaches its byte cap. The block's SPU RAM
//! comes off the map's dialogue budget before the rate allocator runs, and the
//! resident core no longer carries the three suit lines this replaces.

use crate::audio::{
    concat_sources, cook_psau, level_kept_db, CORE_LEVEL_LIMIT_DB, HEV_RATE_FLOOR, HEV_RATE_TOP,
};
use crate::generators::Entity;
use crate::Result;
use hl_format::hev::{self, NO_WORD, PARK_BYTES, SENTENCES, SENTENCE_COUNT, WORDS, WORD_COUNT};
use std::collections::HashSet;
use std::path::Path;

/// SPU RAM one map's suit block may take. The resident core gave back about
/// the same when it dropped the suit's own bell, "health critical" and "near
/// death" clips, so a map's dialogue budget is left as the previous cooker had
/// it.
pub const BLOCK_CAP: u32 = 44 * 1024;

pub struct WordCook {
    pub rate: u32,
    pub blob: Vec<u8>,
    /// SPU RAM the uploaded word takes, parking block included.
    pub spu: u32,
}

pub struct Cooked {
    pub words: Vec<Option<WordCook>>,
}

/// SPU bytes of a cooked PSAU: its ADPCM rounded to a block, plus the parking
/// block. The 32 byte header is not uploaded.
fn spu_bytes(blob: &[u8]) -> u32 {
    let blocks = u32::from_le_bytes(blob[24..28].try_into().expect("PSAU block count"));
    blocks * 16 + PARK_BYTES
}

impl Cooked {
    /// Cook every word of the table once. A word whose recordings are missing
    /// from this install is absent, and so is every sentence that needs it.
    pub fn new(valve: &Path) -> Result<Self> {
        let mut words = Vec::with_capacity(WORD_COUNT);
        for wavs in WORDS {
            let wavs: Vec<String> = wavs.iter().map(|w| w.to_string()).collect();
            let Some(source) = concat_sources(valve, &wavs)? else {
                words.push(None);
                continue;
            };
            // Lowest rate that keeps CORE_LEVEL_LIMIT_DB of the energy.
            let rate = [
                HEV_RATE_TOP,
                7_000,
                6_000,
                5_500,
                5_000,
                4_500,
                4_000,
                3_600,
                3_200,
                2_800,
                HEV_RATE_FLOOR,
            ]
            .into_iter()
            .filter(|&r| r <= HEV_RATE_TOP.min(source.wav.rate) && r >= HEV_RATE_FLOOR)
            .rev()
            .find(|&r| level_kept_db(&source, r) >= CORE_LEVEL_LIMIT_DB)
            .unwrap_or(HEV_RATE_TOP.min(source.wav.rate));
            let blob = cook_psau(&source, rate, false);
            let spu = spu_bytes(&blob);
            words.push(Some(WordCook { rate, blob, spu }));
        }
        Ok(Self { words })
    }

    fn sentence_ready(&self, id: u8) -> bool {
        SENTENCES[id as usize]
            .iter()
            .filter(|&&w| w != NO_WORD)
            .all(|&w| self.words[w as usize].is_some())
    }

    /// Words the sentences need, each once.
    fn word_set(&self, sentences: &[u8]) -> Vec<bool> {
        let mut used = vec![false; WORD_COUNT];
        for &s in sentences {
            for &w in SENTENCES[s as usize].iter().filter(|&&w| w != NO_WORD) {
                used[w as usize] = true;
            }
        }
        used
    }

    pub fn spu_cost(&self, sentences: &[u8]) -> u32 {
        self.word_set(sentences)
            .iter()
            .zip(&self.words)
            .map(|(used, word)| match (used, word) {
                (true, Some(w)) => w.spu,
                _ => 0,
            })
            .sum()
    }

    /// The prefix of `wanted` (priority order) that fits `cap` SPU bytes, with
    /// sentences whose words are missing skipped.
    pub fn fit(&self, wanted: &[u8], cap: u32) -> Vec<u8> {
        let mut kept: Vec<u8> = Vec::new();
        for &s in wanted {
            if !self.sentence_ready(s) {
                continue;
            }
            kept.push(s);
            if self.spu_cost(&kept) > cap {
                kept.pop();
            }
        }
        kept
    }

    /// The block for `sentences`: header, directory, words. Returns the bytes
    /// and the SPU RAM they take.
    pub fn block(&self, sentences: &[u8]) -> (Vec<u8>, u32) {
        let used = self.word_set(sentences);
        let mut spu = 0u32;
        let mut offset = hev::directory_bytes(WORD_COUNT);
        let mut directory = Vec::with_capacity(WORD_COUNT * hev::ENTRY_BYTES);
        let mut payload = Vec::new();
        for (index, word) in self.words.iter().enumerate() {
            match (used[index], word) {
                (true, Some(w)) => {
                    directory.extend_from_slice(&(offset as u32).to_le_bytes());
                    directory.extend_from_slice(&(w.blob.len() as u32).to_le_bytes());
                    payload.extend_from_slice(&w.blob);
                    offset += w.blob.len();
                    spu += w.spu;
                }
                _ => directory.extend_from_slice(&[0; hev::ENTRY_BYTES]),
            }
        }
        let mut block = Vec::with_capacity(offset);
        block.extend_from_slice(&hev::MAGIC);
        block.extend_from_slice(&(WORD_COUNT as u16).to_le_bytes());
        block.extend_from_slice(&0u16.to_le_bytes());
        block.extend_from_slice(&spu.to_le_bytes());
        block.extend_from_slice(&directory);
        block.extend_from_slice(&payload);
        (block, spu)
    }
}

const BULLET: &[&str] = &[
    "monster_human_grunt",
    "monster_human_assassin",
    "monster_turret",
    "monster_miniturret",
    "monster_sentry",
    "monster_apache",
    "monster_osprey",
    "func_tank",
    "monster_hornet",
];
const SLASH: &[&str] = &[
    "monster_headcrab",
    "monster_babycrab",
    "monster_zombie",
    "monster_bullchicken",
    "monster_alien_slave",
    "monster_alien_grunt",
    "monster_snark",
    "monster_leech",
    "monster_ichthyosaur",
    "monster_bigmomma",
    "monster_gargantua",
    "monster_rat",
];
const ACID: &[&str] = &["monster_bullchicken"];
const SONIC: &[&str] = &["monster_houndeye"];

const DMG_PARALYZE: u32 = 1 << 15;
const DMG_NERVEGAS: u32 = 1 << 16;
const DMG_POISON: u32 = 1 << 17;
const DMG_RADIATION: u32 = 1 << 18;
const DMG_ACID: u32 = 1 << 20;

/// Maps before the suit exists: the tram ride and the first Black Mesa map.
pub fn map_has_suit_voice(map_name: &str) -> bool {
    !(map_name.starts_with("c0a0") || map_name == "c1a0")
}

/// The sentences a map's entities can make the suit speak, in priority order.
pub fn wanted(map_name: &str, entities: &[Entity], classes: &HashSet<String>) -> Vec<u8> {
    if !map_has_suit_voice(map_name) {
        return Vec::new();
    }
    let has = |list: &[&str]| list.iter().any(|class| classes.contains(*class));
    let mut hurt_bits = 0u32;
    let mut battery = false;
    let mut long_jump = false;
    for entity in entities {
        let class = entity.get("classname").map(String::as_str).unwrap_or("");
        match class {
            "trigger_hurt" => {
                hurt_bits |= entity
                    .get("damagetype")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0) as u32
            }
            "item_battery" => battery = true,
            "item_longjump" => long_jump = true,
            "world_items" => match entity.get("type").map(String::as_str) {
                Some("44") => battery = true,
                _ => {}
            },
            _ => {}
        }
    }
    let acid = has(ACID) || hurt_bits & DMG_ACID != 0;
    let poison = hurt_bits & (DMG_POISON | DMG_PARALYZE) != 0;
    // What this map can hurt the player with comes first (a bullet map has
    // no use for "internal bleeding"), then the lines every map can need.
    // The two health warnings stand where the resident core used to keep them.
    let mut out = vec![hev::S_HLTH3, hev::S_HLTH2];
    if has(BULLET) {
        out.push(hev::S_DMG6);
    }
    if has(SLASH) {
        out.push(hev::S_DMG0);
    }
    if acid {
        out.push(hev::S_DET1);
    }
    if poison {
        out.push(hev::S_DMG3);
    }
    if hurt_bits & DMG_NERVEGAS != 0 {
        out.push(hev::S_DET0);
    }
    if hurt_bits & DMG_RADIATION != 0 {
        out.push(hev::S_DET2);
    }
    if has(SONIC) {
        out.push(hev::S_DMG2);
    }
    out.extend([hev::S_DMG4, hev::S_MED1, hev::S_HEAL7, hev::S_AMO0]);
    if battery {
        // A battery adds 15, so the levels it can leave behind are multiples
        // of 15 first; the rest follow.
        for level in [
            2u8, 5, 8, 11, 14, 17, 3, 4, 6, 7, 9, 10, 12, 13, 15, 16, 18, 1, 19,
        ] {
            out.push(hev::S_POWER + level);
        }
    }
    if has(SLASH) {
        out.push(hev::S_DMG1);
    }
    out.extend([hev::S_DMG5, hev::S_DMG7, hev::S_HLTH1]);
    if long_jump {
        out.push(hev::S_A1);
    }
    debug_assert!(out.iter().all(|&s| (s as usize) < SENTENCE_COUNT));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn early_maps_have_no_suit_voice() {
        assert!(!map_has_suit_voice("c0a0c"));
        assert!(!map_has_suit_voice("c1a0"));
        assert!(map_has_suit_voice("c1a0d"));
        assert!(map_has_suit_voice("t0a0"));
        assert!(wanted("c0a0", &[], &classes(&[])).is_empty());
    }

    #[test]
    fn a_map_asks_only_for_what_it_can_cause() {
        let hurt = Entity::from([
            ("classname".to_string(), "trigger_hurt".to_string()),
            ("damagetype".to_string(), (1u32 << 18).to_string()),
        ]);
        let battery = Entity::from([("classname".to_string(), "item_battery".to_string())]);
        let w = wanted("c1a1", &[hurt, battery], &classes(&["monster_zombie"]));
        assert!(w.contains(&hev::S_DMG4) && w.contains(&hev::S_AMO0));
        assert!(w.contains(&hev::S_DMG0) && w.contains(&hev::S_DMG1));
        assert!(w.contains(&hev::S_DET2));
        assert!(!w.contains(&hev::S_DMG6) && !w.contains(&hev::S_DET1));
        assert!(w.contains(&(hev::S_POWER + 19)));
        assert!(!w.contains(&hev::S_A1));
    }
}

/// Print what each word costs and what every sentence of the table needs, for
/// choosing the block cap.
pub fn report(valve: &Path) -> Result<()> {
    let cooked = Cooked::new(valve)?;
    for (index, word) in cooked.words.iter().enumerate() {
        match word {
            Some(w) => println!(
                "word {index:2} {:>5} Hz {:>6} SPU bytes  {}",
                w.rate, w.spu, WORDS[index][0]
            ),
            None => println!("word {index:2} missing {}", WORDS[index][0]),
        }
    }
    let all: Vec<u8> = (0..SENTENCE_COUNT as u8).collect();
    println!(
        "whole table: {} SPU bytes; cap {}",
        cooked.spu_cost(&all),
        BLOCK_CAP
    );
    let mut running = Vec::new();
    for id in 0..SENTENCE_COUNT as u8 {
        running.push(id);
        println!(
            "through sentence {id:2}: {} SPU bytes",
            cooked.spu_cost(&running)
        );
    }
    Ok(())
}

/// Scratch: how much of each word is speech (20 ms RMS above 12% of the
/// loudest window), as first and last active second.
pub fn activity(valve: &Path) -> Result<()> {
    for (index, wavs) in WORDS.iter().enumerate() {
        let wavs: Vec<String> = wavs.iter().map(|w| w.to_string()).collect();
        let Some(source) = concat_sources(valve, &wavs)? else {
            continue;
        };
        let rate = source.wav.rate as f64;
        let win = (rate * 0.02) as usize;
        let rms: Vec<f64> = source
            .wav
            .samples
            .chunks(win.max(1))
            .map(|c| {
                (c.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / c.len() as f64).sqrt()
            })
            .collect();
        let peak = rms.iter().cloned().fold(0.0, f64::max);
        let active: Vec<usize> = rms
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > peak * 0.12)
            .map(|(i, _)| i)
            .collect();
        let (first, last) = (
            active.first().copied().unwrap_or(0),
            active.last().copied().unwrap_or(0),
        );
        println!(
            "word {index:2} total {:.2}s speech {:.2}..{:.2}s ({:.0}% kept)  {}",
            source.wav.samples.len() as f64 / rate,
            first as f64 * 0.02,
            (last + 1) as f64 * 0.02,
            100.0 * (last + 1 - first) as f64 * 0.02 / (source.wav.samples.len() as f64 / rate),
            WORDS[index][0]
        );
    }
    Ok(())
}
