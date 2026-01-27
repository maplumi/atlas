use crate::components::{
    ComponentBounds, ComponentTimeSpan, ComponentVectorGeometry, Drawable2D, Drawable3D, Transform,
    VectorGeometry, VectorGeometryId, Visibility,
};
use crate::entity::EntityId;
use foundation::handles::Handle;
use foundation::time::Time;

#[derive(Debug, Default)]
pub struct World {
    next_index: u32,
    transforms: Vec<Option<Transform>>,
    bounds: Vec<Option<ComponentBounds>>,
    visibility: Vec<Option<Visibility>>,
    time_spans: Vec<Option<ComponentTimeSpan>>,
    drawables_2d: Vec<Option<Drawable2D>>,
    drawables_3d: Vec<Option<Drawable3D>>,
    vector_geometry: Vec<Option<ComponentVectorGeometry>>,
    vector_geometries: Vec<VectorGeometry>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn(&mut self) -> EntityId {
        let id = EntityId(Handle::new(self.next_index, 0));
        self.next_index += 1;
        let idx = id.index() as usize;
        self.ensure_capacity(idx);
        id
    }

    pub fn set_transform(&mut self, entity: EntityId, transform: Transform) {
        self.ensure_capacity(entity.index() as usize);
        self.transforms[entity.index() as usize] = Some(transform);
    }

    pub fn set_bounds(&mut self, entity: EntityId, bounds: ComponentBounds) {
        self.ensure_capacity(entity.index() as usize);
        self.bounds[entity.index() as usize] = Some(bounds);
    }

    pub fn set_visibility(&mut self, entity: EntityId, visibility: Visibility) {
        self.ensure_capacity(entity.index() as usize);
        self.visibility[entity.index() as usize] = Some(visibility);
    }

    pub fn set_time_span(&mut self, entity: EntityId, span: ComponentTimeSpan) {
        self.ensure_capacity(entity.index() as usize);
        self.time_spans[entity.index() as usize] = Some(span);
    }

    pub fn set_drawable_2d(&mut self, entity: EntityId, drawable: Drawable2D) {
        self.ensure_capacity(entity.index() as usize);
        self.drawables_2d[entity.index() as usize] = Some(drawable);
    }

    pub fn set_drawable_3d(&mut self, entity: EntityId, drawable: Drawable3D) {
        self.ensure_capacity(entity.index() as usize);
        self.drawables_3d[entity.index() as usize] = Some(drawable);
    }

    pub fn add_vector_geometry(&mut self, geometry: VectorGeometry) -> VectorGeometryId {
        let id = VectorGeometryId(self.vector_geometries.len() as u32);
        self.vector_geometries.push(geometry);
        id
    }

    pub fn set_vector_geometry(&mut self, entity: EntityId, component: ComponentVectorGeometry) {
        self.ensure_capacity(entity.index() as usize);
        self.vector_geometry[entity.index() as usize] = Some(component);
    }

    pub fn vector_geometry_component(&self, entity: EntityId) -> Option<ComponentVectorGeometry> {
        self.vector_geometry
            .get(entity.index() as usize)
            .and_then(|v| *v)
    }

    pub fn vector_geometry(&self, id: VectorGeometryId) -> Option<&VectorGeometry> {
        self.vector_geometries.get(id.0 as usize)
    }

    pub fn vector_geometries_by_entity(
        &self,
    ) -> Vec<(EntityId, Transform, ComponentVectorGeometry)> {
        let mut out = Vec::new();
        for (idx, comp) in self.vector_geometry.iter().enumerate() {
            let Some(comp) = comp else { continue };
            let Some(transform) = self.transforms.get(idx).and_then(|t| *t) else {
                continue;
            };
            let visible = self
                .visibility
                .get(idx)
                .and_then(|v| *v)
                .map(|v| v.visible)
                .unwrap_or(true);
            if !visible {
                continue;
            }

            out.push((EntityId(Handle::new(idx as u32, 0)), transform, *comp));
        }
        out
    }

    pub fn drawables_2d(&self) -> Vec<(EntityId, Transform, Drawable2D)> {
        self.collect_drawables(&self.drawables_2d)
    }

    pub fn drawables_3d(&self) -> Vec<(EntityId, Transform, Drawable3D)> {
        self.collect_drawables(&self.drawables_3d)
    }

    pub fn drawables_2d_at_time(&self, time: Time) -> Vec<(EntityId, Transform, Drawable2D)> {
        self.collect_drawables_at_time(&self.drawables_2d, time)
    }

    pub fn drawables_3d_at_time(&self, time: Time) -> Vec<(EntityId, Transform, Drawable3D)> {
        self.collect_drawables_at_time(&self.drawables_3d, time)
    }

    fn collect_drawables<T: Copy>(&self, drawables: &[Option<T>]) -> Vec<(EntityId, Transform, T)> {
        let mut out = Vec::new();
        for (idx, drawable) in drawables.iter().enumerate() {
            let Some(drawable) = drawable else { continue };
            let Some(transform) = self.transforms.get(idx).and_then(|t| *t) else {
                continue;
            };
            let visible = self
                .visibility
                .get(idx)
                .and_then(|v| *v)
                .map(|v| v.visible)
                .unwrap_or(true);
            if !visible {
                continue;
            }

            out.push((EntityId(Handle::new(idx as u32, 0)), transform, *drawable));
        }
        out
    }

    fn collect_drawables_at_time<T: Copy>(
        &self,
        drawables: &[Option<T>],
        time: Time,
    ) -> Vec<(EntityId, Transform, T)> {
        let mut out = Vec::new();
        for (idx, drawable) in drawables.iter().enumerate() {
            let Some(drawable) = drawable else { continue };
            let Some(transform) = self.transforms.get(idx).and_then(|t| *t) else {
                continue;
            };

            let visible = self
                .visibility
                .get(idx)
                .and_then(|v| *v)
                .map(|v| v.visible)
                .unwrap_or(true);
            if !visible {
                continue;
            }

            if let Some(ComponentTimeSpan { span }) = self.time_spans.get(idx).and_then(|s| *s)
                && (time.0 < span.start.0 || time.0 > span.end.0)
            {
                continue;
            }

            out.push((EntityId(Handle::new(idx as u32, 0)), transform, *drawable));
        }
        out
    }

    fn ensure_capacity(&mut self, idx: usize) {
        if self.transforms.len() <= idx {
            let new_len = idx + 1;
            self.transforms.resize(new_len, None);
            self.bounds.resize(new_len, None);
            self.visibility.resize(new_len, None);
            self.time_spans.resize(new_len, None);
            self.drawables_2d.resize(new_len, None);
            self.drawables_3d.resize(new_len, None);
            self.vector_geometry.resize(new_len, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::World;
    use crate::components::{ComponentTimeSpan, Drawable2D, Transform, Visibility};
    use foundation::math::Vec2;
    use foundation::time::{Time, TimeSpan};

    #[test]
    fn spawn_and_collect_drawables() {
        let mut world = World::new();
        let entity = world.spawn();
        world.set_transform(entity, Transform::identity());
        world.set_drawable_2d(entity, Drawable2D::rect(Vec2::new(1.0, 1.0)));

        let drawables = world.drawables_2d();
        assert_eq!(drawables.len(), 1);
        assert_eq!(drawables[0].0, entity);
    }

    #[test]
    fn hidden_entities_are_filtered() {
        let mut world = World::new();
        let entity = world.spawn();
        world.set_transform(entity, Transform::identity());
        world.set_drawable_2d(entity, Drawable2D::rect(Vec2::new(1.0, 1.0)));
        world.set_visibility(entity, Visibility::hidden());

        let drawables = world.drawables_2d();
        assert!(drawables.is_empty());
    }

    #[test]
    fn time_span_filters_drawables_when_querying_at_time() {
        let mut world = World::new();
        let entity = world.spawn();
        world.set_transform(entity, Transform::identity());
        world.set_drawable_2d(entity, Drawable2D::rect(Vec2::new(1.0, 1.0)));
        world.set_time_span(
            entity,
            ComponentTimeSpan::new(TimeSpan {
                start: Time(10.0),
                end: Time(20.0),
            }),
        );

        assert!(world.drawables_2d_at_time(Time(5.0)).is_empty());
        assert_eq!(world.drawables_2d_at_time(Time(10.0)).len(), 1);
        assert_eq!(world.drawables_2d_at_time(Time(15.0)).len(), 1);
        assert_eq!(world.drawables_2d_at_time(Time(20.0)).len(), 1);
        assert!(world.drawables_2d_at_time(Time(25.0)).is_empty());
    }
}
