use crate::handles::Handle;

#[derive(Debug)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// A simple, deterministic generational arena.
///
/// Properties:
/// - Allocation reuses freed indices via a LIFO free-list.
/// - Handles are validated by `(index, generation)`.
/// - Iteration order is stable (ascending index).
///
/// Note: A `Handle` is only meaningful within the arena it came from.
#[derive(Debug)]
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    live: usize,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
        }
    }
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Allocate a new value and return its handle.
    pub fn alloc(&mut self, value: T) -> Handle {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            debug_assert!(slot.value.is_none());
            slot.value = Some(value);
            self.live += 1;
            return Handle::new(index, slot.generation);
        }

        let index = self.slots.len() as u32;
        self.slots.push(Slot {
            generation: 0,
            value: Some(value),
        });
        self.live += 1;
        Handle::new(index, 0)
    }

    pub fn is_valid(&self, h: Handle) -> bool {
        let Some(slot) = self.slots.get(h.index() as usize) else {
            return false;
        };
        slot.generation == h.generation() && slot.value.is_some()
    }

    pub fn get(&self, h: Handle) -> Option<&T> {
        let slot = self.slots.get(h.index() as usize)?;
        if slot.generation != h.generation() {
            return None;
        }
        slot.value.as_ref()
    }

    pub fn get_mut(&mut self, h: Handle) -> Option<&mut T> {
        let slot = self.slots.get_mut(h.index() as usize)?;
        if slot.generation != h.generation() {
            return None;
        }
        slot.value.as_mut()
    }

    /// Remove a value. Returns `None` if the handle is invalid.
    pub fn free(&mut self, h: Handle) -> Option<T> {
        let slot = self.slots.get_mut(h.index() as usize)?;
        if slot.generation != h.generation() {
            return None;
        }
        let v = slot.value.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(h.index());
        self.live -= 1;
        Some(v)
    }

    /// Iterate live entries in ascending index order.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, slot)| {
            let v = slot.value.as_ref()?;
            Some((Handle::new(i as u32, slot.generation), v))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Arena;

    #[test]
    fn alloc_get_free_reuse() {
        let mut a = Arena::new();
        let h0 = a.alloc("a");
        let h1 = a.alloc("b");
        assert_eq!(a.get(h0), Some(&"a"));
        assert_eq!(a.get(h1), Some(&"b"));
        assert!(a.is_valid(h0));

        assert_eq!(a.free(h0), Some("a"));
        assert!(!a.is_valid(h0));
        assert!(a.get(h0).is_none());

        let h0b = a.alloc("c");
        assert_eq!(h0b.index(), h0.index());
        assert_ne!(h0b.generation(), h0.generation());
        assert_eq!(a.get(h0b), Some(&"c"));
    }

    #[test]
    fn iter_is_stable_by_index() {
        let mut a = Arena::new();
        let h0 = a.alloc(10);
        let h1 = a.alloc(20);
        let h2 = a.alloc(30);
        a.free(h1);
        let _h1b = a.alloc(40);

        let ids: Vec<u32> = a.iter().map(|(h, _)| h.index()).collect();
        // All live entries in ascending index order.
        assert_eq!(ids, vec![h0.index(), h1.index(), h2.index()]);
    }
}
