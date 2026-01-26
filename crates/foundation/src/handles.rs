/// Generational handle types (very small stub)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Handle(u32, u32); // (index, generation)

impl Handle {
    pub fn new(index: u32, generation: u32) -> Self {
        Handle(index, generation)
    }

    pub fn index(self) -> u32 {
        self.0
    }

    pub fn generation(self) -> u32 {
        self.1
    }
}

#[cfg(test)]
mod tests {
    use super::Handle;

    #[test]
    fn handle_accessors() {
        let handle = Handle::new(7, 2);
        assert_eq!(handle.index(), 7);
        assert_eq!(handle.generation(), 2);
    }
}
