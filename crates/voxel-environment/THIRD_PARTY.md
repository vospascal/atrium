# Third-party atmosphere implementation

The WGSL atmosphere provider in this crate is adapted from
[`JolifantoBambla/webgpu-sky-atmosphere`](https://github.com/JolifantoBambla/webgpu-sky-atmosphere),
which is released under the MIT license. That implementation is based on
Sébastien Hillaire's 2020 sky-atmosphere technique and includes code derived from
Epic Games' implementation.

Specifically, we adapted its **LUT path** — four persistent lookup tables populated by
compute passes — rather than its full-resolution ray-marching path. The adapted files are:

- `shaders/lut/transmittance.wgsl`
- `shaders/lut/multiple_scattering.wgsl`
- `shaders/lut/sky_view.wgsl`
- `shaders/lut/aerial_perspective.wgsl`
- `shaders/lut/common.wgsl`
- `shaders/environment/hillaire.wgsl` — the sampling half

Adapted, not copied: the LUTs are parameterized for this engine's world scale
(`src/scale.rs`) and the aerial-perspective froxel grid is camera-relative. The starting LUT
sizes are Jolifanto's, pinned by a test in `src/adapters/hillaire.rs`.
`shaders/environment/{common,appearance,dispatch}.wgsl` and all Rust in this crate are
Atrium's own.

`sebh/UnrealEngineSkyAtmosphere` remains the reference implementation and comparison
harness. Bruneton's precomputed-atmospheric-scattering work underlies the technique and is
credited where applicable.

## MIT license notice

Copyright (c) 2024 Lukas Herzberger

Copyright (c) 2020 Epic Games, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy of this
software and associated documentation files (the "Software"), to deal in the Software
without restriction, including without limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

The upstream repositories and their license texts remain the authoritative notices.
