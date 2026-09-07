//! Memory-card save/load: one checkpoint slot on port 1.
//!
//! What a save holds is decided by what a level transition already carries. The
//! runtime keeps per-map entity state only for the map you are standing in;
//! everything that survives a `trigger_changelevel` lives in the `CARRY_*`
//! statics plus the 15-record transition mailbox. A save is that same set,
//! serialised, plus where the player is standing. So loading restores exactly
//! what walking through a level change restores -- no more, and no less.
//!
//! ## Why this fits
//!
//! The payload is a few hundred bytes against a 8 KiB card block, and it stages
//! through `SCRATCH_XY`, which is per-frame projection scratch. Saving runs from
//! the pause menu with no render in flight, and the next world frame takes a
//! fresh projection token, so nothing can read the clobbered contents. That is
//! what keeps this feature at zero resident RAM: static headroom is 32 KiB and a
//! dedicated buffer would have spent a tenth of it for data that is live for a
//! few milliseconds.

use crate::{N_AMMO, N_WEAPONS};

/// Directory name on the card. The BIOS shows `title` in its manager; the file
/// name follows the retail `BExxx-xxxxx<id>` convention so a real console lists
/// it beside commercial saves.
#[cfg(any(test, target_arch = "mips"))]
const FILE_MANUAL: &str = "BESLES-00000HLPSX001";
#[cfg(any(test, target_arch = "mips"))]
const FILE_AUTO: &str = "BESLES-00000HLPSX002";

/// Two slots, because one is a data-loss bug waiting to happen: an autosave
/// firing at a level change would silently replace the save the player made
/// deliberately. They never write to the same file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Manual,
    Auto,
}

/// Which physical card port. Hardcoding port 1 meant a player whose slot-1
/// card was full simply could not save.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Port {
    One,
    Two,
}

/// A card port plus a slot: the four places a save can live.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub port: Port,
    pub slot: Slot,
}

impl Target {
    pub const ALL: [Target; 4] = [
        Target {
            port: Port::One,
            slot: Slot::Manual,
        },
        Target {
            port: Port::One,
            slot: Slot::Auto,
        },
        Target {
            port: Port::Two,
            slot: Slot::Manual,
        },
        Target {
            port: Port::Two,
            slot: Slot::Auto,
        },
    ];
    pub const fn label(self) -> &'static str {
        match (self.port, self.slot) {
            (Port::One, Slot::Manual) => "Card 1  Manual",
            (Port::One, Slot::Auto) => "Card 1  Autosave",
            (Port::Two, Slot::Manual) => "Card 2  Manual",
            (Port::Two, Slot::Auto) => "Card 2  Autosave",
        }
    }
}

impl Slot {
    #[cfg(any(test, target_arch = "mips"))]
    const fn file(self) -> &'static str {
        match self {
            Slot::Manual => FILE_MANUAL,
            Slot::Auto => FILE_AUTO,
        }
    }
    #[cfg(any(test, target_arch = "mips"))]
    const fn title(self) -> &'static str {
        match self {
            Slot::Manual => "HALF-LIFE",
            Slot::Auto => "HALF-LIFE AUTO",
        }
    }
}

/// Bumped whenever the field layout changes. A card written by an older build
/// is reported as absent rather than misread: every field here is raw state fed
/// straight back into the simulation, so a stale layout is not recoverable.
const SAVE_VERSION: u16 = 2;
const SAVE_MAGIC: u32 = u32::from_le_bytes(*b"HLSV");

/// Serialised checkpoint. Plain little-endian scalars written by hand rather
/// than a transmute: the layout is a file format that outlives any struct
/// padding decision the compiler makes.
#[derive(Clone, Copy)]
pub struct Checkpoint {
    pub room_id: u16,
    pub pos: [i32; 3],
    pub yaw: u16,
    pub pitch: i16,
    pub health: u16,
    pub armor: u16,
    pub suit: bool,
    pub crouch: bool,
    pub longjump: bool,
    pub owned: u16,
    pub current: u8,
    pub clips: [u16; N_WEAPONS],
    pub ammo: [u16; N_AMMO],
    pub global_hash: [u16; 32],
    pub global_on: [bool; 32],
    pub global_count: u16,
    /// Whether `pos` is meaningful. A manual save records where the player is
    /// standing; an autosave fires at a level change and the destination map's
    /// own spawn is the correct arrival, so it clears this rather than
    /// recording a position that belongs to the map being left.
    pub has_pos: bool,
    /// Monotonic write counter. With two slots and no clock on the console,
    /// this is what "most recent" means: each write takes the highest sequence
    /// on the card and adds one.
    pub sequence: u32,
}

impl Checkpoint {
    pub const EMPTY: Self = Self {
        room_id: 0,
        pos: [0; 3],
        yaw: 0,
        pitch: 0,
        health: 0,
        armor: 0,
        suit: false,
        crouch: false,
        longjump: false,
        owned: 0,
        current: 0,
        clips: [0; N_WEAPONS],
        ammo: [0; N_AMMO],
        global_hash: [0; 32],
        global_on: [false; 32],
        global_count: 0,
        has_pos: false,
        sequence: 0,
    };
}

/// Cursor-based writer. Every field is length-checked against the staging
/// buffer, so a future field that outgrows it truncates the save instead of
/// walking off the end of the scratch it borrows.
struct Writer<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl<'a> Writer<'a> {
    fn u8(&mut self, v: u8) {
        if self.at < self.buf.len() {
            self.buf[self.at] = v;
            self.at += 1;
        }
    }
    fn u16(&mut self, v: u16) {
        self.u8(v as u8);
        self.u8((v >> 8) as u8);
    }
    fn u32(&mut self, v: u32) {
        self.u16(v as u16);
        self.u16((v >> 16) as u16);
    }
    fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> u8 {
        let v = self.buf.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        u16::from(self.u8()) | (u16::from(self.u8()) << 8)
    }
    fn u32(&mut self) -> u32 {
        u32::from(self.u16()) | (u32::from(self.u16()) << 16)
    }
    fn i32(&mut self) -> i32 {
        self.u32() as i32
    }
}

/// Serialise into `buf`, returning the byte count. Ends with a sum check over
/// the payload: the card's own per-frame XOR catches media damage, this catches
/// a short or interleaved write.
pub fn encode(cp: &Checkpoint, buf: &mut [u8]) -> usize {
    let mut w = Writer { buf, at: 0 };
    w.u32(SAVE_MAGIC);
    w.u16(SAVE_VERSION);
    w.u16(cp.room_id);
    for c in cp.pos {
        w.i32(c);
    }
    w.u16(cp.yaw);
    w.u16(cp.pitch as u16);
    w.u16(cp.health);
    w.u16(cp.armor);
    w.u8(cp.suit as u8);
    w.u8(cp.crouch as u8);
    w.u8(cp.longjump as u8);
    w.u8(cp.current);
    w.u16(cp.owned);
    for v in cp.clips {
        w.u16(v);
    }
    for v in cp.ammo {
        w.u16(v);
    }
    for v in cp.global_hash {
        w.u16(v);
    }
    for v in cp.global_on {
        w.u8(v as u8);
    }
    w.u16(cp.global_count);
    w.u8(cp.has_pos as u8);
    w.u32(cp.sequence);
    let end = w.at;
    let sum = checksum(&w.buf[..end]);
    w.u32(sum);
    w.at
}

/// Parse `buf`, rejecting a wrong magic, a wrong version, or a bad checksum.
pub fn decode(buf: &[u8]) -> Option<Checkpoint> {
    let mut r = Reader { buf, at: 0 };
    if r.u32() != SAVE_MAGIC || r.u16() != SAVE_VERSION {
        return None;
    }
    let mut cp = Checkpoint::EMPTY;
    cp.room_id = r.u16();
    for c in cp.pos.iter_mut() {
        *c = r.i32();
    }
    cp.yaw = r.u16();
    cp.pitch = r.u16() as i16;
    cp.health = r.u16();
    cp.armor = r.u16();
    cp.suit = r.u8() != 0;
    cp.crouch = r.u8() != 0;
    cp.longjump = r.u8() != 0;
    cp.current = r.u8();
    cp.owned = r.u16();
    for v in cp.clips.iter_mut() {
        *v = r.u16();
    }
    for v in cp.ammo.iter_mut() {
        *v = r.u16();
    }
    for v in cp.global_hash.iter_mut() {
        *v = r.u16();
    }
    for v in cp.global_on.iter_mut() {
        *v = r.u8() != 0;
    }
    cp.global_count = r.u16();
    cp.has_pos = r.u8() != 0;
    cp.sequence = r.u32();
    let end = r.at;
    let stored = r.u32();
    if r.at > buf.len() || stored != checksum(&buf[..end]) {
        return None;
    }
    Some(cp)
}

fn checksum(bytes: &[u8]) -> u32 {
    // FNV-1a: the same hash the cook and runtime already share for globalnames,
    // so there is one checksum idiom in the project rather than two.
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Outcome of a card operation, kept coarse: the pause menu shows one line and
/// the player's next action is the same for every failure -- check the card.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardResult {
    Ok,
    NoCard,
    Failed,
}

#[cfg(target_arch = "mips")]
fn with_card<T>(port: Port, f: impl FnOnce(&mut psx_mc::Card<psx_mc::HardwareCard>) -> T) -> T {
    let hw = match port {
        Port::One => psx_mc::Slot::One,
        Port::Two => psx_mc::Slot::Two,
    };
    let mut card = psx_mc::Card::new(psx_mc::HardwareCard::new(hw));
    f(&mut card)
}

/// Write the checkpoint to port 1, formatting an unformatted card first.
#[cfg(target_arch = "mips")]
#[inline(never)]
#[optimize(size)]
pub fn write(target: Target, cp: &Checkpoint, staging: &mut [u8]) -> CardResult {
    let next = latest_sequence(staging).wrapping_add(1);
    let mut stamped = *cp;
    stamped.sequence = next;
    let len = encode(&stamped, staging);
    with_card(target.port, |card| match card.is_formatted() {
        Err(_) => CardResult::NoCard,
        Ok(formatted) => {
            if !formatted && card.format().is_err() {
                return CardResult::Failed;
            }
            match card.write(target.slot.file(), target.slot.title(), &staging[..len]) {
                Ok(()) => CardResult::Ok,
                Err(_) => CardResult::Failed,
            }
        }
    })
}

/// Highest sequence already on the card, 0 when neither slot is readable.
#[cfg(target_arch = "mips")]
fn latest_sequence(staging: &mut [u8]) -> u32 {
    let mut best = 0;
    for target in Target::ALL {
        if let Ok(cp) = read(target, staging) {
            best = best.max(cp.sequence);
        }
    }
    best
}

/// The most recently written slot. This is what a bare "Load" resumes: with no
/// clock on the console the write counter is the only ordering available.
#[cfg(target_arch = "mips")]
pub fn read_latest(staging: &mut [u8]) -> Result<Checkpoint, CardResult> {
    let mut best: Option<Checkpoint> = None;
    let mut err = CardResult::Failed;
    for target in Target::ALL {
        match read(target, staging) {
            Ok(cp) => {
                if best.map_or(true, |b| cp.sequence > b.sequence) {
                    best = Some(cp);
                }
            }
            // A missing card in one port is not a failure when the other has a save.
            Err(e) => err = e,
        }
    }
    best.ok_or(err)
}

/// Read the checkpoint back. A missing file and a corrupt one both report
/// `Failed`: there is nothing the player can do differently between them.
#[cfg(target_arch = "mips")]
#[inline(never)]
#[optimize(size)]
pub fn read(target: Target, staging: &mut [u8]) -> Result<Checkpoint, CardResult> {
    with_card(target.port, |card| match card.is_formatted() {
        Err(_) => Err(CardResult::NoCard),
        Ok(false) => Err(CardResult::Failed),
        Ok(true) => match card.read(target.slot.file(), staging) {
            Ok(len) => decode(&staging[..len]).ok_or(CardResult::Failed),
            Err(_) => Err(CardResult::Failed),
        },
    })
}

#[cfg(not(target_arch = "mips"))]
pub fn write(_target: Target, _cp: &Checkpoint, _staging: &mut [u8]) -> CardResult {
    CardResult::NoCard
}

#[cfg(not(target_arch = "mips"))]
pub fn read(_target: Target, _staging: &mut [u8]) -> Result<Checkpoint, CardResult> {
    Err(CardResult::NoCard)
}

#[cfg(not(target_arch = "mips"))]
pub fn read_latest(_staging: &mut [u8]) -> Result<Checkpoint, CardResult> {
    Err(CardResult::NoCard)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Checkpoint {
        let mut cp = Checkpoint::EMPTY;
        cp.room_id = 42;
        cp.pos = [-1234, 5678, -90];
        cp.yaw = 0x0abc;
        cp.pitch = -700;
        cp.health = 87;
        cp.armor = 33;
        cp.suit = true;
        cp.longjump = true;
        cp.owned = 0b101;
        cp.current = 3;
        cp.clips[3] = 25;
        cp.ammo[1] = 150;
        cp.global_hash[2] = 1698;
        cp.global_on[2] = true;
        cp.global_count = 3;
        cp.has_pos = true;
        cp.sequence = 7;
        cp
    }

    #[test]
    fn round_trips_every_field() {
        let mut buf = [0u8; 512];
        let len = encode(&sample(), &mut buf);
        let back = decode(&buf[..len]).expect("decodes");
        let cp = sample();
        assert_eq!(back.room_id, cp.room_id);
        assert_eq!(back.pos, cp.pos);
        assert_eq!(back.yaw, cp.yaw);
        assert_eq!(back.pitch, cp.pitch);
        assert_eq!(back.health, cp.health);
        assert_eq!(back.armor, cp.armor);
        assert!(back.suit && back.longjump && !back.crouch);
        assert_eq!(back.owned, cp.owned);
        assert_eq!(back.current, cp.current);
        assert_eq!(back.clips, cp.clips);
        assert_eq!(back.ammo, cp.ammo);
        assert_eq!(back.global_hash, cp.global_hash);
        assert_eq!(back.global_on[2], true);
        assert_eq!(back.global_count, cp.global_count);
        assert_eq!(back.sequence, cp.sequence);
        assert!(back.has_pos);
    }

    #[test]
    fn rejects_corruption_and_stale_layouts() {
        let mut buf = [0u8; 512];
        let len = encode(&sample(), &mut buf);
        // A flipped payload byte must not decode: the player would otherwise
        // resume with silently wrong ammo or globals.
        buf[16] ^= 0x40;
        assert!(decode(&buf[..len]).is_none());
        buf[16] ^= 0x40;
        assert!(decode(&buf[..len]).is_some());
        buf[4] = 0xff; // version
        assert!(decode(&buf[..len]).is_none());
        buf[0] = 0; // magic
        assert!(decode(&buf[..len]).is_none());
    }

    #[test]
    fn slots_are_distinct_files_so_an_autosave_cannot_clobber_a_manual_one() {
        assert_ne!(Slot::Manual.file(), Slot::Auto.file());
        assert_ne!(Slot::Manual.title(), Slot::Auto.title());
        // Every target must be distinguishable in the browser.
        for (i, a) in Target::ALL.iter().enumerate() {
            for b in Target::ALL.iter().skip(i + 1) {
                assert_ne!(a.label(), b.label());
            }
        }
    }

    #[test]
    fn payload_fits_one_card_block_with_room_to_spare() {
        let mut buf = [0u8; 512];
        let len = encode(&sample(), &mut buf);
        assert!(len < 8192, "one block is 8192 bytes, payload is {len}");
        assert!(len < buf.len(), "sample must fit the test buffer");
    }
}
