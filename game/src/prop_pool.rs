//! Keyed state pools for per-prop state that only a few props ever use.
//!
//! A per-prop array costs one row per prop on every map whether or not the map
//! places a single prop that needs it. A pool holds `N` state rows plus a
//! one-byte owner tag each, where `N` is the most props of that kind any room
//! of the whole game places (the cook scan in `game/build.rs` measures it and
//! sizes the pool). The pool is for state that:
//!
//! * is read and written only by code that already knows the prop's kind, and
//! * has an all-zero empty value (so the static lands in `.bss`).
//!
//! State shared by every walker (health, position, the jump momentum) stays in
//! the per-prop arrays. See `docs/engine/RAM.md` for the recipe.

/// `N` rows of `T`, each claimed by at most one prop index (0..=254).
#[derive(Clone, Copy)]
pub struct PropPool<T: Copy, const N: usize> {
    /// Owning prop index plus one; zero marks a free row.
    owner: [u8; N],
    state: [T; N],
}

impl<T: Copy, const N: usize> PropPool<T, N> {
    /// An empty pool whose rows hold `empty`. Use an all-zero `empty` so the
    /// static is placed in `.bss`.
    pub const fn new(empty: T) -> Self {
        Self {
            owner: [0; N],
            state: [empty; N],
        }
    }

    fn row(&self, prop: usize) -> Option<usize> {
        let tag = (prop + 1) as u8;
        let mut row = 0;
        while row < N {
            if self.owner[row] == tag {
                return Some(row);
            }
            row += 1;
        }
        None
    }

    /// The prop's state, or `None` when it has not claimed a row (its state is
    /// then the empty value).
    pub fn get(&self, prop: usize) -> Option<&T> {
        self.row(prop).map(|row| &self.state[row])
    }

    /// The prop's row, claiming a free one set to `empty` first. `None` only
    /// when all `N` rows belong to other props, which the cook census rules out.
    pub fn claim(&mut self, prop: usize, empty: T) -> Option<&mut T> {
        let row = match self.row(prop) {
            Some(row) => row,
            None => {
                let mut free = 0;
                while free < N && self.owner[free] != 0 {
                    free += 1;
                }
                if free == N {
                    return None;
                }
                self.owner[free] = (prop + 1) as u8;
                self.state[free] = empty;
                free
            }
        };
        Some(&mut self.state[row])
    }

    /// Frees the prop's row; its state reads as empty again.
    pub fn release(&mut self, prop: usize) {
        if let Some(row) = self.row(prop) {
            self.owner[row] = 0;
        }
    }

    /// Frees every row (map load).
    pub fn clear(&mut self) {
        self.owner = [0; N];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prop_keeps_its_row_until_released() {
        let mut pool = PropPool::<u16, 2>::new(0);
        *pool.claim(7, 0).unwrap() = 11;
        *pool.claim(9, 0).unwrap() = 22;
        assert_eq!(pool.get(7), Some(&11));
        assert_eq!(*pool.claim(7, 0).unwrap(), 11);
        assert!(pool.claim(3, 0).is_none());
        pool.release(7);
        assert_eq!(pool.get(7), None);
        assert_eq!(*pool.claim(3, 5).unwrap(), 5);
        assert_eq!(pool.get(9), Some(&22));
    }

    #[test]
    fn clear_frees_everything_and_prop_zero_is_a_valid_key() {
        let mut pool = PropPool::<u8, 3>::new(0);
        *pool.claim(0, 0).unwrap() = 1;
        assert_eq!(pool.get(0), Some(&1));
        pool.clear();
        assert_eq!(pool.get(0), None);
    }
}
