/// Minimal arena allocator placeholder
pub struct Arena<T> {
    items: Vec<T>,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Arena { items: Vec::new() }
    }
    pub fn alloc(&mut self, v: T) -> usize {
        self.items.push(v);
        self.items.len() - 1
    }
    pub fn get(&self, idx: usize) -> Option<&T> {
        self.items.get(idx)
    }
}
