#[derive(Debug, Clone, PartialEq)]
pub struct ComponentProperties {
    pub pairs: Vec<(String, String)>,
}

impl ComponentProperties {
    pub fn new(pairs: Vec<(String, String)>) -> Self {
        Self { pairs }
    }
}
