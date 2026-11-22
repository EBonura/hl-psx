//! monster_tentacle: the Blast Pit's three tentacles (c1a4i), one in c2a5x and
//! four in c4a1b.
//!
//! The retail tentacle never walks and never dies to damage. It waits wholly
//! down its pit until it hears something, then grows to one of four heights
//! (the floor and levels 1 to 3) picked by how high above its base the noise
//! was, turns toward it (no further than its sweep arc, 65 degrees either side
//! unless the map says otherwise) and strikes there. A strike swings its claw
//! through the spot; for part of the swing the claw kills (200 damage), at
//! other times a touch of the claw does 20 or 25. While the last noise is
//! fresh (five to ten seconds) it keeps striking at that noise's height and
//! bearing; after that it idles at its height, now and then tapping or
//! drifting to another level. Damage only wears it down to 1 health: then it
//! sinks back into its pit, where it heals to full.
//!
//! It hears with twice the usual sensitivity: the player's footsteps (his
//! speed), his gunfire, and the noises triggers make. Its body moves by its
//! sequences only: going up or down plays the model's transition sequences
//! (rising out of the pit, floor to level 1, level 1 to 2, 2 to 3), the
//! reversible ones backwards to come down. All of it plays half as fast again
//! while noises keep coming.
//!
//! Its sounds come from its sequences' events: the claw's bang on each strike
//! (full volume) and the light taps and scrapes of the rise, the floor idle and
//! the taps (0.3 to 0.5 volume), each the sample its `sound` keyvalue names;
//! and, when it settles at a level with nothing heard, a song at most every 10
//! to 20 seconds.
//!
//! A trigger toggling it makes it thrash (CTentacle::DieThink with the
//! Engine_Idle goal): it stops listening, climbs to level 3, swings over to the
//! engine and loops the engine sway, swat and bob turned half round, until a
//! trigger turns it off. Its engine sequences are cooked on c1a4i only, the one
//! map that toggles a tentacle.
//!
//! Everything here is arithmetic over a [`World`], so the host test crate runs
//! it; the game supplies the hearing, the random numbers, the clip lengths
//! cooked from tentacle2.mdl and the sounds.

/// The tentacle's model type.
pub const KIND: u8 = 50;
/// Concurrent tentacles: c4a1b has four.
pub const SLOTS: usize = 4;
pub const FREE: u8 = 0xFF;
/// It thinks every 0.1 s.
pub const THINK_TICKS: u32 = 2;
/// Spawn health, and what it heals to in its pit.
pub const HEALTH: u8 = 75;
/// A touch of the claw outside the killing swing, and any other part of it.
const CLAW_TAP: u8 = 25;
const BODY_TOUCH: u8 = 20;
const KILL_SWING: u8 = 200;
/// It can hurt once every half second.
const HIT_WAIT: u8 = 10;
/// `HearingSensitivity`.
pub const HEARING: i32 = 2;
/// One second of a single-frame sequence (the pit idle).
const STILL_TICKS: u16 = 20;
/// The q12 cycle a sequence runs over.
const CYCLE: i32 = 4096;
/// Playback rate of 1.0.
const RATE_ONE: i32 = 256;

/// Degrees in 12-bit heading units.
const fn deg(d: i32) -> i32 {
    d * 4096 / 360
}

/// The default sweep: 65 degrees either side of the spawn heading. The
/// cooker writes each tentacle's own limit (half its `sweeparc`, 0..=2048,
/// 2048 for no limit) into its prop's carry word, which never carries a
/// tentacle across a transition; this is that default.
pub const MAX_YAW: i16 = deg(65) as i16;

// Cooked clip slots, in the order of tentacle2's row in
// host/hl-content/model-roster.txt.
pub const PIT: u8 = 0;
/// rise_to_Temp1: out of the pit to level 1, backwards to sink.
const RISE: u8 = 1;
/// Temp1_to_Floor: level 1 down to the floor.
const TO_FLOOR: u8 = 2;
/// Engine_death1, in the death slot every actor shares.
pub const DEATH: u8 = 4;
/// Floor_to_Lev1.
const FLOOR_TO_L1: u8 = 7;
/// Lev1_to_Lev2 and Lev2_to_Lev3, both reversible.
const L1_TO_L2: u8 = 11;
const L2_TO_L3: u8 = 15;
/// Lev3_to_Engine and the engine thrash loop (Engine_Idle, Engine_Sway,
/// Engine_Swat, Engine_Bob), cooked after the shared slots on c1a4i only.
const TO_ENGINE: u8 = 19;
const ENGINE_IDLE: u8 = 20;
const ENGINE_SWAY: u8 = 21;
const ENGINE_SWAT: u8 = 22;
/// Clip slots the tables below describe.
const CLIPS: usize = 24;
const IDLE: [u8; 4] = [3, 8, 12, 16];
const STRIKE: [u8; 4] = [5, 9, 13, 17];
const TAP: [u8; 4] = [6, 10, 14, 18];
/// Each strike's killing swing in q12 of its cycle: the source frames of its
/// start and end events (3 and 4) over the sequence's frame intervals
/// (Floor_Strike 6..12 of 15, Lev1 9..20 of 30, Lev2 13..20 of 25, Lev3 6..12
/// of 20).
const KILL: [(u16, u16); 4] = [
    kill_swing(6, 12, 15),
    kill_swing(9, 20, 30),
    kill_swing(13, 20, 25),
    kill_swing(6, 12, 20),
];

const fn kill_swing(start: u32, end: u32, intervals: u32) -> (u16, u16) {
    (
        (start * 4096 / intervals) as u16,
        (end * 4096 / intervals) as u16,
    )
}
/// How high its ear is at each level (`MyHeight`).
const HEIGHT: [i32; 4] = [0, 256, 448, 640];

/// What [`World::sound`] plays: a light tap or scrape (events 2 and 6), the
/// claw's bang (event 1), its song.
pub const SOUND_TAP: u8 = 0;
pub const SOUND_BANG: u8 = 1;
pub const SOUND_SING: u8 = 2;

/// The sound events of the cooked sequences, `(bang << 15) | (slot << 8) |
/// position`, the position in 256ths of the cycle (the source frame over the
/// sequence's frame intervals; an event on the last frame sits at 255).
static EVENTS: [u16; 18] = [
    event(RISE, 23, 60, false),
    event(RISE, 30, 60, false),
    event(TO_FLOOR, 26, 35, false),
    event(IDLE[0], 0, 40, false),
    event(STRIKE[0], 8, 15, true),
    event(TAP[0], 7, 20, false),
    event(TAP[0], 13, 20, false),
    event(TAP[0], 20, 20, false),
    event(STRIKE[1], 11, 30, true),
    event(TAP[1], 7, 20, false),
    event(TAP[1], 13, 20, false),
    event(STRIKE[2], 16, 25, true),
    event(TAP[2], 7, 20, false),
    event(TAP[2], 13, 20, false),
    event(STRIKE[3], 10, 20, true),
    event(TAP[3], 7, 20, false),
    event(TAP[3], 13, 20, false),
    event(ENGINE_SWAT, 8, 20, true),
];

const fn event(slot: u8, frame: u32, intervals: u32, bang: bool) -> u16 {
    let at = frame * 256 / intervals;
    let at = if at > 255 { 255 } else { at };
    ((bang as u16) << 15) | ((slot as u16) << 8) | at as u16
}

/// Where its sounds come from: its ear at the level `seq` holds it, the top
/// for the engine sequences (the claw is up at the engine).
fn sound_height(seq: u8) -> i32 {
    if seq >= TO_ENGINE {
        return HEIGHT[3];
    }
    let level = level_of(seq);
    if level < 0 {
        0
    } else {
        HEIGHT[level as usize]
    }
}

// Nodes of the model's transition graph. tentacle2 links pit, level 1, level 2
// and level 3 in a line; the floor hangs off level 1 (Temp1_to_Floor down,
// Floor_to_Lev1 up, neither reversible).
const N_PIT: u8 = 0;
const N_L1: u8 = 1;
const N_L2: u8 = 2;
const N_L3: u8 = 3;
const N_FLOOR: u8 = 4;
/// Above level 3, swung over to the engine.
const N_ENGINE: u8 = 5;
const LEVEL_NODE: [u8; 4] = [N_FLOOR, N_L1, N_L2, N_L3];
const TRANSITIONS: [u8; 6] = [RISE, TO_FLOOR, FLOOR_TO_L1, L1_TO_L2, L2_TO_L3, TO_ENGINE];

/// The level a noise `dz` units above the base calls for (`Level`).
pub const fn level_for(dz: i32) -> u8 {
    if dz < 216 {
        0
    } else if dz < 408 {
        1
    } else if dz < 600 {
        2
    } else {
        3
    }
}

/// The level a sequence holds the tentacle at (`MyLevel`); -1 in the pit.
pub const fn level_of(seq: u8) -> i8 {
    if (seq as usize) < LEVEL_OF.len() {
        LEVEL_OF[seq as usize]
    } else {
        -1
    }
}

/// `level_of` for each cooked clip slot, looked up rather than branched.
const LEVEL_OF: [i8; CLIPS] = {
    let mut t = [0; CLIPS];
    let mut seq = 0;
    while seq < t.len() {
        t[seq] = seq_level(seq as u8);
        seq += 1;
    }
    t
};

const fn seq_level(seq: u8) -> i8 {
    match seq {
        RISE | TO_FLOOR | FLOOR_TO_L1 => 0,
        3 | 5 | 6 => 0,
        L1_TO_L2 | 8 | 9 | 10 => 1,
        L2_TO_L3 | 12 | 13 | 14 => 2,
        16 | 17 | 18 => 3,
        _ => -1,
    }
}

/// Entry and exit node of a sequence.
fn ends(seq: u8) -> (u8, u8) {
    if (seq as usize) < ENDS.len() {
        ENDS[seq as usize]
    } else {
        (N_PIT, N_PIT)
    }
}

/// `seq_ends` for each cooked clip slot.
const ENDS: [(u8, u8); CLIPS] = {
    let mut t = [(N_PIT, N_PIT); CLIPS];
    let mut seq = 0;
    while seq < t.len() {
        t[seq] = seq_ends(seq as u8);
        seq += 1;
    }
    t
};

const fn seq_ends(seq: u8) -> (u8, u8) {
    match seq {
        RISE => (N_PIT, N_L1),
        TO_FLOOR => (N_L1, N_FLOOR),
        FLOOR_TO_L1 => (N_FLOOR, N_L1),
        L1_TO_L2 => (N_L1, N_L2),
        L2_TO_L3 => (N_L2, N_L3),
        TO_ENGINE => (N_L3, N_ENGINE),
        ENGINE_IDLE..=23 => (N_ENGINE, N_ENGINE),
        _ => {
            let level = level_of(seq);
            let node = if level < 0 {
                N_PIT
            } else {
                LEVEL_NODE[level as usize]
            };
            (node, node)
        }
    }
}

const fn reversible(seq: u8) -> bool {
    matches!(seq, RISE | L1_TO_L2 | L2_TO_L3)
}

/// The next node on the way from `from` to `to`.
const fn next_node(from: u8, to: u8) -> u8 {
    if from == N_FLOOR {
        return N_L1;
    }
    // The engine hangs off level 3: climb there first.
    if to == N_ENGINE {
        return if from == N_L3 { N_ENGINE } else { from + 1 };
    }
    if to == N_FLOOR {
        return if from == N_L1 {
            N_FLOOR
        } else if from < N_L1 {
            from + 1
        } else {
            from - 1
        };
    }
    if to > from {
        from + 1
    } else {
        from - 1
    }
}

/// The sequence to play next on the way to `goal`, and its direction
/// (`FindTransition` over tentacle2's graph).
#[optimize(size)]
pub fn find_transition(seq: u8, dir: i8, goal: u8) -> (u8, i8) {
    let (entry, exit) = ends(seq);
    let at = if dir > 0 { exit } else { entry };
    let want = ends(goal).0;
    if at == want {
        return (goal, 1);
    }
    let hop = next_node(at, want);
    for t in TRANSITIONS {
        let (a, b) = ends(t);
        if a == at && b == hop {
            return (t, 1);
        }
        if reversible(t) && b == at && a == hop {
            return (t, -1);
        }
    }
    (goal, 1)
}

/// What the tentacle needs from the world.
pub trait World {
    /// Uniform integer in `lo..=hi`.
    fn rand(&mut self, lo: i32, hi: i32) -> i32;
    /// The nearest noise heard at `ear` (with [`HEARING`] applied), if any.
    fn hear(&mut self, ear: [i32; 3]) -> Option<[i32; 3]>;
    /// The 12-bit heading of a flat direction (0 = +Z, 1024 = +X).
    fn yaw_of(&mut self, dx: i32, dz: i32) -> u16;
    /// Length of a cooked sequence in ticks at the authored rate.
    fn ticks(&mut self, seq: u8) -> u16;
    /// Play [`SOUND_TAP`], [`SOUND_BANG`] or [`SOUND_SING`] `height` units
    /// above its base.
    fn sound(&mut self, what: u8, height: i32);
}

/// Signed 12-bit angle.
fn wrap(a: i32) -> i32 {
    let a = a & 0xFFF;
    if a > 2048 {
        a - 4096
    } else {
        a
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tentacle {
    /// Owning prop; [`FREE`] when the slot is unused.
    pub prop: u8,
    /// The sequence playing (a cooked clip slot) and its direction.
    pub seq: u8,
    pub dir: i8,
    goal: u8,
    /// Position in the sequence, 0..=4096.
    pub cycle: u16,
    /// Playback rate, 256 = authored speed; negative plays backwards.
    rate: i16,
    /// The random rate offset of this sequence (up to a fifth either way).
    adj: i16,
    finished: bool,
    /// Heading now, at spawn, and the one it turns toward.
    pub yaw: u16,
    home: u16,
    ideal: u16,
    max_yaw: i16,
    /// Bearing of the last noise from the spawn heading, and its level.
    sound_yaw: i16,
    sound_level: u8,
    /// Ticks the last noise keeps it striking, and since it was heard.
    sound_left: u16,
    since_sound: u16,
    prev_sound: [i32; 3],
    hit_dmg: u8,
    hit_wait: u8,
    /// Playing its death; the prop goes when it ends.
    pub dying: bool,
    /// A trigger toggled it: it thrashes at the engine and no longer listens.
    thrash: bool,
    /// Ticks until it may sing again.
    next_song: u16,
}

impl Tentacle {
    pub const EMPTY: Self = Self {
        prop: FREE,
        seq: PIT,
        dir: 1,
        goal: PIT,
        cycle: 0,
        rate: RATE_ONE as i16,
        adj: 0,
        finished: false,
        yaw: 0,
        home: 0,
        ideal: 0,
        max_yaw: MAX_YAW,
        sound_yaw: 0,
        sound_level: 0,
        sound_left: 0,
        since_sound: u16::MAX,
        prev_sound: [0; 3],
        hit_dmg: BODY_TOUCH,
        hit_wait: 0,
        dying: false,
        thrash: false,
        next_song: 0,
    };

    /// CTentacle::Spawn: down its pit, facing its authored heading, turning
    /// at most `max_yaw` either side of it (2048 turns it anywhere).
    pub fn spawn(prop: u8, yaw: u16, max_yaw: u16) -> Self {
        let mut t = Self::EMPTY;
        t.prop = prop;
        t.max_yaw = max_yaw as i16;
        t.yaw = yaw;
        t.home = yaw;
        t.ideal = yaw;
        t
    }

    /// The sequence and its position, for drawing.
    pub fn pose(&self) -> (u8, u16) {
        (self.seq, self.cycle)
    }

    /// A trigger turned it off (`USE_OFF`): it plays its death and goes.
    pub fn die(&mut self) {
        self.dying = true;
        self.seq = DEATH;
        self.dir = 1;
        self.cycle = 0;
        self.rate = RATE_ONE as i16;
    }

    /// A trigger toggled it (`USE_TOGGLE`): once the sequence playing ends it
    /// heads for the engine and thrashes there.
    pub fn thrash(&mut self) {
        self.thrash = true;
        self.goal = ENGINE_IDLE;
    }

    /// While a scripted sequence owns it, it waits in the pit afterwards.
    pub fn scripted(&mut self) {
        self.seq = PIT;
        self.goal = PIT;
        self.dir = 1;
        self.cycle = 0;
        self.yaw = self.home;
        self.ideal = self.home;
    }

    /// Damage `dmg` leaves it with this much health (`TakeDamage`): never
    /// below 1.
    pub fn take_damage(health: u8, dmg: u8) -> u8 {
        health.saturating_sub(dmg).max(1)
    }

    /// The damage of the player touching it now, with `claw` when the touch
    /// is on the claw (`HitTouch`); `None` while it waits between hits.
    pub fn touch(&mut self, claw: bool) -> Option<u8> {
        if self.hit_wait != 0 {
            return None;
        }
        self.hit_wait = HIT_WAIT;
        Some(if claw { self.hit_dmg } else { BODY_TOUCH })
    }

    /// Whether the claw is in its killing swing.
    pub fn killing(&self) -> bool {
        self.hit_dmg == KILL_SWING
    }

    /// One 20 Hz tick at `origin` (its base, y up): the sequence advances
    /// and the body turns; every [`THINK_TICKS`] it listens and, when a
    /// sequence has ended, picks the next. Returns true when its death has
    /// played out and the prop should go.
    #[optimize(size)]
    pub fn tick<W: World>(
        &mut self,
        origin: [i32; 3],
        health: &mut u8,
        think: bool,
        w: &mut W,
    ) -> bool {
        self.advance(w);
        self.hit_wait = self.hit_wait.saturating_sub(1);
        self.yaw = turn_toward(self.yaw, self.ideal, TURN_Q12);
        if self.dying {
            return self.finished;
        }
        if think {
            self.think(origin, health, w);
        }
        false
    }

    fn advance<W: World>(&mut self, w: &mut W) {
        let ticks = if self.seq == PIT {
            STILL_TICKS
        } else {
            w.ticks(self.seq).max(1)
        } as i32;
        let before = self.cycle as i32;
        let after = before + self.rate as i32 * (CYCLE / RATE_ONE) / ticks;
        if !(0..CYCLE).contains(&after) {
            self.finished = true;
        }
        self.cycle = after.clamp(0, CYCLE) as u16;
        // Events fire playing forward only, each once as the cycle passes it.
        let mut i = 0;
        while i < EVENTS.len() {
            let e = EVENTS[i];
            let at = (e & 0xFF) as i32 * 16;
            if (e >> 8) as u8 & 0x7F == self.seq && before <= at && at < after {
                w.sound((e >> 15) as u8, sound_height(self.seq));
            }
            i += 1;
        }
        // The strike's killing swing starts and ends on its events.
        let level = level_of(self.seq);
        if level >= 0 && self.seq == STRIKE[level as usize] {
            let (from, to) = KILL[level as usize];
            let crossed = |at: u16| before < at as i32 && after >= at as i32;
            if crossed(from) {
                self.hit_dmg = KILL_SWING;
            }
            if crossed(to) {
                self.hit_dmg = CLAW_TAP;
            }
        }
    }

    fn think<W: World>(&mut self, origin: [i32; 3], health: &mut u8, w: &mut W) {
        if self.thrash {
            // DieThink: deaf, it heads for the engine at 1.5 speed, then loops
            // a random engine sequence turned half round at its own speed.
            if self.finished {
                self.finished = false;
                if self.seq == self.goal {
                    self.goal = ENGINE_SWAY + w.rand(0, 2) as u8;
                }
                self.next(w);
                let engine = self.seq >= ENGINE_IDLE;
                self.ideal = (self.home + if engine { 2048 } else { 0 }) & 0xFFF;
                if !engine {
                    self.rate = (RATE_ONE * 3 / 2) as i16;
                }
            }
            return;
        }
        self.since_sound = self.since_sound.saturating_add(THINK_TICKS as u16);
        self.sound_left = self.sound_left.saturating_sub(THINK_TICKS as u16);
        self.next_song = self.next_song.saturating_sub(THINK_TICKS as u16);
        let level = level_of(self.seq);
        let height = sound_height(self.seq);
        if let Some(p) = w.hear([origin[0], origin[1] + height, origin[2]]) {
            let mut d = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
            // Two noises within half a second: aim where the source will be
            // a second on, at the speed it moved between them.
            if self.since_sound < 10 {
                let dt = self.since_sound.max(1) as i32;
                for (a, (now, then)) in d.iter_mut().zip(p.iter().zip(self.prev_sound)) {
                    *a += (now - then) * 20 / dt;
                }
            }
            self.since_sound = 0;
            self.prev_sound = p;
            self.sound_yaw = wrap(w.yaw_of(d[0], d[2]) as i32 - self.home as i32) as i16;
            self.sound_level = level_for(d[1]);
            self.sound_left = (w.rand(100, 200)) as u16;
        }
        let max = self.max_yaw as i32;
        let dy = (self.sound_yaw as i32).clamp(-max, max);
        self.ideal = ((self.home as i32 + dy) & 0xFFF) as u16;

        if self.finished {
            self.finished = false;
            let sy = (self.sound_yaw as i32).abs();
            if *health <= 1 {
                self.goal = PIT;
                if self.seq == PIT {
                    *health = HEALTH;
                }
            } else if self.sound_left > 0 {
                let lv = self.sound_level as usize;
                // Ahead strikes, to the side taps; behind it would rear,
                // a sequence not cooked: it idles at that level.
                self.goal = if sy <= max + deg(30) {
                    STRIKE[lv]
                } else if sy <= max * 2 {
                    TAP[lv]
                } else {
                    IDLE[lv]
                };
            } else if self.seq == PIT {
                self.goal = PIT;
            } else if self.seq == self.goal {
                if level >= 0 && self.next_song == 0 {
                    // Settled at a level with nothing heard: it sings, then
                    // not again for 10 to 20 seconds (its last random rate
                    // offset picks when, leaving the random stream alone).
                    w.sound(SOUND_SING, 0);
                    self.next_song = (300 + self.adj as i32 * 2) as u16;
                }
                self.goal = if level < 0 {
                    IDLE[0]
                } else if w.rand(0, 15) == 0 {
                    IDLE[w.rand(0, 3) as usize]
                } else if w.rand(0, 3) == 0 {
                    TAP[level as usize]
                } else {
                    IDLE[level as usize]
                };
                let drift = w.rand(deg(2), deg(8)) as i16;
                self.sound_yaw += if self.sound_yaw < 0 { drift } else { -drift };
            }
            self.next(w);
        }
        // Half as fast again for two seconds after a noise, then easing back
        // to the authored speed by the fifth.
        let dir = self.dir as i32;
        let since = self.since_sound as i32;
        if since < 40 {
            self.rate = (dir * RATE_ONE * 3 / 2) as i16 + self.adj;
        } else if since < 100 {
            self.rate = (dir * (RATE_ONE + (100 - since) * RATE_ONE / 40)) as i16 + self.adj;
        }
    }

    /// Start the next sequence on the way to the goal, at the authored speed
    /// give or take a fifth.
    #[optimize(size)]
    fn next<W: World>(&mut self, w: &mut W) {
        let (seq, dir) = find_transition(self.seq, self.dir, self.goal);
        self.seq = seq;
        self.dir = dir;
        self.cycle = if dir > 0 { 0 } else { CYCLE as u16 };
        self.adj = w.rand(-RATE_ONE / 5, RATE_ONE / 5) as i16;
        self.rate = (dir as i32 * RATE_ONE) as i16 + self.adj;
    }
}

/// Its turn per tick: yaw speed 18, as `senses::turn_rate_q12` reads it.
const TURN_Q12: u16 = 20;

/// Rotate `cur` toward `target` by at most `rate`, the short way.
fn turn_toward(cur: u16, target: u16, rate: u16) -> u16 {
    let step = wrap(target as i32 - cur as i32).clamp(-(rate as i32), rate as i32);
    ((cur as i32 + step) & 0xFFF) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ears {
        noise: Option<[i32; 3]>,
        heard: u32,
        /// (what, height) of every sound, in order.
        sounds: Vec<(u8, i32)>,
    }

    fn ears(noise: Option<[i32; 3]>) -> Ears {
        Ears {
            noise,
            heard: 0,
            sounds: Vec::new(),
        }
    }

    impl World for Ears {
        fn rand(&mut self, lo: i32, _hi: i32) -> i32 {
            lo
        }
        fn hear(&mut self, _ear: [i32; 3]) -> Option<[i32; 3]> {
            if self.noise.is_some() {
                self.heard += 1;
            }
            self.noise
        }
        fn yaw_of(&mut self, dx: i32, dz: i32) -> u16 {
            // four quadrants are enough here
            if dz.abs() >= dx.abs() {
                if dz >= 0 {
                    0
                } else {
                    2048
                }
            } else if dx >= 0 {
                1024
            } else {
                3072
            }
        }
        fn ticks(&mut self, _seq: u8) -> u16 {
            20
        }
        fn sound(&mut self, what: u8, height: i32) {
            self.sounds.push((what, height));
        }
    }

    fn run(t: &mut Tentacle, w: &mut Ears, ticks: u32, health: &mut u8) -> Vec<u8> {
        let mut seen = Vec::new();
        for k in 0..ticks {
            t.tick([0, 0, 0], health, k % THINK_TICKS == 0, w);
            if seen.last() != Some(&t.seq) {
                seen.push(t.seq);
            }
        }
        seen
    }

    #[test]
    fn levels_follow_the_height_of_the_noise() {
        assert_eq!(level_for(0), 0);
        assert_eq!(level_for(215), 0);
        assert_eq!(level_for(216), 1);
        assert_eq!(level_for(407), 1);
        assert_eq!(level_for(408), 2);
        assert_eq!(level_for(599), 2);
        assert_eq!(level_for(600), 3);
    }

    #[test]
    fn transitions_walk_the_model_graph() {
        // out of the pit to level 3: rise, 1 to 2, 2 to 3, then the strike
        assert_eq!(find_transition(PIT, 1, STRIKE[3]), (RISE, 1));
        assert_eq!(find_transition(RISE, 1, STRIKE[3]), (L1_TO_L2, 1));
        assert_eq!(find_transition(L1_TO_L2, 1, STRIKE[3]), (L2_TO_L3, 1));
        assert_eq!(find_transition(L2_TO_L3, 1, STRIKE[3]), (STRIKE[3], 1));
        // to the floor goes through level 1
        assert_eq!(find_transition(PIT, 1, STRIKE[0]), (RISE, 1));
        assert_eq!(find_transition(RISE, 1, STRIKE[0]), (TO_FLOOR, 1));
        assert_eq!(find_transition(IDLE[3], 1, IDLE[0]), (L2_TO_L3, -1));
        assert_eq!(find_transition(L2_TO_L3, -1, IDLE[0]), (L1_TO_L2, -1));
        assert_eq!(find_transition(L1_TO_L2, -1, IDLE[0]), (TO_FLOOR, 1));
        // from the floor back into the pit: up to level 1, then sink
        assert_eq!(find_transition(IDLE[0], 1, PIT), (FLOOR_TO_L1, 1));
        assert_eq!(find_transition(FLOOR_TO_L1, 1, PIT), (RISE, -1));
        assert_eq!(find_transition(RISE, -1, PIT), (PIT, 1));
    }

    #[test]
    fn it_stays_in_the_pit_until_it_hears() {
        let mut w = ears(None);
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        assert_eq!(run(&mut t, &mut w, 400, &mut hp), vec![PIT]);
    }

    #[test]
    fn a_noise_up_high_raises_it_to_strike_there() {
        let mut w = ears(Some([0, 700, 300]));
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        let seen = run(&mut t, &mut w, 300, &mut hp);
        assert_eq!(&seen[..5], &[PIT, RISE, L1_TO_L2, L2_TO_L3, STRIKE[3]]);
        assert!(w.heard > 0);
    }

    #[test]
    fn the_strike_kills_only_in_its_swing() {
        let mut w = ears(Some([0, 0, 300]));
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        let mut killing = false;
        for k in 0..400 {
            t.tick([0, 0, 0], &mut hp, k % 2 == 0, &mut w);
            if t.seq == STRIKE[0] && t.killing() {
                killing = true;
                assert!(t.cycle >= KILL[0].0 && t.cycle < KILL[0].1);
            }
        }
        assert!(killing);
        assert_eq!(t.touch(true), Some(t.hit_dmg));
        assert_eq!(t.touch(true), None);
    }

    #[test]
    fn worn_down_it_sinks_and_heals_in_the_pit() {
        let mut w = ears(Some([0, 300, 300]));
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        run(&mut t, &mut w, 200, &mut hp);
        assert!(level_of(t.seq) >= 0);
        hp = Tentacle::take_damage(hp, 250);
        assert_eq!(hp, 1);
        w.noise = None;
        let seen = run(&mut t, &mut w, 600, &mut hp);
        assert!(seen.contains(&PIT));
        assert_eq!(hp, HEALTH);
    }

    #[test]
    fn a_noise_behind_is_not_struck_at() {
        let mut w = ears(Some([0, 0, -300]));
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        let seen = run(&mut t, &mut w, 400, &mut hp);
        assert!(!seen.iter().any(|s| STRIKE.contains(s) || TAP.contains(s)));
        assert!(seen.contains(&IDLE[0]));
    }

    #[test]
    fn the_engine_hangs_off_level_three() {
        assert_eq!(find_transition(PIT, 1, ENGINE_IDLE), (RISE, 1));
        assert_eq!(find_transition(IDLE[0], 1, ENGINE_IDLE), (FLOOR_TO_L1, 1));
        assert_eq!(find_transition(RISE, 1, ENGINE_IDLE), (L1_TO_L2, 1));
        assert_eq!(find_transition(L2_TO_L3, 1, ENGINE_IDLE), (TO_ENGINE, 1));
        assert_eq!(find_transition(TO_ENGINE, 1, ENGINE_IDLE), (ENGINE_IDLE, 1));
        assert_eq!(
            find_transition(ENGINE_IDLE, 1, ENGINE_SWAT),
            (ENGINE_SWAT, 1)
        );
    }

    #[test]
    fn a_strike_bangs_and_the_rise_taps() {
        let mut w = ears(Some([0, 0, 300]));
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        run(&mut t, &mut w, 400, &mut hp);
        // two light taps on the way out of the pit (MyLevel 0 while rising)
        assert_eq!(&w.sounds[..2], &[(SOUND_TAP, 0), (SOUND_TAP, 0)]);
        assert!(w.sounds.contains(&(SOUND_BANG, 0)));
        assert!(!w.sounds.iter().any(|s| s.0 == SOUND_SING));
    }

    #[test]
    fn down_its_pit_it_is_silent() {
        let mut w = ears(None);
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        run(&mut t, &mut w, 400, &mut hp);
        assert!(w.sounds.is_empty());
    }

    #[test]
    fn settled_with_nothing_heard_it_sings_now_and_then() {
        let mut w = ears(Some([0, 300, 300]));
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        run(&mut t, &mut w, 100, &mut hp);
        w.noise = None;
        w.sounds.clear();
        let mut sang = Vec::new();
        for k in 0..2000u32 {
            t.tick([0, 0, 0], &mut hp, k % THINK_TICKS == 0, &mut w);
            if w.sounds.last() == Some(&(SOUND_SING, 0)) {
                w.sounds.clear();
                sang.push(k);
            }
        }
        assert!(sang.len() >= 2);
        // 10 to 20 seconds apart (the test's random offset is the lowest)
        assert!(sang.windows(2).all(|p| p[1] - p[0] >= 198));
    }

    #[test]
    fn toggled_it_thrashes_at_the_engine_and_stops_listening() {
        let mut w = ears(Some([0, 300, 300]));
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let mut hp = HEALTH;
        run(&mut t, &mut w, 100, &mut hp);
        assert_eq!(level_of(t.seq), 1);
        t.thrash();
        let heard = w.heard;
        let seen = run(&mut t, &mut w, 600, &mut hp);
        assert_eq!(w.heard, heard);
        let to = seen.iter().position(|&s| s == TO_ENGINE).unwrap();
        assert_eq!(&seen[to..to + 3], &[TO_ENGINE, ENGINE_IDLE, ENGINE_SWAY]);
        assert!(!seen[to..].iter().any(|s| STRIKE.contains(s)));
        // turned half round to the engine
        assert_eq!(t.yaw, 2048);
        assert!(t.seq >= ENGINE_SWAY);
    }
    #[test]
    fn a_full_sweep_strikes_behind_it() {
        // sweeparc 360 (c2a5x): no limit, so a noise behind is struck at and
        // the body swings round to it.
        let mut w = ears(Some([0, 0, -300]));
        let mut t = Tentacle::spawn(0, 0, deg(180) as u16);
        let mut hp = HEALTH;
        let seen = run(&mut t, &mut w, 400, &mut hp);
        assert!(seen.contains(&STRIKE[0]));
        assert_eq!(t.yaw, 2048);
    }

    #[test]
    fn a_narrow_sweep_holds_it_to_its_arc() {
        // sweeparc 50 (one of c4a1b's): a noise 90 degrees off is beyond
        // 25 + 30 and twice 25 degrees, so it would rear (it idles), turned
        // no further than 25.
        let max = deg(25) as u16;
        let mut w = ears(Some([300, 0, 0]));
        let mut t = Tentacle::spawn(0, 0, max);
        let mut hp = HEALTH;
        let seen = run(&mut t, &mut w, 400, &mut hp);
        assert!(!seen.iter().any(|s| STRIKE.contains(s) || TAP.contains(s)));
        assert!(seen.contains(&IDLE[0]));
        assert_eq!(t.yaw, max);
        // the default arc strikes the same noise: 90 is within 65 + 30
        let mut w = ears(Some([300, 0, 0]));
        let mut t = Tentacle::spawn(0, 0, MAX_YAW as u16);
        let seen = run(&mut t, &mut w, 400, &mut hp);
        assert!(seen.contains(&STRIKE[0]));
        assert_eq!(t.yaw, MAX_YAW as u16);
    }
}
