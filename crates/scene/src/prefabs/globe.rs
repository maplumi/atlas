use foundation::math::{Vec3, WGS84_A, WGS84_B};

use crate::World;
use crate::components::{ComponentBounds, Drawable3D, Transform};
use crate::entity::EntityId;

pub fn spawn_wgs84_globe(world: &mut World) -> EntityId {
    let entity = world.spawn();
    world.set_transform(entity, Transform::identity());
    world.set_drawable_3d(entity, Drawable3D::wgs84_globe());
    world.set_bounds(
        entity,
        ComponentBounds::new(
            Vec3::new(-WGS84_A, -WGS84_A, -WGS84_B),
            Vec3::new(WGS84_A, WGS84_A, WGS84_B),
        ),
    );
    entity
}

#[cfg(test)]
mod tests {
    use super::spawn_wgs84_globe;
    use crate::World;
    use crate::components::{Drawable3D, Shape3D};

    #[test]
    fn spawns_globe_drawable() {
        let mut world = World::new();
        let entity = spawn_wgs84_globe(&mut world);

        let drawables = world.drawables_3d();
        assert_eq!(drawables.len(), 1);
        assert_eq!(drawables[0].0, entity);

        let Drawable3D { shape } = drawables[0].2;
        assert!(matches!(shape, Shape3D::Ellipsoid { .. }));
    }
}
