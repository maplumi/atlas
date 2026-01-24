/// Simple generational id implementation (placeholder)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Id(u64);

impl Id {
    pub fn new(n: u64) -> Self {
        Id(n)
    }
}
