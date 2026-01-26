use super::{Ecef, Geodetic, geodetic_to_ecef};

/// Local East-North-Up coordinates (meters).
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Enu {
    pub east: f64,
    pub north: f64,
    pub up: f64,
}

impl Enu {
    pub fn new(east: f64, north: f64, up: f64) -> Self {
        Self { east, north, up }
    }
}

pub fn ecef_to_enu(point: Ecef, origin: Geodetic) -> Enu {
    let origin_ecef = geodetic_to_ecef(origin);
    let dx = point.x - origin_ecef.x;
    let dy = point.y - origin_ecef.y;
    let dz = point.z - origin_ecef.z;

    let sin_lat = origin.lat_rad.sin();
    let cos_lat = origin.lat_rad.cos();
    let sin_lon = origin.lon_rad.sin();
    let cos_lon = origin.lon_rad.cos();

    let east = -sin_lon * dx + cos_lon * dy;
    let north = -sin_lat * cos_lon * dx - sin_lat * sin_lon * dy + cos_lat * dz;
    let up = cos_lat * cos_lon * dx + cos_lat * sin_lon * dy + sin_lat * dz;

    Enu::new(east, north, up)
}

pub fn enu_to_ecef(enu: Enu, origin: Geodetic) -> Ecef {
    let origin_ecef = geodetic_to_ecef(origin);

    let sin_lat = origin.lat_rad.sin();
    let cos_lat = origin.lat_rad.cos();
    let sin_lon = origin.lon_rad.sin();
    let cos_lon = origin.lon_rad.cos();

    let dx = -sin_lon * enu.east - sin_lat * cos_lon * enu.north + cos_lat * cos_lon * enu.up;
    let dy = cos_lon * enu.east - sin_lat * sin_lon * enu.north + cos_lat * sin_lon * enu.up;
    let dz = cos_lat * enu.north + sin_lat * enu.up;

    Ecef::new(origin_ecef.x + dx, origin_ecef.y + dy, origin_ecef.z + dz)
}

#[cfg(test)]
mod tests {
    use super::{Enu, ecef_to_enu, enu_to_ecef};
    use crate::math::{Geodetic, geodetic_to_ecef};

    fn assert_close(a: f64, b: f64, eps: f64) {
        let diff = (a - b).abs();
        assert!(diff <= eps, "expected {a} ~= {b} (diff {diff})");
    }

    #[test]
    fn enu_round_trip_at_equator() {
        let origin = Geodetic::new(0.0, 0.0, 0.0);
        let enu = Enu::new(15.0, -8.0, 2.5);
        let ecef = enu_to_ecef(enu, origin);
        let enu_rt = ecef_to_enu(ecef, origin);

        assert_close(enu_rt.east, enu.east, 1e-9);
        assert_close(enu_rt.north, enu.north, 1e-9);
        assert_close(enu_rt.up, enu.up, 1e-9);
    }

    #[test]
    fn enu_zero_at_origin() {
        let origin = Geodetic::new(0.1, -0.2, 35.0);
        let origin_ecef = geodetic_to_ecef(origin);
        let enu = ecef_to_enu(origin_ecef, origin);
        assert_close(enu.east, 0.0, 1e-9);
        assert_close(enu.north, 0.0, 1e-9);
        assert_close(enu.up, 0.0, 1e-9);
    }
}
