use foundation::math::{Vec2, Vec3};
use foundation::time::Time;
use scene::components::{Shape2D, Shape3D, Transform};
use scene::world::World;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Camera2D {
    pub center: Vec2,
    pub size: Vec2,
}

impl Camera2D {
    pub fn new(center: Vec2, size: Vec2) -> Self {
        Self { center, size }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Camera3D {
    pub position: Vec3,
    pub target: Vec3,
    pub fov_y_rad: f64,
    pub near: f64,
    pub far: f64,
}

impl Camera3D {
    pub fn look_at(position: Vec3, target: Vec3, fov_y_rad: f64, near: f64, far: f64) -> Self {
        Self {
            position,
            target,
            fov_y_rad,
            near,
            far,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum RenderCommand {
    Draw2D {
        transform: Transform,
        shape: Shape2D,
    },
    Draw3D {
        transform: Transform,
        shape: Shape3D,
    },
}

#[derive(Debug, Default)]
pub struct RenderFrame {
    pub commands: Vec<RenderCommand>,
}

pub struct Renderer;

impl Renderer {
    pub fn collect_2d(world: &World, _camera: Camera2D, time: Time) -> RenderFrame {
        let mut frame = RenderFrame::default();
        for (_, transform, drawable) in world.drawables_2d_at_time(time) {
            frame.commands.push(RenderCommand::Draw2D {
                transform,
                shape: drawable.shape,
            });
        }
        frame
    }

    pub fn collect_3d(world: &World, _camera: Camera3D, time: Time) -> RenderFrame {
        let mut frame = RenderFrame::default();
        for (_, transform, drawable) in world.drawables_3d_at_time(time) {
            frame.commands.push(RenderCommand::Draw3D {
                transform,
                shape: drawable.shape,
            });
        }
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::{Camera2D, Camera3D, RenderCommand, Renderer};
    use foundation::math::{Vec2, Vec3};
    use foundation::time::Time;
    use scene::components::{Drawable2D, Drawable3D, Transform};
    use scene::world::World;

    #[test]
    fn collect_2d_commands() {
        let mut world = World::new();
        let entity = world.spawn();
        world.set_transform(entity, Transform::identity());
        world.set_drawable_2d(entity, Drawable2D::rect(Vec2::new(1.0, 2.0)));

        let frame = Renderer::collect_2d(
            &world,
            Camera2D::new(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)),
            Time(0.0),
        );
        assert!(matches!(
            frame.commands.as_slice(),
            [RenderCommand::Draw2D { .. }]
        ));
    }

    #[test]
    fn collect_3d_commands() {
        let mut world = World::new();
        let entity = world.spawn();
        world.set_transform(entity, Transform::identity());
        world.set_drawable_3d(entity, Drawable3D::cube(1.0));

        let frame = Renderer::collect_3d(
            &world,
            Camera3D::look_at(
                Vec3::new(0.0, 0.0, 10.0),
                Vec3::new(0.0, 0.0, 0.0),
                1.0,
                0.1,
                1000.0,
            ),
            Time(0.0),
        );
        assert!(matches!(
            frame.commands.as_slice(),
            [RenderCommand::Draw3D { .. }]
        ));
    }
}
