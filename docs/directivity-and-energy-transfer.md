# Directivity Patterns and Energy Transfer Along Paths

## Background: What We Built in the Spatial Project

The TypeScript spatial audio garden (`/spatial`) was our first pass at modeling
directional hearing. The key design: every sound source and the listener both
carry a **ConeConfig** — an inner angle of full sensitivity, an outer angle
where attenuation begins, and a gain floor beyond the outer cone.

```
ConeConfig {
  coneInnerAngle: 360°    // full volume within this arc
  coneOuterAngle: 360°    // attenuated beyond this arc
  coneOuterGain: 1.0      // gain floor outside the outer cone
}
```

Sources defaulted to omnidirectional (360° / 360° / 1.0) — a campfire radiates
sound in all directions equally. But the listener ("YouNode") was configured as
a directional receiver:

```
YouNode hearing cone:
  coneInnerAngle:  30°    focused hearing — full volume
  coneOuterAngle:  90°    peripheral hearing — attenuated
  coneOuterGain:   0.3    behind you — 30% volume
```

The `DirectionalEmissionProcessor` computed the relative angle between the
listener's facing direction and each source, then applied a gain envelope:

```
if relative_angle <= inner_half:
    gain = 1.0
else if relative_angle <= outer_half:
    gain = lerp(1.0, outer_gain, (angle - inner) / (outer - inner))
else:
    gain = outer_gain
```

This gave us the experience of "turning your head" toward a sound source and
having it get louder — and sounds behind you becoming quieter but not silent.


## The Problem: Omnidirectional Is Not How Anything Works

In the spatial project, sources were omnidirectional by default and only the
listener had a cone. In atrium, where we're building toward ray-traced
acoustics, this simplification breaks down.

**Real sound sources are directional.** A trumpet bell projects a focused beam
of high-frequency energy forward. A human voice is roughly cardioid — louder
in front of the speaker, quieter behind their head. Even a "simple" source
like a campfire has a vertical directivity pattern shaped by the fire column
and surrounding structure.

**Real receivers are directional.** Human ears have complex frequency-dependent
directivity created by the pinna (outer ear), head shadow, and torso
reflections. At low frequencies we hear nearly omnidirectionally. At high
frequencies our hearing is sharply directional — roughly ±60° in the
horizontal plane, narrower vertically. This is the physical basis of HRTF.

A microphone — which is what each listener in atrium effectively is — has a
polar pickup pattern. Omnidirectional, cardioid, supercardioid, figure-8.
These aren't just theoretical models; they describe how much energy a receiver
captures as a function of the arrival angle.

When we say "since we're the mic, we can limit our range of hearing" — this is
exactly right. The listener isn't a point that absorbs all incoming energy.
It's a directional receiver with a sensitivity pattern.


## The Core Insight: Directivity Gates Energy Transfer Along Paths

Here is the key idea. Consider the full chain of how sound energy gets from a
source to a listener:

```
                    PATH
   SOURCE ─────────────────────────> LISTENER

   1. The source radiates energy in some direction
   2. That energy travels along a path (direct, reflected, diffracted)
   3. The energy arrives at the listener from some direction
   4. The listener's sensitivity in that arrival direction determines
      how much energy is actually captured
```

Each of these stages independently affects the final perceived level. They
multiply together:

```
perceived_energy = source_directivity(emission_angle)
                 × path_transfer(distance, reflections, occlusion)
                 × receiver_directivity(arrival_angle)
```

### Stage 1: Source Directivity — "How much energy leaves in this direction?"

The source has a facing direction and a radiation pattern. A function that maps
the angle between the source's forward vector and the direction toward the
listener to a gain value (0.0 to 1.0).

```
        source forward
            ↑
           /|\        ← cone of high emission
          / | \
         /  |  \
        / inner \
       /    |    \
      /     |     \
     /   outer     \
    /       |       \
   ─────────┼─────────  ← reduced emission beyond outer cone
```

For the cone model:
- Within the inner angle: full radiation (gain = 1.0)
- Between inner and outer: linearly interpolated
- Beyond the outer angle: reduced to a floor gain

For polar patterns:
- Omnidirectional: `gain = 1.0` (constant in all directions)
- Cardioid: `gain = 0.5 + 0.5 × cos(θ)` (heart-shaped, null at 180°)
- Supercardioid: `gain = 0.37 + 0.63 × cos(θ)` (tighter, small rear lobe)
- Figure-8: `gain = |cos(θ)|` (front and back, null at sides)

### Stage 2: Path Transfer — "How much energy survives the journey?"

This is everything between emission and reception:

- **Distance attenuation**: inverse-distance law, 1/r energy falloff
- **Air absorption**: frequency-dependent, high frequencies decay faster over
  distance (significant above ~2kHz over >10m)
- **Reflections**: each wall/surface bounce multiplies by the surface's
  absorption coefficient (0.0 = perfect absorber, 1.0 = perfect reflector)
- **Occlusion**: solid objects between source and listener block or diffract
  the path, reducing energy and filtering high frequencies
- **Ray tracing**: discovers which paths exist — direct line, first-order
  reflections off walls, higher-order reflections, diffraction around edges

This is where `cast_ray()` lives. Ray tracing answers: "what paths connect
this source to this listener, and what happens to the energy along each path?"

Each valid path has its own direction of departure from the source and
direction of arrival at the listener. This is crucial — a reflected path
arrives from a different angle than the direct path.

### Stage 3: Receiver Directivity — "How sensitive am I to energy arriving from this direction?"

The receiver (listener / microphone) has its own facing direction and
sensitivity pattern. Same math as source directivity, but applied to the
arrival angle rather than the emission angle.

```
        listener forward
             ↑
            /|\
           / | \        ← high sensitivity zone
          /  |  \
         / inner \
        /    |    \
       /     |     \
      /   outer     \
     /       |       \
    ─────────┼─────────  ← reduced sensitivity
             |
         (behind)       ← sounds here are quieter
```

For our atrium listeners:
- The 5.1 speaker system renders for a fixed room — speakers don't have
  directional pickup, they project. But the **virtual listener** in the scene
  that determines what the speakers play absolutely has directivity.
- Each binaural headphone listener has their own position and orientation.
  Their receiver pattern approximates how human ears work — sensitive in front,
  attenuated behind, with frequency-dependent rolloff.


## How It All Composes: A Concrete Example

Imagine a musician (source) and a listener in the atrium. The musician faces
north, playing a trumpet. The listener faces east.

```
         N
         ↑
         🎺 → musician facing north
              (trumpet projects forward)


                       👂 → listener facing east
```

### Direct path

The direct path goes from the trumpet roughly southeast to the listener.

1. **Source directivity**: The trumpet faces north. The path to the listener
   departs roughly southeast — say 135° off the trumpet's forward axis. The
   trumpet's radiation pattern at 135° might give gain = 0.15 (most energy
   goes forward through the bell).

2. **Path transfer**: Direct line, 8 meters, no occlusion.
   Distance gain = ref / (ref + rolloff × (8 - ref)) ≈ 0.2

3. **Receiver directivity**: The sound arrives from roughly the listener's
   left-rear (west-ish). If the listener has a cardioid-like pattern, the
   sensitivity at that arrival angle (≈135° off forward) might be 0.25.

```
direct_energy = 0.15 × 0.2 × 0.25 = 0.0075
```

### Reflected path (off north wall)

The same trumpet sound bounces off the north wall and arrives at the listener
from the north (the listener's left side).

1. **Source directivity**: This path departs northward from the trumpet —
   that's 0° off the trumpet's forward axis. Gain = 1.0 (full blast through
   the bell).

2. **Path transfer**: Total path length 12m (trumpet → north wall → listener),
   one reflection (wall absorption coefficient 0.7).
   Distance gain ≈ 0.12, × wall coefficient 0.7 = 0.084

3. **Receiver directivity**: Sound arrives from the north, which is to the
   listener's left (90° off their forward east-facing direction). Cardioid
   sensitivity at 90° = 0.5.

```
reflected_energy = 1.0 × 0.084 × 0.5 = 0.042
```

The reflected path carries **5.6× more perceived energy** than the direct
path — because the trumpet's directivity strongly favors the reflected path
(sound goes forward through the bell, bounces off the north wall, comes back
to the listener from a more sensitive angle).

Without directivity modeling, both paths would differ only by distance and
wall absorption, missing the dominant effect.


## Why This Matters for Atrium

Atrium targets two rendering modes:

1. **5.1 speaker array** — fixed speakers in a physical room, rendering the
   virtual atrium. One shared listener position, projected to surround channels
   via VBAP or ambisonics.

2. **Per-listener binaural headphones** — each person has their own position
   and facing direction, receiving a personal stereo mix via HRTF convolution.

In both cases, directivity patterns are essential:

- **Source directivity** determines which reflections carry the most energy.
  A directional source in a reverberant room sounds fundamentally different
  from an omnidirectional one — the direct-to-reverberant ratio changes, and
  specific reflections become much louder or quieter based on where the source
  "points."

- **Receiver directivity** determines what each listener actually perceives.
  Two listeners facing different directions hear different mixes of the same
  reflections. This is the whole point of per-listener rendering — if
  receivers are omnidirectional, rotating your head changes the stereo panning
  but not the energy balance. With directional receivers, turning toward a
  sound makes it louder. Turning away makes it quieter. This matches how
  human hearing actually works.

Together, these create the sense that sources and listeners exist in a real
physical space with real physical characteristics. The ray tracer finds the
paths. Directivity gates the energy on those paths.


## Relationship to the Existing Codebase

### What we have now (atrium, Phase 1)

```
perceived_gain = distance_gain(listener, source)          ← path stage only
stereo_balance = stereo_pan(listener, source_position)    ← azimuth for L/R
```

Both source and listener are treated as omnidirectional. `stereo_pan()`
computes the azimuth from listener to source and maps it to L/R gains, but
this is panning (placing the sound in the stereo field) not directivity
(attenuating based on facing direction). The listener's yaw rotates the stereo
image but doesn't change the perceived loudness.

### What we had in the spatial project

```
perceived_gain = distance_gain
               × listener_cone_gain(relative_angle)      ← receiver stage
```

Source directivity was structurally present (`ConeConfig` on every
`SoundNode`) but defaulted to omnidirectional. The listener's hearing cone
(`DirectionalEmissionProcessor`) applied a cone-based attenuation to each
source based on the angle from the listener's facing direction.

### What atrium needs

```
perceived_gain = source_directivity(emission_angle)       ← NEW
               × path_transfer(ray_result)                ← cast_ray() + distance
               × receiver_directivity(arrival_angle)      ← NEW
stereo_balance = stereo_pan(listener, arrival_direction)   ← already have this
```

Where `arrival_direction` comes from the ray result (not necessarily the
direct line to the source — a reflected path arrives from a different angle).


## Design Sketch: DirectivityPattern

A directivity pattern is a function from angle to gain. It needs to be
lightweight (called per-source per-path per-sample-buffer), deterministic,
and describable with a small number of parameters.

```
DirectivityPattern
├── Omnidirectional                          gain = 1.0
├── Cone { inner, outer, outer_gain }        three-parameter cone model
├── Polar { alpha }                          gain = alpha + (1 - alpha) * cos(theta)
│   ├── alpha = 1.0  → omnidirectional
│   ├── alpha = 0.5  → cardioid
│   ├── alpha = 0.37 → supercardioid
│   └── alpha = 0.25 → hypercardioid
└── (future: Measured / tabulated for HRTF-derived patterns)
```

Both `SoundSource` and `Listener` would carry a `DirectivityPattern` and a
facing direction. The gain for a given angle is computed as:

```rust
fn gain_at_angle(&self, angle_radians: f32) -> f32
```

Where `angle_radians` is the absolute angle between the entity's forward
vector and the direction of interest (0 = directly ahead, π = directly
behind).

For the cone model:
```rust
let half_inner = self.inner / 2.0;
let half_outer = self.outer / 2.0;
if angle <= half_inner {
    1.0
} else if angle <= half_outer {
    let t = (angle - half_inner) / (half_outer - half_inner);
    1.0 + t * (self.outer_gain - 1.0)   // lerp from 1.0 to outer_gain
} else {
    self.outer_gain
}
```

For the polar model:
```rust
self.alpha + (1.0 - self.alpha) * angle.cos()
```


## Open Questions

1. **Frequency dependence.** Real directivity is frequency-dependent — sources
   and ears are more directional at high frequencies. Do we model this with
   per-band directivity, or keep it broadband for now?

2. **Elevation.** Current listener has yaw only. Source and receiver directivity
   in the vertical plane matters (sound from above vs. below). When do we add
   pitch to the listener?

3. **Per-source patterns.** Should every source carry its own pattern, or do we
   define pattern "types" (speech, instrument, ambient) and assign them?

4. **HRTF integration.** The receiver directivity pattern overlaps conceptually
   with HRTF. When we add HRTF convolution for binaural output, how do we
   avoid double-counting the directional attenuation that HRTF already encodes?
