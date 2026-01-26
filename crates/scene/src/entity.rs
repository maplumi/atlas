use foundation::handles::Handle;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct EntityId(pub Handle);

impl EntityId {
    pub fn index(&self) -> u32 {
        self.0.index()
    }
}
