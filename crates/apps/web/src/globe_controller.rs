//! Quaternion-based globe controller with arcball rotation, inertia, and smooth zoom.
//!
//! This module provides a high-quality interactive globe controller that:
//! - Uses quaternions to avoid gimbal lock
//! - Implements arcball (virtual trackball) rotation
//! - Supports inertia and damping after drag release
//! - Provides smooth exponential zoom
//! - Works with all mouse buttons (left/right/middle drag)

use std::collections::VecDeque;

/// WGS84 semi-major axis in meters.
const WGS84_A: f64 = 6_378_137.0;

/// Minimum camera distance from globe center (meters).
const MIN_DISTANCE: f64 = WGS84_A * 1.01;

/// Maximum camera distance from globe center (meters).
const MAX_DISTANCE: f64 = WGS84_A * 20.0;

/// Damping factor for angular velocity decay (per second).
const ANGULAR_DAMPING: f64 = 4.0;

/// Minimum angular velocity threshold before stopping inertia.
const ANGULAR_VELOCITY_THRESHOLD: f64 = 0.001;

/// Zoom smoothing factor (higher = faster response).
const ZOOM_SMOOTHING: f64 = 8.0;

/// Maximum samples to keep for velocity estimation.
const VELOCITY_HISTORY_SIZE: usize = 5;

/// Drag button type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragButton {
    #[default]
    None,
    Left,
    Right,
    Middle,
}

/// A velocity sample for inertia calculation.
#[derive(Debug, Clone, Copy)]
struct VelocitySample {
    /// Quaternion delta representing the rotation.
    delta_quat: [f64; 4],
    /// Time delta in seconds.
    dt: f64,
}

/// Globe controller state.
#[derive(Debug, Clone)]
pub struct GlobeController {
    /// Current orientation as a unit quaternion [x, y, z, w].
    /// This represents the rotation of the camera around the globe.
    pub orientation: [f64; 4],

    /// Current camera distance from globe center (meters).
    pub distance: f64,

    /// Target distance for smooth zoom (meters).
    target_distance: f64,

    /// Globe center in world coordinates (usually origin).
    pub target: [f64; 3],

    /// Angular velocity quaternion for inertia [x, y, z, w].
    angular_velocity: [f64; 4],

    /// Whether inertia is currently active.
    pub inertia_active: bool,

    /// Canvas dimensions.
    canvas_width: f64,
    canvas_height: f64,

    /// Drag state.
    dragging: bool,
    pub drag_button: DragButton,

    /// Last pointer position in pixels.
    last_pos_px: [f64; 2],

    /// Start pointer position for drag.
    start_pos_px: [f64; 2],

    /// Last arcball unit vector.
    arcball_last_unit: Option<[f64; 3]>,

    /// Last frame time for dt calculation.
    #[allow(dead_code)]
    last_time_s: f64,

    /// Velocity history for inertia estimation.
    velocity_history: VecDeque<VelocitySample>,

    /// Last update timestamp for velocity estimation.
    last_velocity_time_s: f64,
}

impl Default for GlobeController {
    fn default() -> Self {
        // Default orientation: looking at Africa (roughly lon 20°E, lat 5°N).
        // In viewer space: yaw ~160° places Africa in front.
        let yaw_rad = 160f64.to_radians();
        let pitch_rad = 5f64.to_radians();
        let orientation = quat_from_yaw_pitch(yaw_rad, pitch_rad);
        let default_distance = 3.0 * WGS84_A;

        Self {
            orientation,
            distance: default_distance,
            target_distance: default_distance,
            target: [0.0, 0.0, 0.0],
            angular_velocity: [0.0, 0.0, 0.0, 1.0],
            inertia_active: false,
            canvas_width: 1280.0,
            canvas_height: 720.0,
            dragging: false,
            drag_button: DragButton::None,
            last_pos_px: [0.0, 0.0],
            start_pos_px: [0.0, 0.0],
            arcball_last_unit: None,
            last_time_s: 0.0,
            velocity_history: VecDeque::with_capacity(VELOCITY_HISTORY_SIZE),
            last_velocity_time_s: 0.0,
        }
    }
}

impl GlobeController {
    /// Create a new globe controller.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set canvas dimensions.
    pub fn set_canvas_size(&mut self, width: f64, height: f64) {
        self.canvas_width = width.max(1.0);
        self.canvas_height = height.max(1.0);
    }

    /// Handle pointer down event.
    ///
    /// - `pos_px`: Pointer position in pixels [x, y].
    /// - `button`: Which button was pressed (0=left, 1=middle, 2=right).
    pub fn on_pointer_down(&mut self, pos_px: [f64; 2], button: i32) {
        // Stop inertia on new interaction.
        self.inertia_active = false;
        self.angular_velocity = [0.0, 0.0, 0.0, 1.0];
        self.velocity_history.clear();

        self.dragging = true;
        self.drag_button = match button {
            0 => DragButton::Left,
            1 => DragButton::Middle,
            2 => DragButton::Right,
            _ => DragButton::Left,
        };
        self.last_pos_px = pos_px;
        self.start_pos_px = pos_px;
        self.last_velocity_time_s = now_seconds();

        // Initialize arcball unit vector.
        self.arcball_last_unit = Some(self.screen_to_arcball(pos_px));
    }

    /// Handle pointer move event.
    ///
    /// - `pos_px`: Current pointer position in pixels [x, y].
    pub fn on_pointer_move(&mut self, pos_px: [f64; 2]) {
        if !self.dragging {
            return;
        }

        let now = now_seconds();
        let dt = (now - self.last_velocity_time_s).max(1e-6);
        self.last_velocity_time_s = now;

        // Compute arcball rotation.
        let next_unit = self.screen_to_arcball(pos_px);

        if let Some(prev_unit) = self.arcball_last_unit {
            // Compute rotation quaternion from prev_unit to next_unit.
            let delta_q = quat_from_unit_vectors(prev_unit, next_unit);

            // Apply rotation: new_orientation = delta_q * orientation
            // This rotates the camera frame, effectively rotating the globe in the opposite direction.
            self.orientation = quat_mul(delta_q, self.orientation);
            self.orientation = quat_normalize(self.orientation);

            // Record velocity sample for inertia.
            self.velocity_history.push_back(VelocitySample {
                delta_quat: delta_q,
                dt,
            });
            if self.velocity_history.len() > VELOCITY_HISTORY_SIZE {
                self.velocity_history.pop_front();
            }
        }

        self.arcball_last_unit = Some(next_unit);
        self.last_pos_px = pos_px;
    }

    /// Handle pointer up event.
    pub fn on_pointer_up(&mut self) {
        if !self.dragging {
            return;
        }

        // Estimate angular velocity from recent samples.
        self.angular_velocity = self.estimate_angular_velocity();

        // Activate inertia if velocity is significant.
        let vel_mag = quat_angle(self.angular_velocity);
        self.inertia_active = vel_mag > ANGULAR_VELOCITY_THRESHOLD;

        self.dragging = false;
        self.drag_button = DragButton::None;
        self.arcball_last_unit = None;
        self.velocity_history.clear();
    }

    /// Handle mouse wheel event for zoom.
    ///
    /// - `delta`: Wheel delta (positive = zoom out, negative = zoom in).
    pub fn on_wheel(&mut self, delta: f64) {
        // Stop inertia on zoom interaction.
        self.inertia_active = false;

        // Exponential zoom for smooth feel.
        let zoom_factor = (delta * 0.002).exp();
        self.target_distance *= zoom_factor;
        self.target_distance = self.target_distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Update the controller each frame.
    ///
    /// - `dt`: Time delta in seconds since last update.
    pub fn update(&mut self, dt: f64) {
        let dt = dt.clamp(0.0, 0.1); // Cap to avoid large jumps.

        // Apply inertia if active and not dragging.
        if self.inertia_active && !self.dragging {
            // Scale the angular velocity by dt to get per-frame rotation.
            let scaled_vel = quat_slerp([0.0, 0.0, 0.0, 1.0], self.angular_velocity, dt * 60.0);
            self.orientation = quat_mul(scaled_vel, self.orientation);
            self.orientation = quat_normalize(self.orientation);

            // Decay angular velocity.
            let decay = (-ANGULAR_DAMPING * dt).exp();
            self.angular_velocity = quat_slerp([0.0, 0.0, 0.0, 1.0], self.angular_velocity, decay);

            // Stop inertia if velocity is below threshold.
            let vel_mag = quat_angle(self.angular_velocity);
            if vel_mag < ANGULAR_VELOCITY_THRESHOLD {
                self.inertia_active = false;
                self.angular_velocity = [0.0, 0.0, 0.0, 1.0];
            }
        }

        // Smooth zoom interpolation.
        let zoom_alpha = 1.0 - (-ZOOM_SMOOTHING * dt).exp();
        self.distance += (self.target_distance - self.distance) * zoom_alpha;
        self.distance = self.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Get the view-projection matrix for rendering.
    ///
    /// - `aspect`: Canvas aspect ratio (width / height).
    /// - `fov_y_rad`: Vertical field of view in radians.
    pub fn view_proj_matrix(&self, aspect: f64, fov_y_rad: f64) -> [[f32; 4]; 4] {
        let eye = self.eye_position();
        let view = mat4_look_at_rh(eye, self.target, [0.0, 1.0, 0.0]);

        // Dynamic clipping planes for depth precision.
        let near = (self.distance * 0.001).max(10.0);
        let far = (self.distance * 4.0 + 4.0 * WGS84_A).max(near + 1.0);
        let proj = mat4_perspective_rh_z0(fov_y_rad, aspect, near, far);

        mat4_mul(proj, view)
    }

    /// Get the camera eye position in world coordinates.
    pub fn eye_position(&self) -> [f64; 3] {
        // The orientation quaternion rotates the camera around the globe.
        // Camera direction points from eye toward target (globe center).
        // We compute eye = target + orientation * (0, 0, distance).
        let forward = [0.0, 0.0, self.distance];
        let rotated = quat_rotate_vec3(self.orientation, forward);
        vec3_add(self.target, rotated)
    }

    /// Get the camera forward direction (normalized).
    pub fn forward_direction(&self) -> [f64; 3] {
        let eye = self.eye_position();
        vec3_normalize(vec3_sub(self.target, eye))
    }

    /// Get yaw angle in radians (for compatibility with existing code).
    pub fn yaw_rad(&self) -> f64 {
        let dir = self.forward_direction();
        (-dir[2]).atan2(dir[0])
    }

    /// Get pitch angle in radians (for compatibility with existing code).
    pub fn pitch_rad(&self) -> f64 {
        let dir = self.forward_direction();
        dir[1].clamp(-1.0, 1.0).asin()
    }

    /// Get the current camera distance from globe center.
    pub fn distance(&self) -> f64 {
        self.distance
    }

    /// Get the current orientation quaternion.
    pub fn orientation(&self) -> [f64; 4] {
        self.orientation
    }

    /// Get the angular velocity quaternion.
    pub fn angular_velocity(&self) -> [f64; 4] {
        self.angular_velocity
    }

    /// Check if inertia animation is currently active.
    pub fn is_inertia_active(&self) -> bool {
        self.inertia_active
    }

    /// Set orientation from yaw/pitch angles (for compatibility).
    pub fn set_from_yaw_pitch(&mut self, yaw_rad: f64, pitch_rad: f64) {
        self.orientation = quat_from_yaw_pitch(yaw_rad, pitch_rad);
        // Stop any active inertia when orientation is set externally
        self.inertia_active = false;
        self.angular_velocity = [0.0, 0.0, 0.0, 1.0];
    }

    /// Set the camera distance directly (used when syncing from 2D view).
    pub fn set_distance(&mut self, distance: f64) {
        let clamped = distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.distance = clamped;
        self.target_distance = clamped;
    }

    /// Stop any active inertia animation.
    #[allow(dead_code)]
    pub fn stop_inertia(&mut self) {
        self.inertia_active = false;
        self.angular_velocity = [0.0, 0.0, 0.0, 1.0];
        self.velocity_history.clear();
    }

    /// Reset to default view.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Convert screen position to arcball unit vector.
    fn screen_to_arcball(&self, pos_px: [f64; 2]) -> [f64; 3] {
        // NDC coordinates.
        let min_dim = self.canvas_width.min(self.canvas_height).max(1.0);
        let nx = (2.0 * pos_px[0] - self.canvas_width) / min_dim;
        let ny = (self.canvas_height - 2.0 * pos_px[1]) / min_dim;

        // Map to arcball sphere.
        let r2 = nx * nx + ny * ny;
        if r2 <= 1.0 {
            let z = (1.0 - r2).sqrt();
            vec3_normalize([nx, ny, z])
        } else {
            let inv_r = 1.0 / r2.sqrt();
            vec3_normalize([nx * inv_r, ny * inv_r, 0.0])
        }
    }

    /// Estimate angular velocity from recent samples.
    fn estimate_angular_velocity(&self) -> [f64; 4] {
        if self.velocity_history.is_empty() {
            return [0.0, 0.0, 0.0, 1.0];
        }

        // Average the quaternion deltas weighted by their dt.
        let mut total_dt = 0.0;
        let mut accumulated = [0.0, 0.0, 0.0, 1.0];

        for sample in &self.velocity_history {
            if sample.dt > 0.0 {
                // Normalize the delta to per-second rate.
                let rate = 1.0 / sample.dt;
                accumulated = quat_mul(
                    quat_slerp([0.0, 0.0, 0.0, 1.0], sample.delta_quat, rate * 0.016),
                    accumulated,
                );
                total_dt += sample.dt;
            }
        }

        if total_dt > 0.0 {
            // Scale to approximate per-frame velocity.
            let avg_dt = total_dt / self.velocity_history.len() as f64;
            quat_slerp([0.0, 0.0, 0.0, 1.0], accumulated, avg_dt)
        } else {
            [0.0, 0.0, 0.0, 1.0]
        }
    }

    /// Get debug info string.
    #[allow(dead_code)]
    pub fn debug_info(&self) -> String {
        format!(
            "orientation: [{:.3}, {:.3}, {:.3}, {:.3}]\n\
             distance: {:.0}m\n\
             drag: {:?}\n\
             inertia: {}",
            self.orientation[0],
            self.orientation[1],
            self.orientation[2],
            self.orientation[3],
            self.distance,
            self.drag_button,
            self.inertia_active
        )
    }
}

// ============================================================================
// Quaternion math utilities
// ============================================================================

/// Create quaternion from yaw and pitch angles.
fn quat_from_yaw_pitch(yaw_rad: f64, pitch_rad: f64) -> [f64; 4] {
    // Compose yaw (around Y) and pitch (around X) rotations.
    let half_yaw = yaw_rad * 0.5;
    let half_pitch = pitch_rad * 0.5;

    let cy = half_yaw.cos();
    let sy = half_yaw.sin();
    let cp = half_pitch.cos();
    let sp = half_pitch.sin();

    // q = quat_yaw * quat_pitch
    // quat_yaw = [0, sin(y/2), 0, cos(y/2)]
    // quat_pitch = [sin(p/2), 0, 0, cos(p/2)]
    [
        cy * sp,  // x
        sy * cp,  // y
        -sy * sp, // z
        cy * cp,  // w
    ]
}

/// Quaternion multiplication: a * b.
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// Normalize a quaternion.
fn quat_normalize(q: [f64; 4]) -> [f64; 4] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n > 1e-10 {
        [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Quaternion conjugate (inverse for unit quaternions).
#[allow(dead_code)]
fn quat_conjugate(q: [f64; 4]) -> [f64; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Rotate a vector by a unit quaternion.
fn quat_rotate_vec3(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let qv = [q[0], q[1], q[2]];
    let t = vec3_mul(vec3_cross(qv, v), 2.0);
    vec3_add(v, vec3_add(vec3_mul(t, q[3]), vec3_cross(qv, t)))
}

/// Spherical linear interpolation between quaternions.
fn quat_slerp(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];

    // If dot is negative, negate one quaternion to take shorter path.
    let mut b = b;
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }

    // If quaternions are very close, use linear interpolation.
    if dot > 0.9995 {
        let result = [
            a[0] + t * (b[0] - a[0]),
            a[1] + t * (b[1] - a[1]),
            a[2] + t * (b[2] - a[2]),
            a[3] + t * (b[3] - a[3]),
        ];
        return quat_normalize(result);
    }

    let theta_0 = dot.clamp(-1.0, 1.0).acos();
    let theta = theta_0 * t;
    let sin_theta = theta.sin();
    let sin_theta_0 = theta_0.sin();

    let s0 = theta.cos() - dot * sin_theta / sin_theta_0;
    let s1 = sin_theta / sin_theta_0;

    [
        s0 * a[0] + s1 * b[0],
        s0 * a[1] + s1 * b[1],
        s0 * a[2] + s1 * b[2],
        s0 * a[3] + s1 * b[3],
    ]
}

/// Get the rotation angle of a quaternion.
fn quat_angle(q: [f64; 4]) -> f64 {
    let w = q[3].clamp(-1.0, 1.0);
    2.0 * w.acos()
}

/// Create quaternion that rotates unit vector a to unit vector b.
fn quat_from_unit_vectors(a: [f64; 3], b: [f64; 3]) -> [f64; 4] {
    let dot = vec3_dot(a, b).clamp(-1.0, 1.0);

    // Nearly opposite vectors: pick arbitrary orthogonal axis.
    if dot < -0.999999 {
        let mut axis = vec3_cross([1.0, 0.0, 0.0], a);
        if vec3_dot(axis, axis) < 1e-12 {
            axis = vec3_cross([0.0, 1.0, 0.0], a);
        }
        axis = vec3_normalize(axis);
        return [axis[0], axis[1], axis[2], 0.0];
    }

    // Nearly identical vectors: return identity.
    if dot > 0.999999 {
        return [0.0, 0.0, 0.0, 1.0];
    }

    let axis = vec3_cross(a, b);
    let w = 1.0 + dot;
    quat_normalize([axis[0], axis[1], axis[2], w])
}

// ============================================================================
// Vector math utilities
// ============================================================================

fn vec3_add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn vec3_sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn vec3_mul(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn vec3_dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vec3_cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn vec3_normalize(a: [f64; 3]) -> [f64; 3] {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if n > 1e-10 {
        [a[0] / n, a[1] / n, a[2] / n]
    } else {
        [0.0, 0.0, 0.0]
    }
}

// ============================================================================
// Matrix utilities
// ============================================================================

fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut c = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            c[col][row] = a[0][row] * b[col][0]
                + a[1][row] * b[col][1]
                + a[2][row] * b[col][2]
                + a[3][row] * b[col][3];
        }
    }
    c
}

fn mat4_perspective_rh_z0(fov_y_rad: f64, aspect: f64, near: f64, far: f64) -> [[f32; 4]; 4] {
    let f = 1.0 / (0.5 * fov_y_rad).tan();
    let m00 = (f / aspect) as f32;
    let m11 = f as f32;
    let m22 = (far / (near - far)) as f32;
    let m23 = ((near * far) / (near - far)) as f32;

    [
        [m00, 0.0, 0.0, 0.0],
        [0.0, m11, 0.0, 0.0],
        [0.0, 0.0, m22, -1.0],
        [0.0, 0.0, m23, 0.0],
    ]
}

fn mat4_look_at_rh(eye: [f64; 3], target: [f64; 3], up: [f64; 3]) -> [[f32; 4]; 4] {
    let f = vec3_normalize(vec3_sub(target, eye));
    let s = vec3_normalize(vec3_cross(f, up));
    let u = vec3_cross(s, f);

    let ex = -vec3_dot(s, eye);
    let ey = -vec3_dot(u, eye);
    let ez = vec3_dot(f, eye);

    [
        [s[0] as f32, u[0] as f32, (-f[0]) as f32, 0.0],
        [s[1] as f32, u[1] as f32, (-f[1]) as f32, 0.0],
        [s[2] as f32, u[2] as f32, (-f[2]) as f32, 0.0],
        [ex as f32, ey as f32, ez as f32, 1.0],
    ]
}

// ============================================================================
// Utility
// ============================================================================

/// Get current time in seconds.
fn now_seconds() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() / 1000.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quat_identity() {
        let q = [0.0, 0.0, 0.0, 1.0];
        let v = [1.0, 2.0, 3.0];
        let rotated = quat_rotate_vec3(q, v);
        assert!((rotated[0] - v[0]).abs() < 1e-10);
        assert!((rotated[1] - v[1]).abs() < 1e-10);
        assert!((rotated[2] - v[2]).abs() < 1e-10);
    }

    #[test]
    fn test_quat_from_unit_vectors() {
        let a = [1.0, 0.0, 0.0];
        let b = [0.0, 1.0, 0.0];
        let q = quat_from_unit_vectors(a, b);
        let rotated = quat_rotate_vec3(q, a);
        assert!((rotated[0] - b[0]).abs() < 1e-6);
        assert!((rotated[1] - b[1]).abs() < 1e-6);
        assert!((rotated[2] - b[2]).abs() < 1e-6);
    }

    #[test]
    fn test_controller_default() {
        let ctrl = GlobeController::new();
        assert!(ctrl.distance > 0.0);
        assert!(!ctrl.dragging);
        assert!(!ctrl.inertia_active);
    }
}
