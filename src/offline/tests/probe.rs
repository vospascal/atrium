//! The probe through the tool: a probe given before the render holds from
//! the first sample, so a file of nothing is silence and the parts heard
//! alone add up to the whole.

use atrium_core::probe::Probe;

use super::{on_headphones, peak, render, storm_on_speakers, MEADOW, STORM};
use crate::offline::RenderSettings;

const NOTHING: Probe = Probe {
    direct: false,
    reflections: false,
    reverb: false,
    ambient: false,
    ..Probe::WHOLE
};

const PARTS: [Probe; 4] = [
    Probe {
        direct: true,
        ..NOTHING
    },
    Probe {
        reflections: true,
        ..NOTHING
    },
    Probe {
        reverb: true,
        ..NOTHING
    },
    Probe {
        ambient: true,
        ..NOTHING
    },
];

/// The meadow on headphones and the storm on 5.1 speakers: with every part
/// hidden, digital silence from the first sample; each part alone, the four
/// files add up to the whole within float rounding, from the first sample.
#[test]
fn the_parts_alone_add_up_to_the_whole_and_nothing_is_silence() {
    for (scene, settings) in [
        (MEADOW, on_headphones(8.0)),
        (STORM, storm_on_speakers(8.0)),
    ] {
        let with = |probe: Probe| {
            render(
                scene,
                &RenderSettings {
                    probe,
                    ..settings.clone()
                },
            )
        };
        let nothing = with(NOTHING);
        assert!(
            nothing.frames().iter().all(|sample| *sample == 0.0),
            "{scene}: with every part hidden, {} is heard",
            peak(nothing.frames())
        );

        let whole = with(Probe::WHOLE);
        let parts = PARTS.map(with);
        let whole_peak = peak(whole.frames());
        assert!(whole_peak > 1e-4, "{scene}: silent");
        let mut largest_difference = 0f32;
        for (index, sample) in whole.frames().iter().enumerate() {
            let sum: f32 = parts.iter().map(|part| part.frames()[index]).sum();
            largest_difference = largest_difference.max((sum - sample).abs());
        }
        // Each part is its own f32 render; the late reverb's feedback
        // network rounds each differently and carries the rounding along.
        // Measured: 3e-5 of the peak for the storm, under -80 dB.
        eprintln!(
            "{scene}: the parts differ from the whole by {:.1e} of its peak",
            largest_difference / whole_peak
        );
        assert!(
            largest_difference <= 1e-4 * whole_peak,
            "{scene}: the parts differ from the whole by {largest_difference} (peak {whole_peak})"
        );
        // The open meadow has no faces to reflect from, so its reflections
        // alone may be silent; the direct sound and the ambient never are.
        assert!(peak(parts[0].frames()) > 0.0, "{scene}: no direct sound");
        assert!(peak(parts[3].frames()) > 0.0, "{scene}: no ambient sound");
    }
}
