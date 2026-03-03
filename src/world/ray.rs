// Ray tracing with persistent incremental rays.
//
// Based on the raytraced-audio Godot project architecture: rays persist across
// audio callbacks, each advancing one bounce per tick. Metrics are smoothed
// over ~1 second for stable room sensing.
//
// See docs/directivity-and-energy-transfer.md for how directivity gates
// energy transfer along ray paths.

use crate::spatial::directivity::directivity_gain;
use crate::spatial::listener::Listener;
use crate::spatial::source::SoundSource;
use crate::world::room::Room;
use crate::world::types::Vec3;

/// A ray in 3D space.
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3, // unit vector
}

/// Result of a ray-surface intersection.
#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub point: Vec3,
    pub normal: Vec3, // unit vector, faces incoming ray
    pub distance: f32,
}

// --- Ray pool constants ---

/// Number of rays in the pool.
const RAY_COUNT: usize = 64;

/// Maximum bounces before a ray is recycled.
const MAX_BOUNCES: u32 = 8;

/// How close a ray path must pass to the listener to count as a "hit" (meters).
const LISTENER_RADIUS: f32 = 0.5;

/// Energy retained per wall bounce.
const WALL_ABSORPTION: f32 = 0.85;

/// Smoothing rate per update tick. At ~93Hz, 0.015 gives ~1s convergence.
const LERP_RATE: f32 = 0.015;

/// Minimum energy before a ray is considered dead.
const MIN_ENERGY: f32 = 0.01;

/// Offset along normal after bounce to prevent self-intersection.
const BOUNCE_EPSILON: f32 = 0.001;

// --- Persistent ray ---

#[derive(Clone, Debug)]
struct PersistentRay {
    ray: Ray,
    energy: f32,
    bounces: u32,
    total_distance: f32,
    alive: bool,
}

impl PersistentRay {
    fn dead() -> Self {
        Self {
            ray: Ray {
                origin: Vec3::ZERO,
                direction: Vec3::new(1.0, 0.0, 0.0),
            },
            energy: 0.0,
            bounces: 0,
            total_distance: 0.0,
            alive: false,
        }
    }
}

// --- Ray metrics ---

/// Smoothed aggregate measurements from the ray pool.
/// Used by AudioProcessors to modulate reverb, reflections, etc.
#[derive(Clone, Copy, Debug)]
pub struct RayMetrics {
    /// Average total distance rays travel. Correlates with room size / RT60.
    pub mean_path_length: f32,
    /// Fraction of rays that reach the listener.
    pub listener_hit_ratio: f32,
    /// Average energy of rays arriving at the listener.
    pub listener_energy: f32,
    /// Average bounce count of rays reaching the listener.
    pub mean_bounce_count: f32,
    /// Fraction of rays that escape (no hit). Always 0 for closed BoxRoom.
    pub escape_ratio: f32,
}

impl Default for RayMetrics {
    fn default() -> Self {
        Self {
            mean_path_length: 5.0,
            listener_hit_ratio: 0.5,
            listener_energy: 0.5,
            mean_bounce_count: 3.0,
            escape_ratio: 0.0,
        }
    }
}

impl RayMetrics {
    fn lerp_toward(&mut self, target: &RayMetrics, rate: f32) {
        self.mean_path_length += (target.mean_path_length - self.mean_path_length) * rate;
        self.listener_hit_ratio += (target.listener_hit_ratio - self.listener_hit_ratio) * rate;
        self.listener_energy += (target.listener_energy - self.listener_energy) * rate;
        self.mean_bounce_count += (target.mean_bounce_count - self.mean_bounce_count) * rate;
        self.escape_ratio += (target.escape_ratio - self.escape_ratio) * rate;
    }
}

// --- Ray pool ---

/// Pre-allocated pool of persistent rays. Updated incrementally per audio buffer.
pub struct RayPool {
    rays: Vec<PersistentRay>,
    smoothed: RayMetrics,
    rng_state: u32,
    next_source: usize,
}

impl RayPool {
    pub fn new() -> Self {
        Self {
            rays: (0..RAY_COUNT).map(|_| PersistentRay::dead()).collect(),
            smoothed: RayMetrics::default(),
            rng_state: 12345,
            next_source: 0,
        }
    }

    pub fn metrics(&self) -> &RayMetrics {
        &self.smoothed
    }

    /// Advance all rays one bounce. Called once per audio buffer (~93Hz).
    pub fn update(
        &mut self,
        sources: &[Box<dyn SoundSource>],
        listener: &Listener,
        room: &dyn Room,
    ) {
        if sources.is_empty() {
            return;
        }

        let mut total_path: f32 = 0.0;
        let mut path_count: u32 = 0;
        let mut listener_hits: u32 = 0;
        let mut listener_energy_sum: f32 = 0.0;
        let mut listener_bounce_sum: f32 = 0.0;
        let mut escape_count: u32 = 0;
        let mut total_active: u32 = 0;

        for i in 0..self.rays.len() {
            let ray = &mut self.rays[i];

            if !ray.alive {
                // Re-emit from a source
                let src_idx = self.next_source % sources.len();
                self.next_source = self.next_source.wrapping_add(1);

                let source = &sources[src_idx];
                let pos = source.position();
                let dir = Self::random_unit_vec(&mut self.rng_state);

                // Weight initial energy by source directivity
                let energy = directivity_gain(
                    pos,
                    source.orientation(),
                    pos + dir,
                    &source.directivity(),
                );

                *ray = PersistentRay {
                    ray: Ray {
                        origin: pos,
                        direction: dir,
                    },
                    energy,
                    bounces: 0,
                    total_distance: 0.0,
                    alive: true,
                };
                continue;
            }

            total_active += 1;

            // Cast one bounce
            if let Some(hit) = room.cast_ray(&ray.ray) {
                // Check if ray path passes near listener
                let listener_dist =
                    point_to_segment_distance(listener.position, ray.ray.origin, hit.point);

                ray.total_distance += hit.distance;

                if listener_dist < LISTENER_RADIUS {
                    listener_hits += 1;
                    listener_energy_sum += ray.energy;
                    listener_bounce_sum += ray.bounces as f32;
                }

                // Apply wall absorption and reflect
                ray.energy *= WALL_ABSORPTION;
                ray.bounces += 1;
                ray.ray.origin = hit.point + hit.normal * BOUNCE_EPSILON;
                ray.ray.direction = ray.ray.direction.reflect(hit.normal).normalize();

                // Check termination
                if ray.bounces >= MAX_BOUNCES || ray.energy < MIN_ENERGY {
                    total_path += ray.total_distance;
                    path_count += 1;
                    ray.alive = false;
                }
            } else {
                // Ray escaped (no hit)
                escape_count += 1;
                total_path += ray.total_distance;
                path_count += 1;
                ray.alive = false;
            }
        }

        // Compute instantaneous metrics
        let instant = RayMetrics {
            mean_path_length: if path_count > 0 {
                total_path / path_count as f32
            } else {
                self.smoothed.mean_path_length
            },
            listener_hit_ratio: if total_active > 0 {
                listener_hits as f32 / total_active as f32
            } else {
                self.smoothed.listener_hit_ratio
            },
            listener_energy: if listener_hits > 0 {
                listener_energy_sum / listener_hits as f32
            } else {
                0.0
            },
            mean_bounce_count: if listener_hits > 0 {
                listener_bounce_sum / listener_hits as f32
            } else {
                self.smoothed.mean_bounce_count
            },
            escape_ratio: if total_active > 0 {
                escape_count as f32 / total_active as f32
            } else {
                self.smoothed.escape_ratio
            },
        };

        self.smoothed.lerp_toward(&instant, LERP_RATE);
    }

    /// Simple LCG pseudo-RNG. Real-time safe (no allocations, no system calls).
    fn next_random(state: &mut u32) -> f32 {
        *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (*state >> 8) as f32 / 16777216.0
    }

    /// Random unit vector on the sphere (cylindrical method).
    fn random_unit_vec(state: &mut u32) -> Vec3 {
        let z = Self::next_random(state) * 2.0 - 1.0;
        let theta = Self::next_random(state) * std::f32::consts::TAU;
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * theta.cos(), r * theta.sin(), z).normalize()
    }
}

/// Minimum distance from a point to a line segment.
fn point_to_segment_distance(point: Vec3, seg_start: Vec3, seg_end: Vec3) -> f32 {
    let ab = seg_end - seg_start;
    let ap = point - seg_start;
    let ab_len_sq = ab.dot(ab);
    if ab_len_sq < 1e-10 {
        return ap.length();
    }
    let t = (ap.dot(ab) / ab_len_sq).clamp(0.0, 1.0);
    let closest = seg_start + ab * t;
    point.distance_to(closest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_unit_vec_is_unit() {
        let mut state = 42u32;
        for _ in 0..100 {
            let v = RayPool::random_unit_vec(&mut state);
            assert!(
                (v.length() - 1.0).abs() < 1e-4,
                "not unit: len={}",
                v.length()
            );
        }
    }

    #[test]
    fn point_to_segment_on_segment() {
        // Point directly on the segment
        let d = point_to_segment_distance(
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
        );
        assert!(d < 1e-5);
    }

    #[test]
    fn point_to_segment_perpendicular() {
        // Point 3m away from segment midpoint
        let d = point_to_segment_distance(
            Vec3::new(1.0, 3.0, 0.0),
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
        );
        assert!((d - 3.0).abs() < 1e-5);
    }

    #[test]
    fn point_to_segment_past_end() {
        // Point past the end of segment — distance to endpoint
        let d = point_to_segment_distance(
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
        );
        assert!((d - 3.0).abs() < 1e-5);
    }

    #[test]
    fn ray_pool_starts_with_dead_rays() {
        let pool = RayPool::new();
        assert_eq!(pool.rays.len(), RAY_COUNT);
        for ray in &pool.rays {
            assert!(!ray.alive);
        }
    }

    #[test]
    fn metrics_default_is_reasonable() {
        let pool = RayPool::new();
        let m = pool.metrics();
        assert!(m.mean_path_length > 0.0);
        assert!(m.listener_hit_ratio >= 0.0 && m.listener_hit_ratio <= 1.0);
    }

    #[test]
    fn lerp_toward_converges() {
        let mut m = RayMetrics::default();
        let target = RayMetrics {
            mean_path_length: 20.0,
            listener_hit_ratio: 1.0,
            listener_energy: 1.0,
            mean_bounce_count: 6.0,
            escape_ratio: 0.0,
        };
        // After many lerp steps, should approach target
        for _ in 0..1000 {
            m.lerp_toward(&target, LERP_RATE);
        }
        assert!(
            (m.mean_path_length - target.mean_path_length).abs() < 0.1,
            "path: {}",
            m.mean_path_length
        );
        assert!(
            (m.listener_hit_ratio - target.listener_hit_ratio).abs() < 0.01,
            "hit: {}",
            m.listener_hit_ratio
        );
    }
}
