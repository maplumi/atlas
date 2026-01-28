use crate::entity::EntityId;
use foundation::handles::Handle;

/// Deterministic selection set backed by a bitset.
///
/// Membership is tracked by `EntityId::index()`.
///
/// Ordering contract:
/// - Iteration yields indices (and `EntityId`s) in ascending index order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionSet {
    words: Vec<u64>,
    len: usize,
}

impl SelectionSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_index(max_index_inclusive: u32) -> Self {
        let mut s = Self::default();
        s.ensure_capacity(max_index_inclusive);
        s
    }

    pub fn clear(&mut self) {
        self.words.clear();
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn contains(&self, entity: EntityId) -> bool {
        self.contains_index(entity.index())
    }

    pub fn contains_index(&self, index: u32) -> bool {
        let (word, bit) = word_bit(index);
        self.words
            .get(word)
            .is_some_and(|w| (w & (1u64 << bit)) != 0)
    }

    /// Inserts `entity` into the set.
    ///
    /// Returns `true` if the set changed.
    pub fn insert(&mut self, entity: EntityId) -> bool {
        self.insert_index(entity.index())
    }

    pub fn insert_index(&mut self, index: u32) -> bool {
        self.ensure_capacity(index);
        let (word, bit) = word_bit(index);
        let mask = 1u64 << bit;
        let w = &mut self.words[word];
        if (*w & mask) != 0 {
            return false;
        }
        *w |= mask;
        self.len += 1;
        true
    }

    /// Removes `entity` from the set.
    ///
    /// Returns `true` if the set changed.
    pub fn remove(&mut self, entity: EntityId) -> bool {
        self.remove_index(entity.index())
    }

    pub fn remove_index(&mut self, index: u32) -> bool {
        let (word, bit) = word_bit(index);
        let Some(w) = self.words.get_mut(word) else {
            return false;
        };
        let mask = 1u64 << bit;
        if (*w & mask) == 0 {
            return false;
        }
        *w &= !mask;
        self.len -= 1;
        true
    }

    pub fn union(&self, other: &Self) -> Self {
        let mut out = self.clone();
        out.union_in_place(other);
        out
    }

    pub fn intersect(&self, other: &Self) -> Self {
        let mut out = self.clone();
        out.intersect_in_place(other);
        out
    }

    /// Set difference: `self \ other`.
    pub fn diff(&self, other: &Self) -> Self {
        let mut out = self.clone();
        out.diff_in_place(other);
        out
    }

    pub fn union_in_place(&mut self, other: &Self) {
        let max_words = other.words.len().max(self.words.len());
        self.words.resize(max_words, 0);
        for (idx, ow) in other.words.iter().copied().enumerate() {
            self.words[idx] |= ow;
        }
        self.recount_len();
    }

    pub fn intersect_in_place(&mut self, other: &Self) {
        let min_words = other.words.len().min(self.words.len());
        for idx in 0..min_words {
            self.words[idx] &= other.words[idx];
        }
        for idx in min_words..self.words.len() {
            self.words[idx] = 0;
        }
        self.recount_len();
    }

    /// Set difference: `self \ other`.
    pub fn diff_in_place(&mut self, other: &Self) {
        let min_words = other.words.len().min(self.words.len());
        for idx in 0..min_words {
            self.words[idx] &= !other.words[idx];
        }
        self.recount_len();
    }

    /// Iterates selected entity indices in ascending order.
    pub fn iter_indices(&self) -> impl Iterator<Item = u32> + '_ {
        SelectionIndexIter {
            words: &self.words,
            word_index: 0,
            current_word: 0,
            base_index: 0,
        }
    }

    /// Iterates selected entities in ascending index order.
    ///
    /// Note: this uses generation 0 handles, matching the current `World` behavior.
    pub fn iter_entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.iter_indices().map(|idx| EntityId(Handle::new(idx, 0)))
    }

    fn ensure_capacity(&mut self, index: u32) {
        let (word, _bit) = word_bit(index);
        if self.words.len() <= word {
            self.words.resize(word + 1, 0);
        }
    }

    fn recount_len(&mut self) {
        self.len = self.words.iter().map(|w| w.count_ones() as usize).sum();
    }
}

fn word_bit(index: u32) -> (usize, u32) {
    let word = (index / 64) as usize;
    let bit = index % 64;
    (word, bit)
}

struct SelectionIndexIter<'a> {
    words: &'a [u64],
    word_index: usize,
    current_word: u64,
    base_index: u32,
}

impl<'a> Iterator for SelectionIndexIter<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.current_word != 0 {
                let tz = self.current_word.trailing_zeros();
                self.current_word &= !(1u64 << tz);
                return Some(self.base_index + tz);
            }

            let w = *self.words.get(self.word_index)?;
            self.current_word = w;
            self.base_index = (self.word_index as u32) * 64;
            self.word_index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SelectionSet;
    use crate::entity::EntityId;
    use foundation::handles::Handle;

    fn e(idx: u32) -> EntityId {
        EntityId(Handle::new(idx, 0))
    }

    #[test]
    fn insert_remove_contains_and_len() {
        let mut s = SelectionSet::new();
        assert!(s.is_empty());
        assert!(!s.contains(e(1)));

        assert!(s.insert(e(1)));
        assert!(s.contains(e(1)));
        assert_eq!(s.len(), 1);
        assert!(!s.insert(e(1)));
        assert_eq!(s.len(), 1);

        assert!(s.remove(e(1)));
        assert!(!s.contains(e(1)));
        assert_eq!(s.len(), 0);
        assert!(!s.remove(e(1)));
    }

    #[test]
    fn iter_is_sorted() {
        let mut s = SelectionSet::new();
        s.insert(e(10));
        s.insert(e(2));
        s.insert(e(65));
        let got: Vec<u32> = s.iter_indices().collect();
        assert_eq!(got, vec![2, 10, 65]);
    }

    #[test]
    fn set_ops_union_intersect_diff() {
        let mut a = SelectionSet::new();
        a.insert(e(1));
        a.insert(e(2));
        a.insert(e(100));

        let mut b = SelectionSet::new();
        b.insert(e(2));
        b.insert(e(3));
        b.insert(e(101));

        let u: Vec<u32> = a.union(&b).iter_indices().collect();
        assert_eq!(u, vec![1, 2, 3, 100, 101]);

        let i: Vec<u32> = a.intersect(&b).iter_indices().collect();
        assert_eq!(i, vec![2]);

        let d: Vec<u32> = a.diff(&b).iter_indices().collect();
        assert_eq!(d, vec![1, 100]);
    }
}
