# HRTF Data

## default.sofa — MIT KEMAR

- **What**: Head-Related Transfer Function impulse responses (SimpleFreeFieldHRIR)
- **Measured on**: KEMAR mannequin with normal pinna
- **Researchers**: Gardner, W. G., and Martin, K. D. (1995). "HRTF measurements of a KEMAR," J Acoust Soc Am 97, 3907-3908.
- **Original source**: http://sound.media.mit.edu/resources/KEMAR.html
- **SOFA conversion by**: Piotr Majdak, Acoustics Research Institute, Austrian Academy of Sciences
- **Downloaded from**: https://www.sofaconventions.org/mediawiki/index.php/Files
- **Measurements**: 710 positions, full sphere (elev −40° to +90°), sampled at 44.1 kHz
- **HRIR length**: 128 taps per ear (~2.9 ms)
- **Format**: SOFA (AES69) — HDF5 container with standardized spatial audio metadata
- **License**: No usage restrictions. Please cite Gardner and Martin (1995).

## Adding other HRTF datasets

Drop any AES69-compliant `.sofa` file here and reference it in your scene config.
Browse available datasets at https://www.sofaconventions.org/mediawiki/index.php/Files.
