/// Generational handle types.
///
/// A `Handle` is a lightweight, copyable identifier that remains safe in the
/// presence of deletion via a generation counter.
///
/// - `index` selects a slot.
/// - `generation` must match the slot's current generation for the handle to be valid.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Handle(u32, u32); // (index, generation)

impl Handle {
    #[inline]
    pub fn new(index: u32, generation: u32) -> Self {
        Handle(index, generation)
    }

    #[inline]
    pub fn index(self) -> u32 {
        self.0
    }

    #[inline]
    pub fn generation(self) -> u32 {
        self.1
    }
}

/// Allocator for generational handles with free-list reuse.
///
/// This is intentionally simple and deterministic:
/// - Allocation prefers reusing indices from the free list (LIFO).
/// - Free increments the slot generation and puts the index back on the free list.
#[derive(Debug, Default)]
pub struct HandleAllocator {
    generations: Vec<u32>,
    free: Vec<u32>,
}

impl HandleAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a new handle.
    pub fn alloc(&mut self) -> Handle {
        if let Some(index) = self.free.pop() {
            let generation = self.generations[index as usize];
            return Handle::new(index, generation);
        }

        let index = self.generations.len() as u32;
        self.generations.push(0);
        Handle::new(index, 0)
    }

    /// Returns true if the handle refers to a live slot.
    pub fn is_valid(&self, h: Handle) -> bool {
        let Some(&generation) = self.generations.get(h.index() as usize) else {
            return false;
        };
        generation == h.generation()
    }

    /// Free a handle. Returns `true` if it was valid and is now freed.
    ///
    /// Invalid or already-freed handles are rejected and return `false`.
    pub fn free(&mut self, h: Handle) -> bool {
        let idx = h.index() as usize;
        let Some(generation) = self.generations.get_mut(idx) else {
            return false;
        };
        if *generation != h.generation() {
            return false;
        }

        *generation = generation.wrapping_add(1);
        self.free.push(h.index());
        true
    }

    pub fn capacity(&self) -> usize {
        self.generations.len()
    }

    pub fn free_len(&self) -> usize {
        self.free.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{Handle, HandleAllocator};

    #[test]
    fn handle_accessors() {
        let handle = Handle::new(7, 2);
        assert_eq!(handle.index(), 7);
        assert_eq!(handle.generation(), 2);
    }

    #[test]
    fn allocates_and_validates() {
        let mut a = HandleAllocator::new();
        let h0 = a.alloc();
        let h1 = a.alloc();
        assert!(a.is_valid(h0));
        assert!(a.is_valid(h1));
        assert_ne!(h0.index(), h1.index());
    }

    #[test]
    fn free_increments_generation_and_reuses_index() {
        let mut a = HandleAllocator::new();
        let h0 = a.alloc();
        assert!(a.free(h0));
        assert!(!a.is_valid(h0));

        let h0b = a.alloc();
        assert_eq!(h0b.index(), h0.index());
        assert_ne!(h0b.generation(), h0.generation());
        assert!(a.is_valid(h0b));
    }

    #[test]
    fn double_free_is_rejected() {
        let mut a = HandleAllocator::new();
        let h = a.alloc();
        assert!(a.free(h));
        assert!(!a.free(h));
    }
}
