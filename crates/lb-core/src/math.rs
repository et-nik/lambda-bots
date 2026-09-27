//! GoldSrc angle conventions: angles are `(pitch, yaw, roll)` in degrees, pitch positive = down
//! for view angles (`v_angle`), yaw counter-clockwise from +X.

use crate::dmath;
use glam::Vec3;

pub fn normalize_angle(mut a: f32) -> f32 {
    a %= 360.0;
    if a > 180.0 {
        a -= 360.0;
    } else if a < -180.0 {
        a += 360.0;
    }
    a
}

pub fn angle_diff(to: f32, from: f32) -> f32 {
    normalize_angle(to - from)
}

/// View angles (pitch down positive) that look along `dir`.
pub fn dir_to_view_angles(dir: Vec3) -> Vec3 {
    if dir.x == 0.0 && dir.y == 0.0 {
        let pitch = if dir.z > 0.0 { -90.0 } else { 90.0 };
        return Vec3::new(pitch, 0.0, 0.0);
    }
    let yaw = dmath::atan2(dir.y, dir.x).to_degrees();
    let pitch = -dmath::atan2(dir.z, dir.truncate().length()).to_degrees();
    Vec3::new(pitch, normalize_angle(yaw), 0.0)
}

/// Forward, right and up vectors of view angles (as `AngleVectors` with view pitch convention).
pub fn view_angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sp, cp) = dmath::sin_cos(angles.x.to_radians());
    let (sy, cy) = dmath::sin_cos(angles.y.to_radians());
    let (sr, cr) = dmath::sin_cos(angles.z.to_radians());
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let right = Vec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
    let up = Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);
    (forward, right, up)
}

/// Projects a desired world-space horizontal velocity onto forward/side move values for a yaw.
pub fn world_vel_to_move(vel: Vec3, yaw_deg: f32) -> (f32, f32) {
    let (sy, cy) = dmath::sin_cos(yaw_deg.to_radians());
    let forward = vel.x * cy + vel.y * sy;
    let side = vel.x * sy - vel.y * cy;
    (forward, side)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_roundtrip() {
        let a = dir_to_view_angles(Vec3::new(1.0, 1.0, 0.0));
        assert!((a.y - 45.0).abs() < 1e-4);
        assert!(a.x.abs() < 1e-4);
        let (f, _, _) = view_angle_vectors(Vec3::new(0.0, 90.0, 0.0));
        assert!((f - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5);
        let down = dir_to_view_angles(Vec3::new(1.0, 0.0, -1.0));
        assert!((down.x - 45.0).abs() < 1e-4, "view pitch is positive when looking down");
    }

    #[test]
    fn move_projection() {
        let (f, s) = world_vel_to_move(Vec3::new(0.0, 100.0, 0.0), 90.0);
        assert!((f - 100.0).abs() < 1e-3 && s.abs() < 1e-3);
        let (f, s) = world_vel_to_move(Vec3::new(0.0, -100.0, 0.0), 0.0);
        assert!(
            f.abs() < 1e-3 && (s - 100.0).abs() < 1e-3,
            "moving to -Y while facing +X is a right strafe"
        );
        assert_eq!(normalize_angle(270.0), -90.0);
    }
}
