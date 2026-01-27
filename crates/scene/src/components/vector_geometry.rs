use foundation::math::Vec3;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct VectorGeometryId(pub u32);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum VectorGeometryKind {
    Point,
    Line,
    Area,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VectorGeometry {
    Point { position: Vec3 },
    Line { vertices: Vec<Vec3> },
    Area { rings: Vec<Vec<Vec3>> },
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ComponentVectorGeometry {
    pub id: VectorGeometryId,
    pub kind: VectorGeometryKind,
}

impl ComponentVectorGeometry {
    pub fn new(id: VectorGeometryId, kind: VectorGeometryKind) -> Self {
        Self { id, kind }
    }
}
