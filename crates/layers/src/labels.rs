use foundation::math::Vec3;
use scene::components::VectorGeometryKind;
use scene::{World, entity::EntityId};
use std::collections::HashSet;

use crate::layer::{Layer, LayerId};

#[derive(Debug, Clone, PartialEq)]
pub struct LabelStyle {
    pub font_size_px: f32,
    pub color: [f32; 4],
    pub halo_color: [f32; 4],
    pub halo_width_px: f32,
}

impl Default for LabelStyle {
    fn default() -> Self {
        Self {
            font_size_px: 14.0,
            color: [1.0, 1.0, 1.0, 1.0],
            halo_color: [0.0, 0.0, 0.0, 0.85],
            halo_width_px: 2.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelRule {
    pub key: String,
    pub kind: Option<VectorGeometryKind>,
    pub priority: f32,
    pub style: LabelStyle,
}

impl LabelRule {
    pub fn new(key: impl Into<String>, priority: f32) -> Self {
        Self {
            key: key.into(),
            kind: None,
            priority,
            style: LabelStyle::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelsConfig {
    pub rules: Vec<LabelRule>,
    pub max_labels: usize,
    pub max_text_len: usize,
}

impl Default for LabelsConfig {
    fn default() -> Self {
        Self {
            rules: vec![LabelRule::new("name", 1.0)],
            max_labels: 10_000,
            max_text_len: 256,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelAnchor {
    pub entity: EntityId,
    pub text: String,
    pub position: Vec3,
    pub kind: VectorGeometryKind,
    pub priority: f32,
    pub style: LabelStyle,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct LabelsLayerSnapshot {
    pub labels: Vec<LabelAnchor>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelsLayer {
    id: LayerId,
    pub config: LabelsConfig,
}

impl LabelsLayer {
    pub fn new(id: u64, config: LabelsConfig) -> Self {
        Self {
            id: LayerId(id),
            config,
        }
    }

    pub fn extract(&self, world: &World) -> LabelsLayerSnapshot {
        let mut out = Vec::new();
        if self.config.rules.is_empty() {
            return LabelsLayerSnapshot { labels: out };
        }

        for (entity, _transform, component) in world.vector_geometries_by_entity() {
            let Some(props) = world.properties(entity) else {
                continue;
            };
            let Some(geom) = world.vector_geometry(component.id) else {
                continue;
            };
            let Some(anchor) = label_anchor_for_geometry(geom) else {
                continue;
            };

            for rule in &self.config.rules {
                if let Some(kind) = rule.kind
                    && component.kind != kind
                {
                    continue;
                }

                let mut text: Option<&str> = None;
                for (k, v) in &props.pairs {
                    if k == &rule.key {
                        text = Some(v.as_str());
                        break;
                    }
                }
                let Some(raw_text) = text else {
                    continue;
                };
                let trimmed = raw_text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.len() > self.config.max_text_len {
                    continue;
                }

                out.push(LabelAnchor {
                    entity,
                    text: trimmed.to_string(),
                    position: anchor,
                    kind: component.kind,
                    priority: rule.priority,
                    style: rule.style.clone(),
                });
            }
        }

        out.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if out.len() > self.config.max_labels {
            out.truncate(self.config.max_labels);
        }

        LabelsLayerSnapshot { labels: out }
    }
}

impl Layer for LabelsLayer {
    fn id(&self) -> LayerId {
        self.id
    }
}

pub trait LabelProjector {
    fn project(&self, world: Vec3) -> Option<[f32; 2]>;
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct LabelLayoutConfig {
    pub viewport_px: [f32; 2],
    pub cell_px: f32,
    pub padding_px: f32,
    pub max_labels: usize,
}

impl Default for LabelLayoutConfig {
    fn default() -> Self {
        Self {
            viewport_px: [1.0, 1.0],
            cell_px: 32.0,
            padding_px: 4.0,
            max_labels: 400,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedLabel2D {
    pub text: String,
    pub screen_pos_px: [f32; 2],
    pub size_px: [f32; 2],
    pub priority: f32,
    pub style: LabelStyle,
}

pub fn layout_labels_2d<P: LabelProjector>(
    labels: &[LabelAnchor],
    projector: &P,
    config: LabelLayoutConfig,
) -> Vec<PlacedLabel2D> {
    let mut out = Vec::new();
    let mut occupied: HashSet<u64> = HashSet::new();

    for label in labels {
        if out.len() >= config.max_labels {
            break;
        }

        let Some(screen) = projector.project(label.position) else {
            continue;
        };
        if !screen[0].is_finite() || !screen[1].is_finite() {
            continue;
        }

        let size = estimate_text_size(label.text.as_str(), &label.style);
        let half_w = size[0] * 0.5 + config.padding_px;
        let half_h = size[1] * 0.5 + config.padding_px;

        if screen[0] + half_w < 0.0
            || screen[1] + half_h < 0.0
            || screen[0] - half_w > config.viewport_px[0]
            || screen[1] - half_h > config.viewport_px[1]
        {
            continue;
        }

        if !try_place_label(&mut occupied, screen, [half_w, half_h], config.cell_px) {
            continue;
        }

        out.push(PlacedLabel2D {
            text: label.text.clone(),
            screen_pos_px: screen,
            size_px: size,
            priority: label.priority,
            style: label.style.clone(),
        });
    }

    out
}

fn estimate_text_size(text: &str, style: &LabelStyle) -> [f32; 2] {
    let count = text.chars().count().max(1) as f32;
    let width = style.font_size_px * 0.6 * count;
    [width, style.font_size_px]
}

fn try_place_label(
    occupied: &mut HashSet<u64>,
    screen: [f32; 2],
    half_size: [f32; 2],
    cell_px: f32,
) -> bool {
    let min_x = ((screen[0] - half_size[0]) / cell_px).floor() as i32;
    let max_x = ((screen[0] + half_size[0]) / cell_px).floor() as i32;
    let min_y = ((screen[1] - half_size[1]) / cell_px).floor() as i32;
    let max_y = ((screen[1] + half_size[1]) / cell_px).floor() as i32;

    for cy in min_y..=max_y {
        for cx in min_x..=max_x {
            if occupied.contains(&cell_key(cx, cy)) {
                return false;
            }
        }
    }

    for cy in min_y..=max_y {
        for cx in min_x..=max_x {
            occupied.insert(cell_key(cx, cy));
        }
    }

    true
}

fn cell_key(cx: i32, cy: i32) -> u64 {
    ((cx as u64) << 32) ^ (cy as u32 as u64)
}

fn label_anchor_for_geometry(geom: &scene::components::VectorGeometry) -> Option<Vec3> {
    match geom {
        scene::components::VectorGeometry::Point { position } => Some(*position),
        scene::components::VectorGeometry::Line { vertices } => line_midpoint(vertices),
        scene::components::VectorGeometry::Area { rings } => area_centroid(rings),
    }
}

fn line_midpoint(vertices: &[Vec3]) -> Option<Vec3> {
    if vertices.len() < 2 {
        return vertices.first().copied();
    }

    let mut total = 0.0;
    let mut segments: Vec<(Vec3, Vec3, f64)> = Vec::with_capacity(vertices.len() - 1);
    for pair in vertices.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let len = vec3_len(vec3_sub(b, a));
        if !len.is_finite() || len <= 0.0 {
            continue;
        }
        total += len;
        segments.push((a, b, len));
    }
    if total <= 0.0 {
        return vertices.first().copied();
    }

    let mut acc = 0.0;
    let target = total * 0.5;
    for (a, b, len) in segments {
        if acc + len >= target {
            let t = (target - acc) / len;
            return Some(vec3_lerp(a, b, t));
        }
        acc += len;
    }

    vertices.last().copied()
}

fn area_centroid(rings: &[Vec<Vec3>]) -> Option<Vec3> {
    let outer = rings.first()?;
    if outer.is_empty() {
        return None;
    }
    let mut sum = Vec3::new(0.0, 0.0, 0.0);
    let mut count = 0.0_f64;
    for v in outer {
        if is_finite_vec3(*v) {
            sum = vec3_add(sum, *v);
            count += 1.0;
        }
    }
    if count <= 0.0 {
        return None;
    }
    Some(vec3_mul(sum, 1.0 / count))
}

fn is_finite_vec3(v: Vec3) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}

fn vec3_add(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z)
}

fn vec3_sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x - b.x, a.y - b.y, a.z - b.z)
}

fn vec3_mul(v: Vec3, s: f64) -> Vec3 {
    Vec3::new(v.x * s, v.y * s, v.z * s)
}

fn vec3_len(v: Vec3) -> f64 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

fn vec3_lerp(a: Vec3, b: Vec3, t: f64) -> Vec3 {
    vec3_add(a, vec3_mul(vec3_sub(b, a), t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundation::handles::Handle;
    use foundation::math::Vec3;
    use scene::components::{
        ComponentVectorGeometry, Transform, VectorGeometry, VectorGeometryKind,
    };
    use scene::world::World;

    struct IdentityProjector;

    impl LabelProjector for IdentityProjector {
        fn project(&self, world: Vec3) -> Option<[f32; 2]> {
            Some([world.x as f32, world.y as f32])
        }
    }

    #[test]
    fn extracts_labels_from_properties() {
        let mut world = World::new();
        let entity = world.spawn();
        world.set_transform(entity, Transform::translate(Vec3::new(1.0, 2.0, 3.0)));
        world.set_properties(
            entity,
            scene::components::ComponentProperties::new(vec![("name".into(), "Alpha".into())]),
        );
        let geom_id = world.add_vector_geometry(VectorGeometry::Point {
            position: Vec3::new(1.0, 2.0, 3.0),
        });
        world.set_vector_geometry(
            entity,
            ComponentVectorGeometry::new(geom_id, VectorGeometryKind::Point),
        );

        let layer = LabelsLayer::new(1, LabelsConfig::default());
        let snapshot = layer.extract(&world);
        assert_eq!(snapshot.labels.len(), 1);
        assert_eq!(snapshot.labels[0].text, "Alpha");
        assert_eq!(snapshot.labels[0].position, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn layout_rejects_overlapping_labels() {
        let mut labels = Vec::new();
        let style = LabelStyle::default();
        let entity = EntityId(Handle::new(1, 0));
        labels.push(LabelAnchor {
            entity,
            text: "A".into(),
            position: Vec3::new(50.0, 50.0, 0.0),
            kind: VectorGeometryKind::Point,
            priority: 2.0,
            style: style.clone(),
        });
        labels.push(LabelAnchor {
            entity,
            text: "B".into(),
            position: Vec3::new(50.0, 50.0, 0.0),
            kind: VectorGeometryKind::Point,
            priority: 1.0,
            style: style.clone(),
        });

        let placed = layout_labels_2d(
            &labels,
            &IdentityProjector,
            LabelLayoutConfig {
                viewport_px: [100.0, 100.0],
                cell_px: 24.0,
                padding_px: 2.0,
                max_labels: 10,
            },
        );

        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].text, "A");
    }
}
