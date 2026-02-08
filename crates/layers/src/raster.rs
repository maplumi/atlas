use crate::layer::{Layer, LayerId};
use crate::symbology::LayerStyle;

#[derive(Debug, Clone, PartialEq)]
pub struct RasterLayer {
    id: LayerId,
    pub style: LayerStyle,
    pub source: Option<String>,
}

impl RasterLayer {
    pub fn new(id: u64) -> Self {
        Self {
            id: LayerId(id),
            style: LayerStyle::default(),
            source: None,
        }
    }
}

impl Layer for RasterLayer {
    fn id(&self) -> LayerId {
        self.id
    }
}
