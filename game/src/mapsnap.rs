//! A map's progress in about a kilobyte: which monsters are dead (and where a
//! following scientist stands), every door, button, trigger, counter, charger,
//! monster maker and breakable, the delayed events still pending, the removed
//! sprites and the light switches.
//!
//! Two uses, one format. A save point (a manual save, or the player's first
//! touch of a trigger_autosave) keeps the capture with the player's checkpoint
//! and, for a manual save, writes it to the card beside it; dying reloads the
//! latest one from RAM, as retail reloads its most recent save. A departure
//! (leaving by trigger_changelevel) keeps the left map's capture so that
//! walking back finds the level as it was left (retail saves each level when
//! it is left). Either way the map is re-spawned exactly as a level entry does
//! and the capture is laid over it, so everything it does not name (trains,
//! pushables, scripted sequences in flight, boss schedules) starts over.
//!
//! Deadlines are absolute simulation ticks, so a restored map resumes the tick
//! count its capture was taken at.
//!
//! Two buffers: `HERE` is the current map's save point (or the departure
//! capture of the map just re-entered), `THERE` the map last left.

use crate::*;

/// Capture size limit. A map that needs more is not captured and a death
/// restarts it from its entry.
pub const CAPACITY: usize = 1024;

const VERSION: u8 = 6;
const OCC_STATE: u8 = PROP_SCI_FOLLOWING
    | PROP_SCI_PROVOKED
    | PROP_SCI_PREDISASTER
    | PROP_SCRIPT_NOINTERRUPT
    | PROP_SCI_HELLO_SAID;
const NO_ROOM: u16 = u16::MAX;

struct Slot {
    buf: [u8; CAPACITY],
    len: u16,
    tick: u16,
    room: u16,
    /// A save point (with the player's checkpoint) rather than a departure.
    save: bool,
}

const EMPTY_SLOT: Slot = Slot {
    buf: [0; CAPACITY],
    len: 0,
    tick: 0,
    room: NO_ROOM,
    save: false,
};

static mut SLOTS: [Slot; 2] = [EMPTY_SLOT; 2];
/// Index of `HERE`.
static mut HERE: usize = 0;
/// The checkpoint of `HERE` when it is a save point.
static mut CHECKPOINT: save::Checkpoint = save::Checkpoint::EMPTY;
/// A restore waits for the next map start.
static mut ARMED: bool = false;
/// Where the monsters section starts, found by `apply_world`.
static mut PROPS_AT: u16 = 0;
/// The running map's dimensions and tick, refreshed every simulation tick so a
/// capture can run from the pause menu, which has no map in hand.
static mut DIMS: (u16, u16, u16) = (0, 0, 0);

#[inline(always)]
pub unsafe fn note_dims(nlogic: usize, nents: usize, tick: u16) {
    DIMS = (nlogic as u16, nents as u16, tick);
}

#[inline(always)]
unsafe fn slot(i: usize) -> &'static mut Slot {
    &mut (*core::ptr::addr_of_mut!(SLOTS))[i & 1]
}

#[inline(always)]
unsafe fn here() -> &'static mut Slot {
    slot(HERE)
}

/// Breakables that carry a `globalname` and have been broken, newest last
/// (a ring: only the neighbouring maps' copies ever ask).
const DEAD_CAP: usize = 16;
static mut DEAD: [u16; DEAD_CAP] = [0; DEAD_CAP];
static mut DEAD_N: u8 = 0;

/// What a map entry started with, so a death that restarts the map from its
/// entry also takes the globals back to that moment.
struct Entry {
    dead: [u16; DEAD_CAP],
    dead_n: u8,
    hash: [u16; 32],
    on: [bool; 32],
    count: usize,
}
static mut ENTRY: Entry = Entry {
    dead: [0; DEAD_CAP],
    dead_n: 0,
    hash: [0; 32],
    on: [false; 32],
    count: 0,
};

#[inline(always)]
unsafe fn dead_ids() -> &'static mut [u16; DEAD_CAP] {
    &mut *core::ptr::addr_of_mut!(DEAD)
}

pub unsafe fn is_dead(id: u16) -> bool {
    let n = (DEAD_N as usize).min(DEAD_CAP);
    dead_ids()[..n].contains(&id)
}

pub unsafe fn mark_dead(id: u16) {
    if !is_dead(id) {
        dead_ids()[DEAD_N as usize % DEAD_CAP] = id;
        DEAD_N = DEAD_N.wrapping_add(1);
        if DEAD_N as usize >= 2 * DEAD_CAP {
            DEAD_N -= DEAD_CAP as u8;
        }
    }
}

/// A fresh game forgets every broken global.
pub unsafe fn reset_dead() {
    DEAD_N = 0;
}

/// A broken breakable (`brush`): mark its global id dead if it has one.
pub unsafe fn on_break(m: &Map, nlogic: usize, brush: u16) {
    let mut li = 0;
    while li < nlogic {
        if LOGIC_KIND[li] == map::LOGIC_GLOBAL_BREAKABLE {
            let rec = m.logic(li);
            if rec.brush == brush {
                mark_dead(rec.arg0);
                #[cfg(feature = "interaction-probe")]
                reference_trace::probe(SIM_NOW as u32, "global_dead", "", rec.arg0 as i32);
            }
        }
        li += 1;
    }
}

/// Map start: the breakables whose global id is already dead are gone.
pub unsafe fn remove_dead(m: &Map, nlogic: usize, nents: usize) {
    let mut li = 0;
    while li < nlogic {
        if LOGIC_KIND[li] == map::LOGIC_GLOBAL_BREAKABLE {
            let rec = m.logic(li);
            if is_dead(rec.arg0) {
                let mut bi = 0;
                while bi < nlogic {
                    if LOGIC_KIND[bi] == map::LOGIC_FUNC_BREAKABLE
                        && m.logic(bi).brush == rec.brush
                    {
                        if let Some(ei) = logic_valid_brush(rec.brush, nents) {
                            ENT_ACTIVE[ei] = 0;
                        }
                        LOGIC_STATE[bi] = LOGIC_STATE_REMOVED;
                        LOGIC_BREAK_HP[bi] = 0;
                        #[cfg(feature = "interaction-probe")]
                        reference_trace::probe(0, "global_removed", "", rec.arg0 as i32);
                    }
                    bi += 1;
                }
            }
        }
        li += 1;
    }
}

/// A map starts: remember the globals it started with.
pub unsafe fn note_entry() {
    ENTRY.dead = *dead_ids();
    ENTRY.dead_n = DEAD_N;
    ENTRY.hash = GLOBAL_HASH;
    ENTRY.on = GLOBAL_ON;
    ENTRY.count = GLOBAL_COUNT;
}

/// Restarting a map from its entry: its globals come back too.
pub unsafe fn rollback_to_entry() {
    *dead_ids() = ENTRY.dead;
    DEAD_N = ENTRY.dead_n;
    GLOBAL_HASH = ENTRY.hash;
    GLOBAL_ON = ENTRY.on;
    GLOBAL_COUNT = ENTRY.count;
}

/// One walker serves both directions, so the format is written once: `read`
/// loads each field from the buffer, otherwise it stores it.
struct Port {
    buf: *mut u8,
    cap: usize,
    at: usize,
    read: bool,
    over: bool,
}

impl Port {
    #[inline(never)]
    unsafe fn b(&mut self, v: *mut u8) {
        if self.at >= self.cap {
            self.over = true;
            if self.read {
                *v = 0;
            }
        } else if self.read {
            *v = *self.buf.add(self.at);
        } else {
            *self.buf.add(self.at) = *v;
        }
        self.at += 1;
    }
    #[inline(never)]
    unsafe fn w(&mut self, v: *mut u16) {
        let mut lo = *v as u8;
        let mut hi = (*v >> 8) as u8;
        self.b(&mut lo);
        self.b(&mut hi);
        if self.read {
            *v = u16::from(lo) | (u16::from(hi) << 8);
        }
    }
}

const SKIP: u8 = 1;
const COUNTED: u8 = 2;
const HP: u8 = 4;

/// What a restore does with a logic kind: nothing for static records, sound
/// and music and the machinery that restarts with the level (SKIP); a counter
/// too for what is left to make or give and for a breakable's health.
#[inline(always)]
fn class(kind: u8) -> u8 {
    match kind {
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
        | map::LOGIC_GLOBAL_BREAKABLE => SKIP,
        map::LOGIC_FUNC_BREAKABLE | map::LOGIC_FUNC_GUNTARGET => COUNTED | HP,
        map::LOGIC_TRIGGER_COUNTER
        | map::LOGIC_MONSTERMAKER
        | map::LOGIC_HEALTH_CHARGER
        | map::LOGIC_HEV_CHARGER => COUNTED,
        _ => 0,
    }
}

/// Whether a brush's phase is a plain 0..4096 slide the capture can hold.
#[inline(always)]
unsafe fn phase_kept(ei: usize) -> bool {
    if ENT_CACHE[ei].kind == ENT_KIND_PUSHABLE {
        return false;
    }
    let li = ENT_BRUSH_LOGIC[ei] as usize;
    li < MAX_LOGIC && matches!(LOGIC_KIND[li], map::LOGIC_FUNC_DOOR | map::LOGIC_FUNC_BUTTON)
}

/// The world: logic states (zeros run-length coded, most records sit at their
/// starting state), counters, brushes, sprites, lights, broken globals and the
/// events still pending (their deadlines are absolute ticks). `dead` says
/// whether to take the broken globals back (a save point) or keep the live
/// ones (walking back into a map).
#[inline(never)]
#[optimize(size)]
unsafe fn walk_world(p: &mut Port, nlogic: usize, nents: usize, dead: bool) {
    let mut ver = VERSION;
    p.b(&mut ver);
    if p.read && ver != VERSION {
        p.over = true;
        return;
    }
    let mut run = 0u8;
    let mut li = 0;
    while li < nlogic {
        if class(LOGIC_KIND[li]) & SKIP == 0 {
            let at = core::ptr::addr_of_mut!(LOGIC_STATE[li]);
            if p.read {
                if run == 0 {
                    let mut b = 0;
                    p.b(&mut b);
                    if b & 0x80 != 0 && b != LOGIC_STATE_REMOVED {
                        run = b & 0x7f;
                    } else {
                        *at = b;
                    }
                }
                if run != 0 {
                    *at = 0;
                    run -= 1;
                }
            } else if *at == 0 && run < 126 {
                run += 1;
            } else {
                if run != 0 {
                    let mut b = 0x80 | run;
                    p.b(&mut b);
                }
                run = (*at == 0) as u8;
                if *at != 0 {
                    p.b(at);
                }
            }
        }
        li += 1;
    }
    if !p.read && run != 0 {
        let mut b = 0x80 | run;
        p.b(&mut b);
    }
    li = 0;
    while li < nlogic {
        let c = class(LOGIC_KIND[li]);
        if c & COUNTED != 0 {
            p.w(core::ptr::addr_of_mut!(LOGIC_COUNTER[li]).cast());
            if c & HP != 0 {
                p.w(core::ptr::addr_of_mut!(LOGIC_BREAK_HP[li]));
            }
        }
        li += 1;
    }
    let mut ei = 0;
    while ei < nents {
        let mut bits = 0u8;
        if !p.read {
            let mut k = 0;
            while k < 8 && ei + k < nents {
                bits |= ((ENT_ACTIVE[ei + k] != 0) as u8) << k;
                k += 1;
            }
        }
        p.b(&mut bits);
        if p.read {
            let mut k = 0;
            while k < 8 && ei + k < nents {
                ENT_ACTIVE[ei + k] = (bits >> k) & 1;
                k += 1;
            }
        }
        ei += 8;
    }
    ei = 0;
    while ei < nents {
        if phase_kept(ei) {
            let mut v = ENT_PHASE[ei] as u16;
            p.w(&mut v);
            if p.read {
                ENT_PHASE[ei] = v as i16 as i32;
                ENT_RENDER_PHASE[ei] = ENT_PHASE[ei];
                ent_prev_off_store(ei, ent_draw_offset(ei));
            }
        }
        ei += 1;
    }
    let sprites: *mut u16 = core::ptr::addr_of_mut!(SPRITE_REMOVED).cast();
    let mut k = 0;
    while k < SPRITE_STATE_WORDS * 2 {
        p.w(sprites.add(k));
        k += 1;
    }
    let lights: *mut u16 = core::ptr::addr_of_mut!(LIGHTSTYLE_ACTIVE_MASK).cast();
    p.w(lights);
    p.w(lights.add(1));
    let mut n = (DEAD_N as usize).min(DEAD_CAP) as u8;
    p.b(&mut n);
    let mut d = 0;
    while d < (n as usize).min(DEAD_CAP) {
        let mut id = dead_ids()[d];
        p.w(&mut id);
        if p.read && dead {
            dead_ids()[d] = id;
        }
        d += 1;
    }
    if p.read && dead {
        DEAD_N = n.min(DEAD_CAP as u8);
    }
    let mut n = 0u8;
    let mut e = 0;
    if p.read {
        // The events queued when the map started (its autos) must not fire
        // again: the queue is exactly what was pending at the capture.
        while e < MAX_LOGIC_EVENTS {
            LOGIC_EVENTS[e] = EMPTY_LOGIC_EVENT;
            e += 1;
        }
    } else {
        while e < MAX_LOGIC_EVENTS {
            n += logic_state::event_active(LOGIC_EVENTS[e].meta) as u8;
            e += 1;
        }
    }
    p.b(&mut n);
    e = 0;
    let mut slot = 0;
    while slot < (n as usize).min(MAX_LOGIC_EVENTS) {
        if !p.read {
            while !logic_state::event_active(LOGIC_EVENTS[e].meta) {
                e += 1;
            }
        }
        let ev = &mut LOGIC_EVENTS[if p.read { slot } else { e }];
        let ev = ev as *mut LogicEvent;
        p.w(core::ptr::addr_of_mut!((*ev).at));
        p.w(core::ptr::addr_of_mut!((*ev).target));
        p.w(core::ptr::addr_of_mut!((*ev).killtarget));
        p.w(core::ptr::addr_of_mut!((*ev).meta));
        e += 1;
        slot += 1;
    }
    if p.read {
        PROPS_AT = p.at as u16;
        (*core::ptr::addr_of_mut!(PVS_LIVE_DIRTY_BITS)).fill(u32::MAX);
        (*core::ptr::addr_of_mut!(PVS_LIVE_ENT_BITS)).fill(0);
    }
}

/// The monsters: who is alive, dead or gone, their health, the flags a
/// following scientist carries and, for a follower, where it stands.
#[inline(never)]
#[optimize(size)]
unsafe fn walk_props(p: &mut Port, m: Option<&Map>, tick: u16) {
    let mut count = PROP_COUNT.min(CARRY_MAILBOX_FIRST) as u8;
    p.b(&mut count);
    let nprops = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
    let mut pi = 0;
    while pi < count as usize {
        let live = pi < nprops;
        // bit 0 active, bit 1 alive, bits 2-6 the scientist flags, bit 7 the
        // monstermaker bytes follow.
        let mut flags = 0u8;
        if !p.read {
            flags = (PROP_ACTIVE[pi] != 0) as u8
                | ((PROP_HEALTH[pi] != 0) as u8) << 1
                | (PROP_OCC_VIS[pi] & OCC_STATE)
                | ((PROP_DORMANT[pi] | PROP_MAKER[pi] != 0) as u8) << 7;
        }
        p.b(&mut flags);
        let mut dormant = PROP_DORMANT[pi.min(MAX_PROPS - 1)];
        let mut maker = PROP_MAKER[pi.min(MAX_PROPS - 1)];
        if flags & 1 != 0 {
            if flags & 0x80 != 0 {
                p.b(&mut dormant);
                p.b(&mut maker);
            } else if p.read {
                dormant = 0;
                maker = 0;
            }
        }
        let mut health = if p.read { 0 } else { PROP_HEALTH[pi] };
        if flags & 2 != 0 {
            p.b(&mut health);
        }
        let mut pos = PROP_POS[pi.min(MAX_PROPS - 1)];
        let mut yaw = PROP_YAW[pi.min(MAX_PROPS - 1)];
        let follower = flags & 3 == 3 && flags & PROP_SCI_FOLLOWING != 0;
        if follower {
            let mut k = 0;
            while k < 3 {
                let mut v = pos[k] as u16;
                p.w(&mut v);
                pos[k] = v as i16 as i32;
                k += 1;
            }
            p.w(&mut yaw);
        }
        if p.read && live {
            PROP_OCC_VIS[pi] = (PROP_OCC_VIS[pi] & !OCC_STATE) | (flags & OCC_STATE);
            if flags & 1 == 0 {
                // Picked up, carried away, or still waiting in a maker's stock.
                PROP_ACTIVE[pi] = 0;
            } else {
                PROP_ACTIVE[pi] = 1;
                PROP_DORMANT[pi] = dormant;
                PROP_MAKER[pi] = maker;
                PROP_HEALTH[pi] = health;
                if flags & 2 == 0 {
                    PROP_STATE[pi] = PROP_STATE_DEAD;
                    PROP_DEATH_START[pi] = tick.wrapping_sub(300);
                } else if follower {
                    PROP_POS[pi] = pos;
                    PROP_YAW[pi] = yaw;
                    let leaf = m.map_or(0, |m| camera_leaf(m, pos));
                    if leaf > 0 && leaf <= i16::MAX as i32 {
                        PROP_LEAF[pi] = leaf as i16;
                    }
                    seed_prop_render_transform(pi);
                    PROP_OCC_VIS[pi] |= PROP_OCC_DIRTY;
                    prop_nav_cache_invalidate(pi);
                }
            }
        }
        pi += 1;
    }
}

/// Capture the running map into the `HERE` slot. False when it does not fit.
#[inline(never)]
unsafe fn capture_here(room: u16, save: bool) -> bool {
    let (nlogic, nents, tick) = DIMS;
    let s = here();
    let mut p = Port {
        buf: s.buf.as_mut_ptr(),
        cap: CAPACITY,
        at: 0,
        read: false,
        over: false,
    };
    walk_world(&mut p, nlogic as usize, nents as usize, false);
    walk_props(&mut p, None, tick);
    if p.over {
        s.room = NO_ROOM;
        s.len = 0;
        return false;
    }
    s.len = p.at as u16;
    s.tick = tick;
    s.room = room;
    s.save = save;
    true
}

/// Capture the map as it stands and keep it, with the player's checkpoint, as
/// the map's latest save point (a manual save or a trigger_autosave).
pub unsafe fn keep_now(cp: &save::Checkpoint) -> bool {
    let ok = capture_here(cp.room_id, true);
    if ok {
        CHECKPOINT = *cp;
    }
    ok
}

/// Leaving `room` by changelevel: keep the map as it stands for a return, and
/// make the slot of the map last left (or of this one's earlier save point)
/// the current one.
pub unsafe fn depart(room: u16) {
    capture_here(room, false);
    HERE ^= 1;
}

/// A new map starts. Whatever `HERE` held for another map is gone; one held
/// for this map is its departure capture, and restores the world on entry.
pub unsafe fn arrive(room: u16) {
    let s = here();
    if s.room != room {
        s.room = NO_ROOM;
        s.len = 0;
        ARMED = false;
    } else if !s.save && s.len != 0 {
        ARMED = true;
    }
}

/// The map has started with the capture laid over it.
pub unsafe fn consume() {
    ARMED = false;
}

/// A fresh game or chapter jump: no capture of an earlier session applies.
pub unsafe fn invalidate() {
    slot(0).room = NO_ROOM;
    slot(1).room = NO_ROOM;
    ARMED = false;
}

/// The player is dead: if this map has a capture, restore it on the next
/// start instead of the level entry. True when armed.
pub unsafe fn arm_for_death(room: u16) -> bool {
    let s = here();
    if s.room != room || s.len == 0 {
        return false;
    }
    ARMED = true;
    true
}

/// A save was read from the card: hold its payload for the map start.
pub unsafe fn arm_from_card(room: u16, cp: save::Checkpoint, payload: &[u8], tick: u16) {
    ARMED = false;
    let s = here();
    if payload.is_empty() || payload.len() > CAPACITY {
        s.room = NO_ROOM;
        return;
    }
    s.buf[..payload.len()].copy_from_slice(payload);
    s.len = payload.len() as u16;
    s.tick = tick;
    s.room = room;
    s.save = true;
    CHECKPOINT = cp;
    ARMED = true;
}

/// What a pending restore replays for `room`: the start tick, and the
/// player's checkpoint when it is a save point.
pub unsafe fn pending(room: u16) -> Option<(Option<save::Checkpoint>, u16)> {
    let s = here();
    if ARMED && s.room == room && s.len != 0 {
        Some((if s.save { Some(CHECKPOINT) } else { None }, s.tick))
    } else {
        None
    }
}

/// The captured bytes of the save point just kept, for the card.
pub unsafe fn bytes() -> &'static [u8] {
    let s = here();
    if s.room == NO_ROOM || !s.save {
        return &[];
    }
    &s.buf[..s.len as usize]
}

pub unsafe fn tick() -> u16 {
    here().tick
}

/// Lay the world of the pending capture over a freshly started map. Runs
/// before the carried doors, trains and pushables are laid over it, so what
/// crossed the transition wins.
pub unsafe fn apply_world(nlogic: usize, nents: usize) {
    let s = here();
    let mut p = Port {
        buf: s.buf.as_mut_ptr(),
        cap: s.len as usize,
        at: 0,
        read: true,
        over: false,
    };
    walk_world(&mut p, nlogic, nents, s.save);
    if p.over {
        ARMED = false;
    }
}

/// Lay the monsters of the pending capture over the freshly spawned ones.
pub unsafe fn apply_props(m: &Map, nlogic: usize) {
    let _ = nlogic; // only the probe build reads it
    let s = here();
    let tick = s.tick;
    let mut p = Port {
        buf: s.buf.as_mut_ptr(),
        cap: s.len as usize,
        at: PROPS_AT as usize,
        read: true,
        over: false,
    };
    walk_props(&mut p, Some(m), tick);
    #[cfg(feature = "interaction-probe")]
    {
        // What came back: dead monsters, then the first logic records off
        // their starting state (index, state).
        let nprops = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
        let mut dead = 0;
        let mut q = 0;
        while q < nprops {
            dead += (PROP_ACTIVE[q] != 0 && PROP_HEALTH[q] == 0) as i32;
            q += 1;
        }
        let mut vals = [dead, -1, 0, -1, 0, -1, 0, -1, 0];
        let mut k = 1;
        let mut l = 0;
        while l < nlogic && k < 9 {
            if LOGIC_STATE[l] != 0 && class(LOGIC_KIND[l]) & SKIP == 0 {
                vals[k] = l as i32;
                vals[k + 1] = LOGIC_STATE[l] as i32;
                k += 2;
            }
            l += 1;
        }
        reference_trace::probe_vals(tick as u32, "restore", &vals);
    }
}
