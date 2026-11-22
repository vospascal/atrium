use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::pipeline::renderers::binaural::diffuse_field::shared_head_filters;

fn shipped_head(sample_rate: f32) -> Arc<ArcHead> {
    let path = crate::pipeline::PipelineParams::default().hrtf_path;
    let sofa = sofar::reader::OpenOptions::new()
        .sample_rate(sample_rate)
        .open(&path)
        .expect("the shipped HRIRs");
    shared_head_filters(&path, &sofa, sample_rate).arcs
}

/// The band's directions cover the horizon once: an arc's shares add up to
/// one however it is cut, and a mirrored arc mirrors the ears, the head
/// being nearly symmetric: an arc 40° to the left gives the left ear what
/// the same arc to the right gives the right, within 0.3 dB per bin
/// averaged over 1 to 4 kHz.
#[test]
fn an_arc_and_its_mirror_swap_the_ears() {
    let head = shipped_head(48_000.0);
    let left = head.spectra(40.0, 50.0);
    let right = head.spectra(-40.0, 50.0);
    let bin_hz = 48_000.0 / (2 * (left.left_power.len() - 1)) as f64;
    let band = (1000.0 / bin_hz) as usize..(4000.0 / bin_hz) as usize;
    let mean = |values: &[f64]| values[band.clone()].iter().sum::<f64>();
    let difference = 10.0 * (mean(&left.left_power) / mean(&right.right_power)).log10();
    assert!(
        difference.abs() < 0.3,
        "mirror differs by {difference:+.2} dB"
    );
    assert!(
        mean(&left.left_power) > 2.0 * mean(&left.right_power),
        "nearer ear louder"
    );
}

/// The time difference of an arc straight ahead is nothing, and of one to
/// the left the head's, positive: the left ear first by some 0.7 ms. A half
/// circle ahead is opposed between the ears around 500 Hz, which must not
/// be read as a time.
#[test]
fn the_time_difference_is_read_from_the_phase_and_not_from_a_change_of_sign() {
    let head = shipped_head(48_000.0);
    let bin_hz = |spectra: &horizon::ArcSpectra| 48_000.0 / (2 * (spectra.cross.len() - 1)) as f64;
    for width in [30.0, 180.0] {
        let ahead = head.spectra(0.0, width);
        let lag = design::interaural_lag(&ahead.cross, bin_hz(&ahead));
        assert!(lag.abs() < 5e-6, "{width}° ahead: {:.1} µs", lag * 1e6);
    }
    let left = head.spectra(90.0, 30.0);
    let lag = design::interaural_lag(&left.cross, bin_hz(&left));
    assert!(
        (600e-6..800e-6).contains(&lag),
        "30° to the left: {:.1} µs",
        lag * 1e6
    );
}

/// A width asked for while audio runs comes back from the designer's
/// thread, and a table handed back is taken.
#[test]
fn the_designer_designs_a_width_asked_for() {
    let head = shipped_head(48_000.0);
    let mut designer = ArcDesigner::start(head);
    assert!(designer.request(3, 45));
    let deadline = Instant::now() + Duration::from_secs(20);
    let table = loop {
        if let Some(table) = designer.designed() {
            break table;
        }
        assert!(Instant::now() < deadline, "no table in 20 s");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(table.width_degrees(), 45);
    designer.retire(table);
}

/// Bearings are picked nearest, in steps of 5°, all the way round.
#[test]
fn the_nearest_bearing_wraps_round() {
    assert_eq!(ArcTable::nearest(0.0), 0);
    assert_eq!(ArcTable::nearest(2.4f32.to_radians()), 0);
    assert_eq!(ArcTable::nearest(2.6f32.to_radians()), 1);
    assert_eq!(ArcTable::nearest(-5f32.to_radians()), 71);
    assert_eq!(ArcTable::nearest(360f32.to_radians()), 0);
}

/// Widths are designed in whole degrees, from the narrowest to just short
/// of all round.
#[test]
fn widths_are_whole_degrees_short_of_all_round() {
    assert_eq!(whole_degrees(90.4), 90);
    assert_eq!(whole_degrees(3.0), 10);
    assert_eq!(whole_degrees(359.7), 359);
}
