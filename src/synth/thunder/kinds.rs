// The four kinds of thunder: what flash each is made of and where it is.

/// What makes one kind of thunder: the flash and where it is.
pub(super) struct Kind {
    /// A ground strike climbs from the ground to the cloud; otherwise a flash
    /// runs horizontally inside the cloud.
    pub(super) ground: bool,
    /// Distance to the strike's ground point, or to the nearest point of a
    /// cloud flash (m).
    pub(super) distance: (f32, f32),
    /// Length of the main channel (m).
    pub(super) length: (f32, f32),
    /// Cloud base for a ground strike, height of a cloud flash (m).
    pub(super) height: (f32, f32),
    /// How far a ground strike's channel runs on inside the cloud once it
    /// reaches it (m): the long, darker tail of a rumble.
    pub(super) in_cloud: (f32, f32),
    pub(super) branches: u32,
    /// Length of one segment of channel (m).
    pub(super) step: f32,
    /// Its loudest moment, before the output trim.
    pub(super) level: f32,
    /// How much of the pulses themselves is heard over the roar: the edge of
    /// a crack close by, little of it from far away.
    pub(super) edge: f32,
    /// Above this the sound falls steeply (Hz): thunder heard from a few
    /// kilometres keeps almost nothing above about 1 kHz.
    pub(super) cliff_hz: f32,
    /// The infrasonic swell's level, as a share of `level`, and its length (s).
    pub(super) infra: f32,
    pub(super) infra_seconds: (f32, f32),
    /// The longest a clap of this kind lasts (s).
    pub(super) seconds: f32,
}

pub(super) const KINDS: [Kind; 4] = [
    // A ground strike a few hundred metres away: a sharp crack and a short roll.
    Kind {
        ground: true,
        distance: (250.0, 600.0),
        length: (2_000.0, 3_000.0),
        height: (1_500.0, 2_500.0),
        in_cloud: (800.0, 2_000.0),
        branches: 4,
        step: 3.0,
        level: 1.0,
        cliff_hz: 2_500.0,
        edge: 0.9,
        infra: 0.01,
        infra_seconds: (0.10, 0.20),
        seconds: 9.0,
    },
    // A ground strike 1.5–3 km off: an onset, a body, a darkening tail.
    Kind {
        ground: true,
        distance: (1_500.0, 3_000.0),
        length: (2_000.0, 3_000.0),
        height: (2_000.0, 3_000.0),
        in_cloud: (500.0, 1_500.0),
        branches: 5,
        step: 4.0,
        level: 0.7,
        cliff_hz: 1_100.0,
        edge: 0.3,
        infra: 0.02,
        infra_seconds: (0.15, 0.30),
        seconds: 12.0,
    },
    // A long flash inside the cloud: rolling thunder that swells and fades.
    Kind {
        ground: false,
        distance: (1_500.0, 4_000.0),
        length: (5_000.0, 9_000.0),
        height: (3_000.0, 5_000.0),
        in_cloud: (0.0, 0.0),
        branches: 3,
        step: 5.0,
        level: 0.6,
        cliff_hz: 950.0,
        edge: 0.25,
        infra: 0.025,
        infra_seconds: (0.20, 0.40),
        seconds: 22.0,
    },
    // A ground strike 8–14 km off: a low grumble, the highs long gone.
    Kind {
        ground: true,
        distance: (8_000.0, 14_000.0),
        length: (3_000.0, 5_000.0),
        height: (2_500.0, 3_500.0),
        in_cloud: (2_000.0, 4_000.0),
        branches: 4,
        step: 6.0,
        level: 0.35,
        cliff_hz: 650.0,
        edge: 0.15,
        infra: 0.03,
        infra_seconds: (0.25, 0.45),
        seconds: 16.0,
    },
];

pub(super) fn kind(take: u8) -> &'static Kind {
    &KINDS[(take as usize).min(KINDS.len() - 1)]
}

/// The longest a clap of `kind` lasts at rate 1 (s): a plan spaces claps
/// with it.
pub fn clap_seconds(kind_number: u8) -> f32 {
    kind(kind_number).seconds
}
