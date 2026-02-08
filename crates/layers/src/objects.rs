use foundation::time::Time;
use scene::components::{Drawable2D, Drawable3D, Transform};
use scene::{World, entity::EntityId};

use crate::layer::{Layer, LayerId};

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ObjectsLayer {
    id: LayerId,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ObjectsLayerSnapshot {
    pub drawables_2d: Vec<(EntityId, Transform, Drawable2D)>,
    pub drawables_3d: Vec<(EntityId, Transform, Drawable3D)>,
}

impl ObjectsLayer {
    pub fn new(id: u64) -> Self {
        Self { id: LayerId(id) }
    }

    pub fn extract(&self, world: &World) -> ObjectsLayerSnapshot {
        ObjectsLayerSnapshot {
            drawables_2d: world.drawables_2d(),
            drawables_3d: world.drawables_3d(),
        }
    }

    pub fn extract_at_time(&self, world: &World, time: Time) -> ObjectsLayerSnapshot {
        ObjectsLayerSnapshot {
            drawables_2d: world.drawables_2d_at_time(time),
            drawables_3d: world.drawables_3d_at_time(time),
        }
    }
}

impl Layer for ObjectsLayer {
    fn id(&self) -> LayerId {
        self.id
    }
}
