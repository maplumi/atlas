use crate::layer::{Layer, LayerId};
use crate::symbology::LayerStyle;

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainLayer {
    id: LayerId,
    pub style: LayerStyle,
    pub source: Option<String>,
}

impl TerrainLayer {
    pub fn new(id: u64) -> Self {
        Self {
            id: LayerId(id),
            style: LayerStyle::default(),
            source: None,
        }
    }
}

impl Layer for TerrainLayer {
    fn id(&self) -> LayerId {
        self.id
    }
}
