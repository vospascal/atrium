// Per-channel gain array shared by every renderer.

/// Maximum output channels supported. Covers stereo, 5.1, and 7.1.
pub const MAX_CHANNELS: usize = 8;

/// Per-channel gain array. Stack-allocated for real-time safety.
#[derive(Clone, Copy, Debug)]
pub struct ChannelGains {
    pub gains: [f32; MAX_CHANNELS],
    pub count: usize,
}

impl ChannelGains {
    pub fn silent(count: usize) -> Self {
        Self {
            gains: [0.0; MAX_CHANNELS],
            count,
        }
    }
}
