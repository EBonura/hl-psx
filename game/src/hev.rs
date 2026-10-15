//! The HEV suit's voice: what makes it speak, in what order, and how often.
//!
//! The rules are player.cpp's. `SetSuitUpdate` puts a sentence in a queue of
//! four unless the same sentence is still inside its no-repeat time;
//! `CheckSuitUpdate` plays the oldest one 0.1 s after the queue was empty and
//! every 3.5 s while it is not. `TakeDamage` chooses the sentences from the
//! kind and size of the hit and the health before and after it.
//!
//! A sentence is a short run of recorded words (`hl_format::hev`). The map's
//! voice chunk carries the words its entities can make the suit say after the
//! dialogue pack, ending in the block's offset; [`load`] uploads them above
//! the dialogue and the speaker plays them one after the other on the
//! charger's voice, which only a wall charger in use shares. A sentence whose
//! words the map does not carry is never asked for: it takes no place in the
//! queue and no no-repeat time.
//!
//! RAM is the constraint here: no search loops, no per-word tables beyond the
//! three small arrays, and every routine is built for size.

use hl_format::hev as table;
use hl_format::hev::{NO_WORD, SENTENCES, SENTENCE_COUNT, WORD_COUNT};
use psx_sfx::{OneShot, Sample, PARKING_TAIL};
use psx_spu::{Spu, SpuAddr, Voice, Volume};

/// CSUITPLAYLIST: sentences waiting.
const QUEUE: usize = 4;
/// SUITUPDATETIME 3.5 s and SUITFIRSTUPDATETIME 0.1 s at 20 Hz.
const UPDATE_TICKS: u32 = 70;
const FIRST_UPDATE_TICKS: u32 = 2;
/// The voice a sentence plays on (the charger's).
const VOICE: u8 = 16;
/// `suitvolume` is 0.25 in valve.rc, but the words are cooked at full scale
/// and at a third of the rate, so they are played a third down.
const VOLUME_DEN: u16 = 3;

/// What the player was hit by, as `TakeDamage` tells the kinds apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    Generic = 0,
    Fall,
    Crush,
    Bullet,
    Slash,
    Sonic,
    Poison,
    Acid,
    NerveGas,
    Radiation,
    Drown,
}

const NONE: u8 = u8::MAX;

/// Per kind: the sentence for a small hit and for a major one (more than 25
/// damage), and the seconds before it may be said again (player.cpp
/// TakeDamage's `SetSuitUpdate` calls).
const REPORTS: [(u8, u8, u16); 11] = [
    (NONE, NONE, 0),                                                // generic
    (table::S_DMG4, table::S_DMG5, table::NEXT_IN_30SEC),           // fall
    (table::S_DMG4, table::S_DMG5, table::NEXT_IN_30SEC),           // crush
    (table::S_DMG6, table::S_DMG6, table::NEXT_IN_30SEC),           // bullet
    (table::S_DMG0, table::S_DMG1, table::NEXT_IN_30SEC),           // slash
    (NONE, table::S_DMG2, table::NEXT_IN_1MIN),                     // sonic
    (table::S_DMG3, table::S_DMG3, table::NEXT_IN_1MIN),            // poison
    (table::S_DET1, table::S_DET1, table::NEXT_IN_1MIN),            // acid
    (table::S_DET0, table::S_DET0, table::NEXT_IN_1MIN),            // nerve gas
    (table::S_DET2, table::S_DET2, table::NEXT_IN_1MIN),            // radiation
    (NONE, NONE, 0),                                                // drowning
];

impl Kind {
    /// DMG_TIMEBASED: poison, nerve gas, radiation, acid.
    pub const fn time_based(self) -> bool {
        matches!(self as u8, 6..=9)
    }
}

/// The queue and the no-repeat times.
pub struct Suit {
    /// Sentence + 1 per slot, 0 empty (m_rgSuitPlayList).
    queue: [u8; QUEUE],
    next: u8,
    /// m_flSuitUpdate in ticks of `clock`; 0 = no sentence is waiting.
    update_at: u32,
    clock: u32,
    /// Per sentence, when it may be said again in 2-tick units (0 = free).
    /// The SDK keeps a list of 32; one slot per sentence needs no search.
    repeat_until: [u16; SENTENCE_COUNT],
}

impl Suit {
    pub const fn new() -> Self {
        Self {
            queue: [0; QUEUE],
            next: 0,
            update_at: 0,
            clock: 4,
            repeat_until: [0; SENTENCE_COUNT],
        }
    }

    /// `SetSuitUpdate(NULL)`: forget what was waiting (death, a new life).
    pub fn clear_queue(&mut self) {
        self.queue = [0; QUEUE];
    }

    /// SetSuitUpdate: queue `sentence` unless it was said within
    /// `repeat_secs` seconds (0 = it may repeat at once).
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn request(&mut self, sentence: u8, repeat_secs: u16) {
        let now = (self.clock >> 1) as u16 | 1;
        let until = &mut self.repeat_until[sentence as usize];
        if *until != 0 && (until.wrapping_sub(now) as i16) > 0 {
            return;
        }
        // 0 marks a free slot; a time that lands on it moves one unit on.
        *until = if repeat_secs == 0 {
            0
        } else {
            now.wrapping_add(repeat_secs * 10) | 1
        };
        self.queue[self.next as usize] = sentence + 1;
        self.next = (self.next + 1) % QUEUE as u8;
        if self.update_at <= self.clock {
            self.update_at = self.clock
                + if self.update_at == 0 {
                    FIRST_UPDATE_TICKS
                } else {
                    UPDATE_TICKS
                };
        }
    }

    /// CheckSuitUpdate for one tick. `idle` is false while a sentence is
    /// still being spoken (the SDK lets them overlap on a static channel; one
    /// SPU voice cannot, so the next one waits for the end). Returns the
    /// sentence to start.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn tick(&mut self, idle: bool) -> Option<u8> {
        self.clock += 1;
        if self.update_at == 0 || self.clock < self.update_at || !idle {
            return None;
        }
        let mut tries = 0;
        while tries < QUEUE {
            let at = (self.next as usize + tries) % QUEUE;
            if self.queue[at] != 0 {
                let sentence = self.queue[at] - 1;
                self.queue[at] = 0;
                self.update_at = self.clock + UPDATE_TICKS;
                return Some(sentence);
            }
            tries += 1;
        }
        self.update_at = 0;
        None
    }
}

/// What `TakeDamage` asks of the suit for one hit. `before` and `after` are
/// the health around it, `amount` the damage before the suit's armour took its
/// share (m_lastDamageAmount), `chance` one 1-in-4 roll (RANDOM_LONG(0, 3) == 0).
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub fn react(
    kind: Kind,
    amount: u16,
    before: u16,
    after: u16,
    chance: bool,
    mut say: impl FnMut(u8, u16),
) {
    // TakeDamage reports whenever damage was dealt, even when the armour took it all.
    if amount == 0 {
        return;
    }
    let trivial = after > 75 || amount < 5;
    let major = amount > 25;
    let timed = kind.time_based();
    if !trivial || timed {
        let (small, big, secs) = REPORTS[kind as usize];
        let sentence = if major { big } else { small };
        // Bullets only count above 5 damage.
        if sentence != NONE && (kind != Kind::Bullet || amount > 5) {
            say(sentence, secs);
        }
    }
    if !trivial {
        if major && before >= 75 {
            // The first big hit: the automedic switches on and gives a shot.
            say(table::S_MED1, table::NEXT_IN_30MIN);
            say(table::S_HEAL7, table::NEXT_IN_30MIN);
        }
        if after < 30 && before < 75 {
            if after < 6 {
                say(table::S_HLTH3, table::NEXT_IN_10MIN);
            } else if after < 20 {
                say(table::S_HLTH2, table::NEXT_IN_10MIN);
            }
            if chance && before < 50 {
                say(table::S_DMG7, table::NEXT_IN_5MIN);
            }
        }
    }
    if timed && before < 75 {
        if before >= 50 {
            say(table::S_HLTH1, table::NEXT_IN_10MIN);
        } else if chance {
            say(table::S_DMG7, table::NEXT_IN_5MIN);
        }
    }
}

// ---- the words in SPU RAM ----

/// Where each word of the table sits in SPU RAM, in 8 byte units (0 = the map
/// does not carry it), at what rate, and how many ticks it lasts.
static mut WORD_ADDR: [u16; WORD_COUNT] = [0; WORD_COUNT];
static mut WORD_RATE: [u16; WORD_COUNT] = [0; WORD_COUNT];
static mut WORD_TICKS: [u8; WORD_COUNT] = [0; WORD_COUNT];
/// Bit per sentence: every word of it is loaded.
static mut READY: u64 = 0;

static mut SUIT: Suit = Suit::new();
/// Sentence being spoken (`NO_WORD` = none), its next part and when the
/// current part ends.
static mut SPEAKING: u8 = NO_WORD;
static mut PART: u8 = 0;
static mut PART_END: u32 = 0;
static mut TICK: u32 = 0;
/// What the next hit on the player is, set by whoever deals it just before it
/// lands (and taken by the damage routine, so it never leaks to another hit).
static mut NEXT_KIND: Kind = Kind::Generic;

pub unsafe fn set_kind(kind: Kind) {
    NEXT_KIND = kind;
}

pub unsafe fn take_kind() -> Kind {
    let kind = NEXT_KIND;
    NEXT_KIND = Kind::Generic;
    kind
}

/// What a `trigger_hurt` record deals (see `hl_format::hev::hurt_kind_bits`).
pub fn hurt_kind(flags: u8) -> Kind {
    if flags & 1 != 0 {
        return Kind::Radiation;
    }
    match (flags & table::HURT_KIND_MASK) >> table::HURT_KIND_SHIFT {
        table::HURT_CRUSH => Kind::Crush,
        table::HURT_FALL => Kind::Fall,
        table::HURT_POISON => Kind::Poison,
        table::HURT_ACID => Kind::Acid,
        table::HURT_NERVEGAS => Kind::NerveGas,
        _ => Kind::Generic,
    }
}

/// What an actor of cooked type `ty` hurts the player with (the cooker's
/// `base_actor_type` numbering); the SDK's damage type for its attack.
pub const fn actor_kind(ty: u8) -> Kind {
    match ty {
        // headcrab, zombie, bullsquid bite, leech, ichthyosaur, Gonarch
        2 | 5 | 7 | 13 | 18 | 19 => Kind::Slash,
        6 => Kind::Sonic,
        // grunts, Barney, the sentries, turrets and the Apache fire bullets
        1 | 8 | 10 | 20 | 21 | 22 | 23 => Kind::Bullet,
        _ => Kind::Generic,
    }
}

#[inline(always)]
fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap_or([0; 4]))
}

/// The block's offset: the chunk's last word, when it points at the magic.
fn block_at(chunk: &[u8]) -> Option<usize> {
    let n = chunk.len();
    if n < 16 {
        return None;
    }
    let at = rd_u32(chunk, n - 4) as usize;
    (at + 12 <= n && chunk[at..at + 4] == table::MAGIC).then_some(at)
}

/// SPU RAM the chunk's suit block takes (0 when it has none), so the room
/// effect's workspace can stay clear of it.
pub fn spu_bytes(chunk: &[u8]) -> u32 {
    block_at(chunk).map_or(0, |at| rd_u32(chunk, at + 8))
}

/// Forget the previous map's words and sentences.
#[inline(never)]
pub unsafe fn unload() {
    // The words are about to be overwritten; silence a sentence in flight.
    let voice = Voice::new(VOICE);
    voice.set_volume(Volume::SILENCE, Volume::SILENCE);
    Voice::release(voice.mask());
    WORD_ADDR = [0; WORD_COUNT];
    READY = 0;
    SPEAKING = NO_WORD;
    SUIT.clear_queue();
}

/// Upload the chunk's suit block to SPU RAM from `base` up (16 byte aligned,
/// the first free address above the dialogue pack). The cooker checked the
/// block and its fit, so nothing is validated again here.
///
/// # Safety
/// Serialized with every other user of the SPU voices and RAM.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub unsafe fn load(spu: &mut Spu, chunk: &[u8], base: u32) {
    unload();
    let Some(at) = block_at(chunk) else { return };
    let block = &chunk[at..];
    let mut next = base;
    let mut word = 0;
    while word < WORD_COUNT {
        let entry = table::directory_bytes(0) + word * table::ENTRY_BYTES;
        let (off, len) = (rd_u32(block, entry) as usize, rd_u32(block, entry + 4) as usize);
        if len != 0 {
            // A PSAU sample: 12 byte asset header, 20 byte audio header (rate
            // at +4, samples at +8, ADPCM blocks at +12), then the blocks.
            let sample = &block[off..off + len];
            let rate = rd_u32(sample, 16);
            WORD_ADDR[word] = (next / 8) as u16;
            WORD_RATE[word] = rate as u16;
            WORD_TICKS[word] = ((rd_u32(sample, 20) * 20 + rate - 1) / rate).min(255) as u8;
            spu.upload_adpcm(SpuAddr::new(next), &sample[32..]);
            next += (len as u32 - 32 + 15) & !15;
            spu.upload_adpcm(SpuAddr::new(next), &PARKING_TAIL);
            next += PARKING_TAIL.len() as u32;
        }
        word += 1;
    }
    let mut sentence = 0;
    while sentence < SENTENCE_COUNT {
        let mut ready = true;
        let mut part = 0;
        while part < table::MAX_PARTS {
            let w = SENTENCES[sentence][part];
            ready &= w == NO_WORD || WORD_ADDR[w as usize] != 0;
            part += 1;
        }
        READY |= (ready as u64) << sentence;
        sentence += 1;
    }
}

/// Is `sentence` one this map can speak?
pub unsafe fn can_say(sentence: u8) -> bool {
    READY & (1 << sentence) != 0
}

/// SetSuitUpdate for a sentence of the table. The suit must be worn.
pub unsafe fn request(sentence: u8, repeat_secs: u16) {
    if can_say(sentence) {
        SUIT.request(sentence, repeat_secs);
    }
}

/// Speak `sentence` at once, outside the queue (EMIT_SOUND_SUIT).
pub unsafe fn say_now(sentence: u8) {
    if can_say(sentence) {
        SPEAKING = sentence;
        PART = 0;
        PART_END = TICK;
    }
}

pub unsafe fn clear_queue() {
    SUIT.clear_queue();
}

/// One simulation tick: start the next waiting sentence, move a running one
/// on to its next word.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub unsafe fn tick(worn: bool) {
    TICK += 1;
    if !worn {
        SUIT.clear_queue();
    } else if let Some(sentence) = SUIT.tick(SPEAKING == NO_WORD) {
        SPEAKING = sentence;
        PART = 0;
        PART_END = TICK;
    }
    if SPEAKING == NO_WORD || TICK < PART_END {
        return;
    }
    let word = if (PART as usize) < table::MAX_PARTS {
        SENTENCES[SPEAKING as usize][PART as usize]
    } else {
        NO_WORD
    };
    if word == NO_WORD {
        SPEAKING = NO_WORD;
        return;
    }
    #[cfg(feature = "reference-trace")]
    crate::reference_trace::hev(TICK as u16, SPEAKING, PART, word);
    let w = word as usize;
    OneShot::new(
        Sample::resident(
            SpuAddr::new(WORD_ADDR[w] as u32 * 8),
            WORD_RATE[w] as u32,
            0,
        ),
        Volume::linear(1, VOLUME_DEN),
    )
    .play(Voice::new(VOICE));
    PART += 1;
    PART_END = TICK + WORD_TICKS[w] as u32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(kind: Kind, amount: u16, before: u16, after: u16, chance: bool) -> Vec<(u8, u16)> {
        let mut out = Vec::new();
        react(kind, amount, before, after, chance, |s, t| out.push((s, t)));
        out
    }

    #[test]
    fn a_small_hit_at_high_health_is_trivial() {
        assert!(said(Kind::Bullet, 4, 100, 96, true).is_empty());
        assert!(said(Kind::Slash, 20, 100, 80, true).is_empty());
    }

    #[test]
    fn the_first_big_hit_names_the_injury_then_starts_the_automedic() {
        // 30 damage by a fall, 100 -> 70 health.
        assert_eq!(
            said(Kind::Fall, 30, 100, 70, false),
            vec![
                (table::S_DMG5, 30),
                (table::S_MED1, 1800),
                (table::S_HEAL7, 1800)
            ]
        );
        // 12 damage by a fall is a minor fracture, and nothing else, at 70 HP.
        assert_eq!(said(Kind::Fall, 12, 82, 70, false), vec![(table::S_DMG4, 30)]);
        assert_eq!(said(Kind::Bullet, 6, 60, 54, false), vec![(table::S_DMG6, 30)]);
        assert!(said(Kind::Bullet, 5, 60, 55, false).len() == 0);
    }

    #[test]
    fn health_warnings_follow_the_new_health() {
        assert_eq!(said(Kind::Generic, 10, 40, 18, false), vec![(table::S_HLTH2, 600)]);
        assert_eq!(said(Kind::Generic, 10, 15, 5, false), vec![(table::S_HLTH3, 600)]);
        // Below 50 a 1-in-4 roll adds "seek medical attention".
        assert_eq!(
            said(Kind::Generic, 10, 40, 18, true),
            vec![(table::S_HLTH2, 600), (table::S_DMG7, 300)]
        );
        assert_eq!(said(Kind::Generic, 10, 60, 18, true), vec![(table::S_HLTH2, 600)]);
    }

    #[test]
    fn hazards_report_even_when_the_hit_is_small() {
        // Time-based damage ignores `trivial`.
        assert_eq!(said(Kind::Radiation, 2, 100, 98, false), vec![(table::S_DET2, 60)]);
        assert_eq!(said(Kind::Acid, 3, 100, 97, false), vec![(table::S_DET1, 60)]);
        assert_eq!(said(Kind::Radiation, 1, 70, 69, false), vec![(table::S_DET2, 60), (table::S_HLTH1, 600)]);
    }

    #[test]
    fn a_sentence_waits_a_tenth_of_a_second_then_every_three_and_a_half() {
        let mut suit = Suit::new();
        suit.request(3, 0);
        suit.request(5, 0);
        let mut played = Vec::new();
        for t in 1..=200 {
            if let Some(s) = suit.tick(true) {
                played.push((t, s));
            }
        }
        // Requested on tick 0: the first plays two ticks on, the second 70
        // later.
        assert_eq!(played, vec![(2, 3), (72, 5)]);
    }

    #[test]
    fn a_sentence_is_not_repeated_inside_its_time() {
        let mut suit = Suit::new();
        suit.request(7, 30);
        let mut count = 0;
        for t in 0..1000 {
            if t == 100 || t == 400 {
                suit.request(7, 30);
            }
            if suit.tick(true).is_some() {
                count += 1;
            }
        }
        // 30 s = 600 ticks: the request at 100 and at 400 are swallowed.
        assert_eq!(count, 1);
        suit.request(7, 30);
        let mut again = 0;
        for _ in 0..100 {
            if suit.tick(true).is_some() {
                again += 1;
            }
        }
        assert_eq!(again, 1);
    }

    #[test]
    fn a_busy_speaker_holds_the_next_sentence() {
        let mut suit = Suit::new();
        suit.request(1, 0);
        for _ in 0..50 {
            assert_eq!(suit.tick(false), None);
        }
        assert_eq!(suit.tick(true), Some(1));
    }

    #[test]
    fn the_queue_keeps_four_and_drops_the_oldest() {
        let mut suit = Suit::new();
        for s in 0..6 {
            suit.request(s, 0);
        }
        let mut order = Vec::new();
        for _ in 0..400 {
            if let Some(s) = suit.tick(true) {
                order.push(s);
            }
        }
        assert_eq!(order, vec![2, 3, 4, 5]);
    }

    #[test]
    fn the_block_is_found_by_the_chunk_footer() {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(b"HSFX");
        chunk.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(block_at(&chunk), None);
        chunk.extend_from_slice(b"HEVB");
        chunk.extend_from_slice(&[0, 0, 0, 0]);
        chunk.extend_from_slice(&1234u32.to_le_bytes());
        chunk.extend_from_slice(&8u32.to_le_bytes());
        assert_eq!(block_at(&chunk), Some(8));
        assert_eq!(spu_bytes(&chunk), 1234);
        // A chunk with a stray last word is not a block.
        chunk[23] = 9;
        assert_eq!(block_at(&chunk), None);
    }
}
