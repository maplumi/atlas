/// Generational handle types (very small stub)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Handle(u32, u32); // (index, generation)

impl Handle {
    pub fn new(index: u32, gen: u32) -> Self { Handle(index, gen) }
}
