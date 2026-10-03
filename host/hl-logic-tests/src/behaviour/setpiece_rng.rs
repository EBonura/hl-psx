//! The random source the set-piece behaviour tests feed their brains.

/// The game's shared impact generator: the 32-bit glibc LCG, high half
/// folded over the low half, reduced by remainder. Goldens seed it with the
/// game's own seed so the recorded draws are the ones a real run makes.
#[derive(Clone, Copy, Debug)]
pub struct Lcg(pub u32);

impl Lcg {
    /// The seed the game starts its impact generator with.
    pub const GAME_SEED: u32 = 0x484c_5058;

    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
        (self.0 ^ (self.0 >> 16)) % n
    }
}
