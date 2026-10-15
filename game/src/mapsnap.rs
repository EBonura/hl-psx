//! A map's progress in a few kilobytes: which monsters are dead and where the
//! living ones stand, every door, button, trigger and breakable, the pending
//! delayed events, and the removed sprites. A manual save writes it to the
//! card beside the player's checkpoint; dying reloads the latest one from RAM
//! (retail reloads the most recent save). Loading re-spawns the map exactly as
//! a level entry does and then lays this over it, so everything the capture
//! does not name (trains, pushables, scripted sequences in flight, boss
//! schedules) starts over, the same as when a level is entered.
//!
//! Deadlines are absolute simulation ticks, so the restored map also resumes
//! the tick count the capture was taken at (`tick`).

use crate::*;

/// Capture size limit. A map that needs more is not captured and a death
/// restarts it from its entry.
pub const CAPACITY: usize = 1024;

const VERSION: u8 = 3;
const OCC_STATE: u8 = PROP_SCI_FOLLOWING
    | PROP_SCI_PROVOKED
    | PROP_SCI_PREDISASTER
    | PROP_SCRIPT_NOINTERRUPT
    | PROP_SCI_HELLO_SAID;

static mut BUF: [u8; CAPACITY] = [0; CAPACITY];
static mut LEN: usize = 0;
static mut TICK: u16 = 0;
static mut ROOM: u16 = u16::MAX;
/// The checkpoint (player, arsenal, globals) the capture was taken with.
static mut CHECKPOINT: save::Checkpoint = save::Checkpoint::EMPTY;
/// A restore waits for the next map start.
static mut ARMED: bool = false;
/// The running map's dimensions and tick, refreshed every simulation tick so a
/// capture can run from the pause menu, which has no map in hand.
static mut DIMS: (u16, u16, u16) = (0, 0, 0);

#[inline(always)]
pub unsafe fn note_dims(nlogic: usize, nents: usize, tick: u16) {
    DIMS = (nlogic as u16, nents as u16, tick);
}

#[inline(always)]
unsafe fn buf() -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(BUF).cast::<u8>(), CAPACITY)
}

struct Out<'a> {
    buf: &'a mut [u8],
    at: usize,
    over: bool,
}

impl Out<'_> {
    #[inline(never)]
    fn u8(&mut self, v: u8) {
        match self.buf.get_mut(self.at) {
            Some(b) => *b = v,
            None => self.over = true,
        }
        self.at += 1;
    }
    #[inline(never)]
    fn u16(&mut self, v: u16) {
        self.u8(v as u8);
        self.u8((v >> 8) as u8);
    }
}

struct In<'a> {
    buf: &'a [u8],
    at: usize,
}

impl In<'_> {
    #[inline(never)]
    fn u8(&mut self) -> u8 {
        let v = self.buf.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        v
    }
    #[inline(never)]
    fn u16(&mut self) -> u16 {
        u16::from(self.u8()) | (u16::from(self.u8()) << 8)
    }
}

/// Logic kinds whose state means nothing to a restore: static records, sound
/// and music, and the machinery that restarts with the level.
#[inline(always)]
fn skipped(kind: u8) -> bool {
    matches!(
        kind,
        map::LOGIC_MAP_FLAGS
            | map::LOGIC_MAP_SOUND
            | map::LOGIC_AMBIENT
            | map::LOGIC_CDTRACK
            | map::LOGIC_ENV_SOUND
            | map::LOGIC_SCRIPTED
            | map::LOGIC_SCRIPTED_HAS_IDLE
            | map::LOGIC_SCRIPTED_HAS_PLAY
            | map::LOGIC_MONSTER_TRIGGER
            | map::LOGIC_BIGMOMMA_NODE
            | map::LOGIC_FUNC_TRAIN
            | map::LOGIC_FUNC_TRACKTRAIN
            | map::LOGIC_MONSTERCLIP
            | map::LOGIC_INFO_LANDMARK
            | map::LOGIC_TRIGGER_CHANGELEVEL
            | map::LOGIC_TRIGGER_TRANSITION
            | map::LOGIC_GLOBAL_PUSHABLE
    )
}

/// Kinds that also keep a counter: what is left to make or give, remaining
/// triggers, and a breakable's health (the second word).
#[inline(always)]
fn counted(kind: u8) -> bool {
    matches!(
        kind,
        map::LOGIC_TRIGGER_COUNTER
            | map::LOGIC_MONSTERMAKER
            | map::LOGIC_HEALTH_CHARGER
            | map::LOGIC_HEV_CHARGER
            | map::LOGIC_FUNC_BREAKABLE
            | map::LOGIC_FUNC_GUNTARGET
    )
}

#[inline(always)]
fn has_hp(kind: u8) -> bool {
    kind == map::LOGIC_FUNC_BREAKABLE || kind == map::LOGIC_FUNC_GUNTARGET
}

/// Whether a brush's phase is a plain 0..4096 slide the capture can hold.
#[inline(always)]
unsafe fn phase_kept(ei: usize) -> bool {
    if ENT_CACHE[ei].kind == ENT_KIND_PUSHABLE {
        return false;
    }
    let li = ENT_BRUSH_LOGIC[ei] as usize;
    li < MAX_LOGIC
        && matches!(
            LOGIC_KIND[li],
            map::LOGIC_FUNC_DOOR | map::LOGIC_FUNC_BUTTON
        )
}

/// Capture the running map. `None` when it does not fit.
#[inline(never)]
#[optimize(size)]
pub unsafe fn capture(nlogic: usize, nents: usize, tick: u16) -> Option<usize> {
    let mut o = Out {
        buf: buf(),
        at: 0,
        over: false,
    };
    o.u8(VERSION);
    let nprops = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
    o.u8(nprops as u8);
    let mut pi = 0;
    while pi < nprops {
        let alive = PROP_HEALTH[pi] != 0;
        let active = PROP_ACTIVE[pi] != 0;
        let flags = (active as u8) | ((alive as u8) << 1) | (PROP_OCC_VIS[pi] & OCC_STATE);
        o.u8(flags);
        if active {
            o.u8(PROP_DORMANT[pi]);
            o.u8(PROP_MAKER[pi]);
            o.u8(PROP_HEALTH[pi]);
            // Only a follower leaves where it spawned and matters if it is moved.
            if alive && flags & PROP_SCI_FOLLOWING != 0 {
                let p = PROP_POS[pi];
                o.u16(p[0] as u16);
                o.u16(p[1] as u16);
                o.u16(p[2] as u16);
                o.u16(PROP_YAW[pi]);
            }
        }
        pi += 1;
    }
    // Logic states, zeros run-length coded (most records sit at their
    // starting state), then the few counters.
    let mut zeros = 0u8;
    let mut li = 0;
    while li < nlogic {
        if !skipped(LOGIC_KIND[li]) {
            let state = LOGIC_STATE[li];
            if state == 0 && zeros < 127 {
                zeros += 1;
            } else {
                if zeros != 0 {
                    o.u8(0x80 | zeros);
                    zeros = 0;
                }
                if state == 0 {
                    zeros = 1;
                } else {
                    o.u8(state);
                }
            }
        }
        li += 1;
    }
    if zeros != 0 {
        o.u8(0x80 | zeros);
    }
    li = 0;
    while li < nlogic {
        let kind = LOGIC_KIND[li];
        if counted(kind) {
            o.u16(LOGIC_COUNTER[li] as u16);
            if has_hp(kind) {
                o.u16(LOGIC_BREAK_HP[li]);
            }
        }
        li += 1;
    }
    let mut bits = 0u8;
    let mut ei = 0;
    while ei < nents {
        bits |= ((ENT_ACTIVE[ei] != 0) as u8) << (ei & 7);
        if ei & 7 == 7 || ei + 1 == nents {
            o.u8(bits);
            bits = 0;
        }
        ei += 1;
    }
    ei = 0;
    while ei < nents {
        if phase_kept(ei) {
            o.u16(ENT_PHASE[ei] as u16);
        }
        ei += 1;
    }
    let mut w = 0;
    while w < SPRITE_STATE_WORDS {
        o.u16(SPRITE_REMOVED[w] as u16);
        o.u16((SPRITE_REMOVED[w] >> 16) as u16);
        w += 1;
    }
    o.u16(LIGHTSTYLE_ACTIVE_MASK as u16);
    o.u16((LIGHTSTYLE_ACTIVE_MASK >> 16) as u16);
    if o.over {
        return None;
    }
    LEN = o.at;
    TICK = tick;
    Some(o.at)
}

/// Capture the map as it stands and keep it, with the player's checkpoint, as
/// the map's latest save point (a manual save). False when it does not fit.
pub unsafe fn keep_now(cp: &save::Checkpoint) -> bool {
    let (nlogic, nents, tick) = DIMS;
    if capture(nlogic as usize, nents as usize, tick).is_some() {
        ROOM = cp.room_id;
        CHECKPOINT = *cp;
        true
    } else {
        ROOM = u16::MAX;
        LEN = 0;
        false
    }
}

/// A new map: nothing of another map may restore into it.
pub unsafe fn forget_if_other_room(room: u16) {
    if ROOM != room {
        ROOM = u16::MAX;
        ARMED = false;
    }
}

/// A fresh game or chapter jump: no save point of an earlier session applies.
pub unsafe fn invalidate() {
    ROOM = u16::MAX;
    ARMED = false;
}

/// The player is dead: if this map has a save point, restore it on the next
/// start instead of the level entry. True when armed.
pub unsafe fn arm_for_death(room: u16) -> bool {
    if ROOM != room || LEN == 0 {
        return false;
    }
    ARMED = true;
    true
}

/// A save was read from the card: hold its payload for the map start.
pub unsafe fn arm_from_card(room: u16, cp: save::Checkpoint, payload: &[u8], tick: u16) {
    ARMED = false;
    if payload.is_empty() || payload.len() > CAPACITY {
        ROOM = u16::MAX;
        return;
    }
    buf()[..payload.len()].copy_from_slice(payload);
    LEN = payload.len();
    TICK = tick;
    ROOM = room;
    CHECKPOINT = cp;
    ARMED = true;
}

/// The checkpoint and start tick a pending restore replays, if armed for `room`.
pub unsafe fn pending(room: u16) -> Option<(save::Checkpoint, u16)> {
    if ARMED && ROOM == room && LEN != 0 {
        Some((CHECKPOINT, TICK))
    } else {
        None
    }
}

/// The captured bytes, for the card.
pub unsafe fn bytes() -> &'static [u8] {
    if ROOM == u16::MAX {
        return &[];
    }
    &buf()[..LEN]
}

pub unsafe fn tick() -> u16 {
    TICK
}

/// Lay the capture over a freshly started map.
#[inline(never)]
#[optimize(size)]
pub unsafe fn apply(m: &Map, nlogic: usize, nents: usize) {
    ARMED = false;
    let mut r = In {
        buf: &buf()[..LEN],
        at: 0,
    };
    if r.u8() != VERSION {
        return;
    }
    let now = TICK;
    let count = r.u8() as usize;
    let nprops = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
    let mut pi = 0;
    while pi < count.min(nprops) {
        let flags = r.u8();
        PROP_OCC_VIS[pi] = (PROP_OCC_VIS[pi] & !OCC_STATE) | (flags & OCC_STATE);
        if flags & 1 == 0 {
            // Picked up, or still waiting in a monstermaker's stock.
            PROP_ACTIVE[pi] = 0;
        } else {
            PROP_ACTIVE[pi] = 1;
            PROP_DORMANT[pi] = r.u8();
            PROP_MAKER[pi] = r.u8();
            PROP_HEALTH[pi] = r.u8();
            if flags & 2 == 0 {
                PROP_STATE[pi] = PROP_STATE_DEAD;
                PROP_DEATH_START[pi] = now.wrapping_sub(300);
            } else if flags & PROP_SCI_FOLLOWING != 0 {
                let pos = [
                    r.u16() as i16 as i32,
                    r.u16() as i16 as i32,
                    r.u16() as i16 as i32,
                ];
                PROP_POS[pi] = pos;
                PROP_YAW[pi] = r.u16();
                let leaf = camera_leaf(m, pos);
                if leaf > 0 && leaf <= i16::MAX as i32 {
                    PROP_LEAF[pi] = leaf as i16;
                }
                seed_prop_render_transform(pi);
                PROP_OCC_VIS[pi] |= PROP_OCC_DIRTY;
                prop_nav_cache_invalidate(pi);
            }
        }
        pi += 1;
    }
    let mut run = 0u8;
    let mut li = 0;
    while li < nlogic {
        if !skipped(LOGIC_KIND[li]) {
            if run == 0 {
                let b = r.u8();
                if b & 0x80 != 0 {
                    run = b & 0x7f;
                } else {
                    LOGIC_STATE[li] = b;
                }
            }
            if run != 0 {
                LOGIC_STATE[li] = 0;
                run -= 1;
            }
        }
        li += 1;
    }
    li = 0;
    while li < nlogic {
        let kind = LOGIC_KIND[li];
        if counted(kind) {
            LOGIC_COUNTER[li] = r.u16() as i16;
            if has_hp(kind) {
                LOGIC_BREAK_HP[li] = r.u16();
            }
        }
        li += 1;
    }
    let mut bits = 0u8;
    let mut ei = 0;
    while ei < nents {
        if ei & 7 == 0 {
            bits = r.u8();
        }
        ENT_ACTIVE[ei] = (bits >> (ei & 7)) & 1;
        ei += 1;
    }
    ei = 0;
    while ei < nents {
        if phase_kept(ei) {
            ENT_PHASE[ei] = r.u16() as i16 as i32;
            ENT_RENDER_PHASE[ei] = ENT_PHASE[ei];
            ent_prev_off_store(ei, ent_draw_offset(ei));
        }
        ei += 1;
    }
    let mut w = 0;
    while w < SPRITE_STATE_WORDS {
        SPRITE_REMOVED[w] = u32::from(r.u16()) | (u32::from(r.u16()) << 16);
        w += 1;
    }
    LIGHTSTYLE_ACTIVE_MASK = u32::from(r.u16()) | (u32::from(r.u16()) << 16);
    // Delayed events queued when the map started (its autos) must not fire
    // again, and ones still pending at the save are not carried.
    let mut e = 0;
    while e < MAX_LOGIC_EVENTS {
        LOGIC_EVENTS[e] = EMPTY_LOGIC_EVENT;
        e += 1;
    }
    (*core::ptr::addr_of_mut!(PVS_LIVE_DIRTY_BITS)).fill(u32::MAX);
    (*core::ptr::addr_of_mut!(PVS_LIVE_ENT_BITS)).fill(0);
    #[cfg(feature = "interaction-probe")]
    {
        // What came back: dead monsters, then the first logic records off
        // their starting state (index, state), and the phases of open doors.
        let mut dead = 0;
        let mut p = 0;
        while p < nprops {
            dead += (PROP_ACTIVE[p] != 0 && PROP_HEALTH[p] == 0) as i32;
            p += 1;
        }
        let mut vals = [dead, -1, 0, -1, 0, -1, 0, -1, 0];
        let mut k = 1;
        let mut l = 0;
        while l < nlogic && k < 9 {
            if LOGIC_STATE[l] != 0 && !skipped(LOGIC_KIND[l]) {
                vals[k] = l as i32;
                vals[k + 1] = LOGIC_STATE[l] as i32;
                k += 2;
            }
            l += 1;
        }
        reference_trace::probe_vals(now as u32, "restore", &vals);
    }
}
