//! Tripmines, measured against retail Half-Life under the Xash3D reference.
//! A mine arms 53 ticks after it is placed (54 after a level load for the
//! ones a map authors). Its beam runs from the wall to the first solid in
//! front of it; any body that can be hurt crossing the beam sets it off, and
//! the blast lands 5 ticks later centred 64 units in front of the mine, doing
//! 150 minus 0.4 per unit up to 375 units at every skill. Mines inside that
//! radius go off too, 4 ticks after the first blast.

use crate::*;

pub(crate) const ARM_TICKS: u8 = 53;
const BEAM_RANGE: i32 = 2048;
const FUSE_TICKS: u8 = 5;
const CHAIN_TICKS: u8 = 4;
pub(crate) const DAMAGE: u8 = 150;
pub(crate) const RADIUS: i32 = 375;
const BLAST_OFFSET: i32 = 64;
/// A map mine finishes arming this many ticks after the level starts.
const MAP_ARM_TICK: u16 = 54;
/// Sim tick the current level's first mine was seen on (a gap means a new level).
static mut BORN: u16 = 0;
static mut LAST_SEEN: u16 = 0;
/// Projectile `life` values at or above this are a lit fuse (life - base ticks left).
pub(crate) const FUSE_BASE: u8 = 128;

pub(crate) const PROP_TYPE_TRIPMINE: u8 = 57;

/// Where the beam stops: the first world or brush surface, at most 2,048 away.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn beam_end(
    m: &Map,
    movers: &[phys::Mover],
    from: [i32; 3],
    dir: [i32; 3],
) -> [i32; 3] {
    let far = [
        from[0] + ((dir[0] * BEAM_RANGE) >> 12),
        from[1] + ((dir[1] * BEAM_RANGE) >> 12),
        from[2] + ((dir[2] * BEAM_RANGE) >> 12),
    ];
    match phys::trace_line(m, movers, from, far) {
        Some(hit) => hit.pos,
        None => far,
    }
}

/// Whether the box `lo..hi` touches the segment a-b.
#[inline(never)]
#[optimize(size)]
fn touches(a: [i32; 3], b: [i32; 3], lo: [i32; 3], hi: [i32; 3]) -> bool {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    // Slab test in fixed point: t runs 0..=4096 along the segment.
    let mut t0 = 0i32;
    let mut t1 = 4096i32;
    let mut axis = 0;
    while axis < 3 {
        if d[axis] == 0 {
            if a[axis] < lo[axis] || a[axis] > hi[axis] {
                return false;
            }
        } else {
            let dd = d[axis];
            let mut ta = ((lo[axis] - a[axis]) << 12) / dd;
            let mut tb = ((hi[axis] - a[axis]) << 12) / dd;
            if ta > tb {
                core::mem::swap(&mut ta, &mut tb);
            }
            t0 = t0.max(ta);
            t1 = t1.min(tb);
            if t0 > t1 {
                return false;
            }
        }
        axis += 1;
    }
    true
}

/// Whether anything that can be hurt (the player, a live actor other than
/// `skip`) stands in the beam from `a` to `b`.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn beam_broken(a: [i32; 3], b: [i32; 3], skip: usize) -> bool {
    let p = LOGIC_PLAYER_POS;
    if LOGIC_PLAYER_HEALTH > 0
        && touches(
            a,
            b,
            [p[0] - 16, p[1] - PLAYER_HULL_HALF_HEIGHT, p[2] - 16],
            [p[0] + 16, p[1] + PLAYER_HULL_HALF_HEIGHT, p[2] + 16],
        )
    {
        return true;
    }
    let nprops = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
    let mut pi = 0usize;
    while pi < nprops {
        let k = PROP_KIND[pi];
        if pi != skip
            && PROP_ACTIVE[pi] != 0
            && PROP_HEALTH[pi] > 0
            && PROP_STATE[pi] != PROP_STATE_DEAD
            && k != PROP_TYPE_TRIPMINE
            && prop_start_health(k) != 0
            && !matches!(model_def(k).ai, AI_ITEM)
        {
            let (lo, hi) = actor_collision_bounds(k, PROP_POS[pi]);
            if touches(a, b, lo, hi) {
                return true;
            }
        }
        pi += 1;
    }
    false
}

/// Light the fuse of every other mine the blast at `c` reaches.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn chain(c: [i32; 3], skip_prop: usize) {
    let r2 = RADIUS * RADIUS;
    let nprops = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
    let mut pi = 0usize;
    while pi < nprops {
        if pi != skip_prop
            && PROP_KIND[pi] == PROP_TYPE_TRIPMINE
            && PROP_ACTIVE[pi] != 0
            && PROP_STATE[pi] != PROP_STATE_ATTACK
            && dist2_3(PROP_POS[pi], c) < r2
        {
            PROP_STATE[pi] = PROP_STATE_ATTACK;
            PROP_AI_TIMER[pi] = CHAIN_TICKS;
        }
        pi += 1;
    }
    let mut i = 0usize;
    while i < MAX_PROJECTILES {
        let p = &mut PROJECTILES[i];
        if p.active && p.kind == PROJ_TRIPMINE && p.life < FUSE_BASE && dist2_3(p.pos, c) < r2 {
            p.life = FUSE_BASE + CHAIN_TICKS;
        }
        i += 1;
    }
}

/// The blast point of a mine at `pos` facing `dir` (Q12).
#[inline(always)]
pub(crate) fn blast_center(pos: [i32; 3], dir: [i32; 3]) -> [i32; 3] {
    [
        pos[0] + ((dir[0] * BLAST_OFFSET) >> 12),
        pos[1] + ((dir[1] * BLAST_OFFSET) >> 12),
        pos[2] + ((dir[2] * BLAST_OFFSET) >> 12),
    ]
}

/// A mine placed by the player: count its arming, watch its beam, light and
/// run its fuse. Returns true when the mine has gone off.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_placed(m: &Map, movers: &[phys::Mover], i: usize) -> bool {
    let p = PROJECTILES[i];
    if p.life >= FUSE_BASE {
        let left = p.life - FUSE_BASE;
        if left > 1 {
            PROJECTILES[i].life = p.life - 1;
            return false;
        }
        PROJECTILES[i].active = false;
        let c = blast_center(p.pos, p.vel);
        explode(m, c, DAMAGE, RADIUS, true);
        chain(c, usize::MAX);
        return true;
    }
    if p.life > 0 {
        PROJECTILES[i].life = p.life - 1;
        return false;
    }
    let end = beam_end(m, movers, p.pos, p.vel);
    if beam_broken(p.pos, end, usize::MAX) {
        PROJECTILES[i].life = FUSE_BASE + FUSE_TICKS;
    }
    false
}

/// A mine a map authors (monster_tripmine). It faces away from the wall behind
/// it along the actor's yaw, arms at tick 54, and is also set off when shot.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_map(m: &Map, movers: &[phys::Mover], pi: usize) {
    if PROP_ACTIVE[pi] == 0 || PROP_STATE[pi] == PROP_STATE_DEAD {
        return;
    }
    let yaw = prop_yaw_value(PROP_YAW[pi]);
    let dir = [
        sincos::sin_q12(yaw),
        0,
        sincos::sin_q12((yaw + 1024) & 0x0fff),
    ];
    let pos = PROP_POS[pi];
    if PROP_STATE[pi] == PROP_STATE_ATTACK {
        PROP_AI_TIMER[pi] -= 1;
        if PROP_AI_TIMER[pi] == 0 {
            PROP_STATE[pi] = PROP_STATE_DEAD;
            PROP_HEALTH[pi] = 0;
            PROP_ACTIVE[pi] = 0;
            let c = blast_center(pos, dir);
            explode(m, c, DAMAGE, RADIUS, false);
            chain(c, pi);
        }
        return;
    }
    if SIM_NOW.wrapping_sub(LAST_SEEN) > 1 {
        BORN = SIM_NOW;
    }
    LAST_SEEN = SIM_NOW;
    if SIM_NOW.wrapping_sub(BORN) < MAP_ARM_TICK {
        return;
    }
    let d2 = dist2_xz(pos, LOGIC_PLAYER_POS);
    if d2 > 2600 * 2600 {
        return;
    }
    let end = beam_end(m, movers, pos, dir);
    if beam_broken(pos, end, pi) {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = FUSE_TICKS;
    }
}

/// A bullet or blast hit the mine: it goes off after the usual fuse.
pub(crate) unsafe fn shot(pi: usize) {
    if PROP_STATE[pi] != PROP_STATE_ATTACK {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = FUSE_TICKS;
    }
}

/// Whether the level's authored mines have finished arming.
#[inline(always)]
pub(crate) unsafe fn map_armed() -> bool {
    SIM_NOW.wrapping_sub(BORN) >= MAP_ARM_TICK
}
