//! `ResetScene`: back to the listener, gain, air, render mode, channels and
//! source motion the scene was built with.

use super::AudioScene;

impl AudioScene {
    /// Restore the state recorded when the scene was built.
    pub(super) fn reset_to_initial_state(&mut self) {
        self.listener.position = self.initial_listener_pos;
        self.listener.yaw = self.initial_listener_yaw;
        self.master_gain = self.initial_master_gain;
        self.environment.atmosphere = self.initial_atmosphere;
        self.active_pipeline = self.initial_render_mode;
        self.restore_startup_channels();
        for p in self.pipelines.iter_mut() {
            p.reset();
        }
        for (source, init) in self.sources.iter_mut().zip(&self.initial_source_states) {
            source.set_position(init.position);
            source.set_orbit_radius(init.orbit_radius);
            source.set_orbit_speed(init.orbit_speed);
            source.set_orbit_angle(0.0);
            source.set_muted(false);
        }
    }
}
