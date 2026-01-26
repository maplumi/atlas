use super::Ecef;

/// WGS84 semi-major axis (meters).
pub const WGS84_A: f64 = 6_378_137.0;
/// WGS84 flattening.
pub const WGS84_F: f64 = 1.0 / 298.257_223_563;
/// WGS84 semi-minor axis (meters).
pub const WGS84_B: f64 = WGS84_A * (1.0 - WGS84_F);
/// WGS84 first eccentricity squared.
pub const WGS84_E2: f64 = WGS84_F * (2.0 - WGS84_F);
/// WGS84 second eccentricity squared.
pub const WGS84_EP2: f64 = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_B * WGS84_B);

/// Geodetic coordinates in radians and meters.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Geodetic {
    pub lat_rad: f64,
    pub lon_rad: f64,
    pub alt_m: f64,
}

impl Geodetic {
    pub fn new(lat_rad: f64, lon_rad: f64, alt_m: f64) -> Self {
        Self {
            lat_rad,
            lon_rad,
            alt_m,
        }
    }
}

pub fn geodetic_to_ecef(geo: Geodetic) -> Ecef {
    let sin_lat = geo.lat_rad.sin();
    let cos_lat = geo.lat_rad.cos();
    let sin_lon = geo.lon_rad.sin();
    let cos_lon = geo.lon_rad.cos();

    let n = WGS84_A / (1.0 - WGS84_E2 * sin_lat * sin_lat).sqrt();
    let x = (n + geo.alt_m) * cos_lat * cos_lon;
    let y = (n + geo.alt_m) * cos_lat * sin_lon;
    let z = (n * (1.0 - WGS84_E2) + geo.alt_m) * sin_lat;

    Ecef::new(x, y, z)
}

pub fn ecef_to_geodetic(ecef: Ecef) -> Geodetic {
    let p = (ecef.x * ecef.x + ecef.y * ecef.y).sqrt();
    let lon = ecef.y.atan2(ecef.x);

    let theta = (ecef.z * WGS84_A).atan2(p * WGS84_B);
    let sin_theta = theta.sin();
    let cos_theta = theta.cos();

    let lat = (ecef.z + WGS84_EP2 * WGS84_B * sin_theta * sin_theta * sin_theta)
        .atan2(p - WGS84_E2 * WGS84_A * cos_theta * cos_theta * cos_theta);

    let sin_lat = lat.sin();
    let n = WGS84_A / (1.0 - WGS84_E2 * sin_lat * sin_lat).sqrt();
    let alt = p / lat.cos() - n;

    Geodetic::new(lat, lon, alt)
}

#[cfg(test)]
mod tests {
    use super::{Geodetic, WGS84_A, ecef_to_geodetic, geodetic_to_ecef};

    fn assert_close(a: f64, b: f64, eps: f64) {
        let diff = (a - b).abs();
        assert!(diff <= eps, "expected {a} ~= {b} (diff {diff})");
    }

    #[test]
    fn geodetic_to_ecef_equator_prime_meridian() {
        let geo = Geodetic::new(0.0, 0.0, 0.0);
        let ecef = geodetic_to_ecef(geo);
        assert_close(ecef.x, WGS84_A, 1e-6);
        assert_close(ecef.y, 0.0, 1e-6);
        assert_close(ecef.z, 0.0, 1e-6);
    }

    #[test]
    fn geodetic_to_ecef_equator_90e() {
        let geo = Geodetic::new(0.0, std::f64::consts::FRAC_PI_2, 0.0);
        let ecef = geodetic_to_ecef(geo);
        assert_close(ecef.x, 0.0, 1e-6);
        assert_close(ecef.y, WGS84_A, 1e-6);
        assert_close(ecef.z, 0.0, 1e-6);
    }

    #[test]
    fn round_trip_geodetic_ecef() {
        let geo = Geodetic::new(
            std::f64::consts::FRAC_PI_6,
            -std::f64::consts::FRAC_PI_3,
            120.0,
        );
        let ecef = geodetic_to_ecef(geo);
        let geo_rt = ecef_to_geodetic(ecef);
        assert_close(geo_rt.lat_rad, geo.lat_rad, 1e-9);
        assert_close(geo_rt.lon_rad, geo.lon_rad, 1e-9);
        assert_close(geo_rt.alt_m, geo.alt_m, 1e-6);
    }
}
