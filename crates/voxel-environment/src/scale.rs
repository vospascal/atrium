//! World-unit conversion for the atmosphere provider.

/// Jolifanto's atmosphere equations use kilometres internally. Atrium's
/// camera and lighting positions are expressed in metres, so one kilometre is
/// 1000 engine units at the renderer boundary.
pub const FROM_KILOMETERS_SCALE: f32 = 1000.0;

/// Convert an engine coordinate scale (world units per metre) to Jolifanto's
/// `fromKilometersScale` value. For Atrium's metre-space camera this is 1000;
/// for detail-cell coordinates (`0.125 m` per cell) it is 8000.
pub const fn from_kilometers_scale(world_units_per_meter: f32) -> f32 {
    FROM_KILOMETERS_SCALE * world_units_per_meter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_atrium_coordinate_spaces() {
        assert_eq!(FROM_KILOMETERS_SCALE, 1000.0);
        assert_eq!(from_kilometers_scale(1.0), 1000.0);
        assert_eq!(from_kilometers_scale(8.0), 8000.0);
    }
}
