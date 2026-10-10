//! Host-side reader for Half-Life's `skill.cfg`.
//!
//! Not part of the `no_std` crate: the game's build script and the host parity
//! test each include this file by path (`#[path = ".../skill_cfg.rs"]`), so the
//! cooked table and the check that audits it parse the file with one routine.

use std::collections::HashMap;

/// Parse `sk_<name><1..3> "value"` lines into `(cvar, [easy, medium, hard])`.
/// `//` starts a comment. A cvar missing a level keeps NaN there.
pub fn parse_cfg(text: &str) -> HashMap<String, [f64; 3]> {
    let mut out: HashMap<String, [f64; 3]> = HashMap::new();
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        let mut fields = line.split_whitespace();
        let (Some(key), Some(value)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some(level) = key.chars().last().and_then(|c| c.to_digit(10)) else {
            continue;
        };
        if !(1..=3).contains(&level) || !key.starts_with("sk_") {
            continue;
        }
        let Ok(value) = value.trim_matches('"').parse::<f64>() else {
            continue;
        };
        out.entry(key[..key.len() - 1].to_string())
            .or_insert([f64::NAN; 3])[level as usize - 1] = value;
    }
    out
}

/// The cooked value of one cvar at one level (0 Easy .. 2 Hard):
/// `floor(value * milli / 1000)` clamped into `u16`. `None` when the cfg lacks
/// the cvar or the level.
pub fn cooked(cfg: &HashMap<String, [f64; 3]>, key: &str, milli: u32, level: usize) -> Option<u16> {
    let v = cfg.get(key)?[level];
    v.is_finite().then(|| {
        (v * milli as f64 / 1000.0 + 1e-9)
            .floor()
            .clamp(0.0, u16::MAX as f64) as u16
    })
}
