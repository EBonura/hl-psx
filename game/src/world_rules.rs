//! Integer-only rules of world objects (floating pushables, rotating doors,
//! track waypoints, blood decals), kept free of PS1 state so the host runner
//! can pin their behaviour. The game side gathers observations from its
//! globals, calls these steps and applies the results.

/// A floating pushable as the buoyancy step sees it: the world centre of its
/// box, half its height, and its buoyancy factor (the authored skin value).
/// World Y is up.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FloatBody {
    pub center: [i32; 3],
    pub half_height: i32,
    pub buoyancy: i32,
}

/// What one buoyancy tick decided: the new vertical speed in whole units per
/// 20 Hz tick, and whether the pushable still rests on its support.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FloatTick {
    pub vy: i32,
    pub grounded: bool,
}

/// Water lift (buoyancy times submerged height) that exactly cancels the
/// pushable's weight: the 800 units per second squared of gravity.
const BALANCE_LIFT: i32 = 800;

/// A pushable resting on a support only breaks free when the water wants to
/// carry it up by at least this many units per tick.
const FLOOR_GRIP: i32 = 4;

/// Fastest a pushable sinks through water, in units per tick.
const MAX_SINK: i32 = 6;

/// How much of the box is under water, measured upward from its bottom. The
/// bottom probe has already been found wet, so the water line lies between
/// one unit above the bottom and the top face.
fn submerged_height(body: FloatBody, wet: &mut dyn FnMut([i32; 3]) -> bool) -> i32 {
    let [x, y, z] = body.center;
    let bottom = y - body.half_height;
    let top = y + body.half_height;
    if wet([x, top, z]) {
        return top - bottom;
    }
    let mut under = bottom + 1;
    let mut over = top;
    while over - under > 1 {
        let mid = under + (over - under) / 2;
        if wet([x, mid, z]) {
            under = mid;
        } else {
            over = mid;
        }
    }
    under - bottom
}

/// One 20 Hz buoyancy tick for a pushable. `vy` is the stored vertical speed
/// before gravity; `grounded` says whether it rests on a support. `wet`
/// answers whether a world point is inside water. Returns None when the
/// pushable has no buoyancy or its bottom is dry, so the caller falls as usual.
///
/// The box is pushed toward the depth where its lift equals its weight, at a
/// speed proportional to how far off that depth it is, so it eases in from
/// either side. A box too heavy for its own height (its balance depth is
/// deeper than the box is tall) never settles and keeps sinking.
pub fn pushable_float_tick(
    body: FloatBody,
    vy: i32,
    grounded: bool,
    wet: &mut dyn FnMut([i32; 3]) -> bool,
) -> Option<FloatTick> {
    if body.buoyancy <= 0 {
        return None;
    }
    let [x, y, z] = body.center;
    if !wet([x, y - body.half_height + 1, z]) {
        return None;
    }
    let lift = submerged_height(body, wet) * body.buoyancy;
    // More buoyant boxes may rise faster; any box may sink at up to 6.
    let top_rise = (body.buoyancy / 16).clamp(1, 6);
    let wanted = (lift - BALANCE_LIFT) / (2 * body.buoyancy);
    let target = wanted.clamp(-MAX_SINK, top_rise);
    let current = if grounded { 0 } else { vy };
    if grounded && wanted < FLOOR_GRIP {
        return Some(FloatTick {
            vy: 0,
            grounded: true,
        });
    }
    // Close half of the gap to the wanted speed each tick, rounding up so the
    // wanted speed is actually reached.
    let gap = target - current;
    let next = current + (gap + gap.signum()) / 2;
    Some(FloatTick {
        vy: next,
        grounded: false,
    })
}

/// Rotating-door spawnflags that keep the authored swing direction.
pub const DOOR_ONE_WAY: u16 = 16;
pub const DOOR_ROTATE_Z: u16 = 64;
pub const DOOR_ROTATE_X: u16 = 128;

/// Whoever started the chain that opens a door: their position and the
/// horizontal direction they face as world (x, z), 1.0 = 4096.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DoorActivator {
    pub pos: [i32; 3],
    pub forward_xz: [i32; 2],
}

/// Which way a rotating door swings as it starts to open. Some(true) means
/// against its authored direction, Some(false) along it, and None leaves the
/// current direction alone (one-way and X/Z-axis doors, or nobody started
/// the chain).
pub fn rotating_door_opens_reversed(
    spawnflags: u16,
    hinge: [i32; 3],
    activator: Option<DoorActivator>,
) -> Option<bool> {
    if spawnflags & (DOOR_ONE_WAY | DOOR_ROTATE_Z | DOOR_ROTATE_X) != 0 {
        return None;
    }
    let who = activator?;
    // Where the activator stands relative to the hinge on the floor plane
    // (kept small enough that the products below cannot overflow).
    let from_hinge = [
        (who.pos[0] - hinge[0]).clamp(-1 << 15, 1 << 15),
        (who.pos[2] - hinge[2]).clamp(-1 << 15, 1 << 15),
    ];
    // Which side of the activator's line of sight they stand on, from the
    // 2D cross product of their facing with their offset from the hinge. One
    // sign sends the door against its authored turn so it opens away from them.
    let turn = who.forward_xz[0] * from_hinge[1] - who.forward_xz[1] * from_hinge[0];
    Some(turn > 0)
}

/// How far past the wound, in world units, a hit's blood can land.
pub const BLEED_REACH: i32 = 172;

/// Blood a creature sheds when hit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BloodColour {
    Red,
    Yellow,
    /// Turrets, machines and scenery do not bleed.
    NoBlood,
}

/// Blood colour for each actor type id.
pub const fn species_blood(actor_type: u8) -> BloodColour {
    match actor_type {
        // headcrab, zombie, houndeye, bullsquid, vortigaunt, alien grunt,
        // controller, cockroach, gargantua, nihilanth, big momma, ichthyosaur,
        // flock, tentacle, vent zombie, snark, baby headcrab
        2 | 5 | 6 | 7 | 9 | 10 | 11 | 14 | 16 | 17 | 18 | 19 | 24 | 50 | 55 | 58 | 59 => {
            BloodColour::Yellow
        }
        // leech, G-Man, turrets, apache, Hazard Course hologram, tripmine,
        // osprey, gibs and scenery props
        13 | 15 | 20..=23 | 56 | 57 | 61..=75 => BloodColour::NoBlood,
        _ => BloodColour::Red,
    }
}

/// What a blood trail needs from the game, called in this order per decal:
/// three random draws (one per axis), then one trace-and-stamp.
pub trait BloodTrailWorld {
    /// Uniform value in 0..n.
    fn random_below(&mut self, n: u32) -> u32;
    /// Trace from `from` to `to` and leave a blood decal where it hits.
    fn trace_and_stamp(&mut self, from: [i32; 3], to: [i32; 3]);
}

/// Blood decals behind a wound: 1, 2 or 4 traces continuing the shot from the
/// wound (damage under 10, under 25, above), each direction jittered per axis
/// by up to 0.1, 0.2 or 0.3. `shot_start`/`shot_end` give the shot direction
/// and `shot_len` its length. Nothing happens for zero damage or a creature
/// that does not bleed.
pub fn blood_trail(
    wound: [i32; 3],
    shot_start: [i32; 3],
    shot_end: [i32; 3],
    shot_len: i32,
    damage: u8,
    colour: BloodColour,
    world: &mut dyn BloodTrailWorld,
) {
    if damage == 0 || colour == BloodColour::NoBlood {
        return;
    }
    // Bigger wounds leave more decals, scattered more widely (fractions of
    // the unit shot direction, in 1/4096).
    let (decals, scatter) = match damage {
        0..=9 => (1, 410),
        10..=24 => (2, 819),
        _ => (4, 1229),
    };
    let heading: [i32; 3] =
        core::array::from_fn(|axis| (shot_end[axis] - shot_start[axis]) * 4096 / shot_len.max(1));
    for _ in 0..decals {
        let mut to = wound;
        for axis in 0..3 {
            let wobble = world.random_below(2 * scatter as u32 + 1) as i32 - scatter;
            to[axis] += ((heading[axis] + wobble) * BLEED_REACH) >> 12;
        }
        world.trace_and_stamp(wound, to);
    }
}

/// A tram's waypoint graph as authored. Node ids are 0..node_count.
pub trait TrackGraph {
    fn node_count(&self) -> usize;
    fn position(&self, node: usize) -> [i32; 3];
    /// False for a plain path: the nodes in order, no branches or switches.
    fn is_branching(&self) -> bool;
    /// The authored next node.
    fn successor(&self, node: usize) -> Option<usize>;
    /// The authored alternate next node, if any.
    fn alternate(&self, node: usize) -> Option<usize>;
    /// The alternate applies only when travelling in reverse.
    fn alternate_reverse_only(&self, node: usize) -> bool;
}

/// How a track node is fired.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackUse {
    On,
    Off,
    Toggle,
}

/// Live switch state of up to 256 track nodes: which nodes with an alternate
/// are switched to it, and which nodes are disabled.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrackSwitches {
    alt: [u32; 8],
    off: [u32; 8],
}

impl TrackSwitches {
    pub const fn new() -> Self {
        Self {
            alt: [0; 8],
            off: [0; 8],
        }
    }

    /// Mark a node disabled (used for nodes authored disabled at map start).
    pub fn disable(&mut self, node: usize) {
        if node < 256 {
            self.off[node >> 5] |= 1 << (node & 31);
        }
    }

    pub fn is_disabled(&self, node: usize) -> bool {
        bit(&self.off, node)
    }

    pub fn is_switched(&self, node: usize) -> bool {
        bit(&self.alt, node)
    }

    /// Fire a node: one with an alternate switches between its paths (On =
    /// primary, Off = alternate), any other is enabled (On) or disabled
    /// (Off); Toggle flips either.
    pub fn use_node(&mut self, node: usize, has_alternate: bool, how: TrackUse) {
        if node >= 256 {
            return;
        }
        let bits = if has_alternate {
            &mut self.alt
        } else {
            &mut self.off
        };
        let mask = 1u32 << (node & 31);
        match how {
            TrackUse::On => bits[node >> 5] &= !mask,
            TrackUse::Off => bits[node >> 5] |= mask,
            TrackUse::Toggle => bits[node >> 5] ^= mask,
        }
    }

    /// The node after `node` given the switches (disabled nodes included).
    pub fn next<G: TrackGraph + ?Sized>(&self, g: &G, node: usize) -> Option<usize> {
        if !g.is_branching() {
            return (node + 1 < g.node_count()).then_some(node + 1);
        }
        if let Some(alt) = g.alternate(node) {
            if self.is_switched(node) && !g.alternate_reverse_only(node) {
                return Some(alt);
            }
        }
        g.successor(node)
    }

    /// The next node a moving train may enter: a disabled node stops it.
    pub fn next_open<G: TrackGraph + ?Sized>(&self, g: &G, node: usize) -> Option<usize> {
        self.next(g, node).filter(|&n| !self.is_disabled(n))
    }
}

#[inline(always)]
fn bit(bits: &[u32; 8], i: usize) -> bool {
    i < 256 && bits[i >> 5] & (1 << (i & 31)) != 0
}

/// Length of a world-space segment, at least 1.
#[inline]
fn track_segment_len(a: [i32; 3], b: [i32; 3]) -> i32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    psx_math::int32::isqrt_i32(dx * dx + dy * dy + dz * dz).max(1)
}

/// Point `ahead` units further along the track from `seg_dist` units past
/// node `seg`, in 1/256 world units. Disabled nodes do not stop it; past the
/// last node it continues straight along the final segment; at a branching
/// dead end with no segment it stays on the node.
pub fn track_lookahead_q8<G: TrackGraph + ?Sized>(
    g: &G,
    sw: &TrackSwitches,
    mut seg: usize,
    mut seg_dist: i32,
    mut ahead: i32,
) -> [i32; 3] {
    let n = g.node_count();
    if n == 0 {
        return [0; 3];
    }
    if n == 1 {
        let p = g.position(0);
        return [p[0] << 8, p[1] << 8, p[2] << 8];
    }
    if !g.is_branching() {
        seg = seg.min(n - 2);
    }
    ahead = ahead.max(0);
    loop {
        let a = g.position(seg);
        let Some(next) = sw.next(g, seg) else {
            return [a[0] << 8, a[1] << 8, a[2] << 8];
        };
        let b = g.position(next);
        let len = track_segment_len(a, b).max(1);
        let left = (len - seg_dist).max(0);
        if ahead <= left {
            return lerp_q8(a, b, seg_dist + ahead, len);
        }
        ahead -= left;
        if sw.next(g, next).is_some() {
            seg = next;
            seg_dist = 0;
            continue;
        }
        let f = (ahead << 12) / len;
        return [
            (b[0] << 8) + ((b[0] - a[0]) * f >> 4),
            (b[1] << 8) + ((b[1] - a[1]) * f >> 4),
            (b[2] << 8) + ((b[2] - a[2]) * f >> 4),
        ];
    }
}

#[inline(always)]
fn lerp_q8(a: [i32; 3], b: [i32; 3], dist: i32, len: i32) -> [i32; 3] {
    let f = (dist.max(0) << 12) / len.max(1);
    [
        (a[0] << 8) + ((b[0] - a[0]) * f >> 4),
        (a[1] << 8) + ((b[1] - a[1]) * f >> 4),
        (a[2] << 8) + ((b[2] - a[2]) * f >> 4),
    ]
}

/// The train's speed after it reaches a node with speed key `node_speed`:
/// a positive key replaces the speed, zero keeps it.
pub const fn track_speed_at_node(current: i32, node_speed: i32) -> i32 {
    if node_speed > 0 {
        node_speed
    } else {
        current
    }
}
