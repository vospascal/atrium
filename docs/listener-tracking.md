# Listener Tracking for VBAP — Technical Report

> Research notes for adding real-time listener position and head orientation tracking
> to the Atrium spatial audio engine. Evaluates sensor technologies, transport protocols,
> and integration architecture.

---

## Table of Contents

1. [Why Track Listeners?](#1-why-track-listeners)
2. [Tracking Requirements for Spatial Audio](#2-tracking-requirements-for-spatial-audio)
3. [Sensor Technologies](#3-sensor-technologies)
   - [24 GHz mmWave Radar (LD2450, Rd-03D)](#31-24-ghz-mmwave-radar)
   - [UWB Ranging (DW3000)](#32-uwb-ranging-dw3000)
   - [Comparison & Recommendation](#33-comparison--recommendation)
4. [Head Orientation Tracking (IMU)](#4-head-orientation-tracking-imu)
5. [Transport: MQTT vs Alternatives](#5-transport-mqtt-vs-alternatives)
6. [VBAP Compensation Algorithms](#6-vbap-compensation-algorithms)
7. [System Architecture](#7-system-architecture)
8. [Bill of Materials](#8-bill-of-materials)
9. [Open Questions & Next Steps](#9-open-questions--next-steps)
10. [References](#10-references)

---

## 1. Why Track Listeners?

VBAP assumes the listener sits at the geometric center of the speaker array (the "sweet spot"). When the listener moves off-center:

- **Angular distortion**: Phantom source images shift toward the nearer speaker due to precedence effect (Haas). A 30 cm offset toward a speaker causes ~0.9 ms arrival time difference, enough to pull the image.
- **Level imbalance**: Inverse-distance law makes nearer speakers louder before VBAP gains are applied. A 2 m vs 4 m distance is a 6 dB advantage.
- **Image collapse**: At ≥1 m offset, phantom images collapse almost entirely to the nearest speaker.

With listener tracking, we can compensate by recalculating VBAP gains from the listener's actual position and applying per-speaker delay correction. Research (Frank & Zotter, 2012) shows this restores localization accuracy to near-sweet-spot quality.

For the binaural headphone path, head *orientation* tracking is essential to select the correct HRTF filters and maintain externalization.

---

## 2. Tracking Requirements for Spatial Audio

### Position Tracking (for VBAP speaker path)

| Parameter             | Minimum     | Recommended | Notes |
|-----------------------|-------------|-------------|-------|
| Position accuracy     | 10 cm       | 2–5 cm      | 1° MAA at 3 m ≈ 5 cm offset |
| Update rate           | 10 Hz       | 30–60 Hz    | Walking speed ~1.5 m/s → 2.5 cm/frame at 60 Hz |
| Tracking-to-audio latency | < 80 ms | < 30 ms     | 50 ms × 1.5 m/s = 7.5 cm position error |
| Identity persistence  | Required    | Required    | Must know *which* listener is where |

### Head Orientation Tracking (for binaural headphone path)

| Parameter             | Minimum     | Recommended | Notes |
|-----------------------|-------------|-------------|-------|
| Angular accuracy      | 5°          | 1–2°        | Below MAA in frontal plane |
| Update rate           | 60 Hz       | 100–200 Hz  | Fast head turns reach 600°/s |
| Tracking-to-audio latency | < 20 ms | < 10 ms     | Brungart et al. (2004): <15 ms for unimpaired localization |
| DOF                   | 3 (yaw/pitch/roll) | 3      | Yaw most important for azimuthal HRTF selection |

### Latency Budget

```
Sensor measurement:              2–5 ms
Wireless transmission (WiFi):    5–15 ms
Processing (EKF / fusion):       1–2 ms
Audio gain/HRTF computation:     < 1 ms
Audio buffer (256 samples@48k):  5.3 ms
DAC output:                      1–3 ms
────────────────────────────────────────
Total estimate:                  15–31 ms
```

This is within the <50 ms threshold for VBAP and approaching the <15 ms target for binaural. Using 256-sample buffers on the NUC is key.

---

## 3. Sensor Technologies

### 3.1. 24 GHz mmWave Radar

#### LD2450 (HiLink)

| Spec                 | Value |
|----------------------|-------|
| Frequency            | 24 GHz FMCW |
| Max range            | ~6 m (moving), ~4 m reliable |
| Max simultaneous targets | 3 |
| Output               | X, Y (mm) + speed (cm/s) per target |
| Field of view        | ~60° horizontal, ~60° vertical |
| Interface            | UART 256000 baud |
| Update rate          | ~10 Hz |
| Positional accuracy  | ±15–30 cm at short range, degrades with distance |
| Power                | 5 V, ~150 mA |
| Price                | $4–8 (AliExpress), ~$10–15 (Western) |

Binary UART protocol: frame header `0xAA 0xFF 0x03 0x00`, followed by 3 target slots each with 16-bit signed X, Y, speed, and resolution fields. Empty slots are zeroed.

**Critical limitation: no persistent target identity.** Target indices are arbitrary per-frame and can swap. You must implement your own tracking layer (Hungarian algorithm or nearest-neighbor + Kalman filter).

#### Rd-03D (Ai-Thinker)

| Spec                 | Value |
|----------------------|-------|
| Frequency            | 24 GHz FMCW |
| Max range            | ~6–8 m (claimed) |
| Max simultaneous targets | 3 |
| Output               | X, Y + speed per target |
| Interface            | UART 256000 baud (different protocol than LD2450) |
| Update rate          | ~10 Hz |
| Accuracy             | Comparable to LD2450 |
| Price                | $3–6 |

Less community support and fewer libraries than the LD2450. Protocol is conceptually similar but not wire-compatible.

#### LD2461 (HiLink, newer)

Claims up to 5 targets. Less community testing. Worth evaluating if the 3-target limit is a problem.

#### Radar Limitations for Spatial Audio

1. **No target identity** — When two people cross paths, target IDs swap or merge. This is catastrophic for multi-listener audio routing.
2. **3-target limit** — Hard firmware constraint.
3. **10 Hz update rate** — Marginal for smooth VBAP gain interpolation.
4. **±20–50 cm accuracy** — Coarse for compensated VBAP where 5 cm is ideal.
5. **Stationary targets** — Can fade from detection (Doppler-dependent).
6. **No orientation data** — Cannot determine head facing direction.

#### When Radar Makes Sense

- Presence detection / occupancy counting
- Coarse zone tracking (is someone in the room?)
- Supplementing a primary tracking system
- Single-listener scenarios where identity is not ambiguous
- Budget prototyping before committing to UWB

### 3.2. UWB Ranging (DW3000)

The Qorvo DW3000 (formerly Decawave) is a second-generation UWB transceiver compliant with IEEE 802.15.4z.

#### Chip Specifications

| Spec                 | Value |
|----------------------|-------|
| Ranging accuracy     | ±10 cm (line-of-sight) |
| Max range            | ~30 m indoor, ~70 m outdoor |
| Channels             | Ch 5 (6.5 GHz), Ch 9 (8.0 GHz) |
| Data rates           | 850 kbps, 6.8 Mbps |
| Interface            | SPI (up to 36 MHz) |
| Supply               | 1.8 V core (LDO accepts 2.8–3.6 V) |
| TX current           | ~60–80 mA peak |
| RX current           | ~55–70 mA |
| Deep sleep            | ~1 µA |
| Package              | QFN 4.97 × 4.97 mm |
| Security             | STS (Scrambled Timestamp Sequence) |

#### DW3000 vs DW1000

| Feature      | DW1000         | DW3000         |
|-------------|----------------|----------------|
| Standard    | 802.15.4-2011  | 802.15.4z      |
| Channels    | 6              | 2 (5 and 9)    |
| Phone interop | No           | FiRa/CCC compatible |
| Power       | ~120+ mA TX    | ~60–80 mA TX   |
| Security    | None           | STS            |
| Package     | 6×6 mm         | 5×5 mm         |

Newer variants: **DW3110/DW3120** integrate an on-chip MCU; **DW3120** adds PDoA (Phase Difference of Arrival) for angle estimation. **DW3720** is the latest with FiRa stack on-chip.

#### Positioning Methods

**TWR (Two-Way Ranging)** — recommended for our use case:
- Tag sends poll → anchor responds → round-trip time gives distance
- DS-TWR (double-sided) uses 3 messages to cancel clock drift
- No clock synchronization between devices needed
- 5–15 tags at high update rates before airtime saturation
- Simpler firmware, bidirectional communication

**TDoA (Time Difference of Arrival):**
- Anchors must be clock-synchronized (~1 ns precision)
- Tags are transmit-only (blink), scales to hundreds of tags
- More complex setup (wired sync backbone or wireless CCP)
- Overkill for <10 listeners

#### Available Modules

| Board                          | Description                           | Price  |
|-------------------------------|---------------------------------------|--------|
| **Makerfabs ESP32-S3 + DW3000** | ESP32-S3 + DW3000, PCB antenna or SMA | ~$20–30 |
| Makerfabs UWB Pro             | Improved antenna                       | ~$25–35 |
| Qorvo DWM3000EVB             | Official eval board for nRF52840-DK    | ~$30–40 |
| Qorvo DWS3000 Shield         | Arduino-compatible shield              | ~$30   |

The **Makerfabs ESP32-S3 + DW3000** is the sweet spot: WiFi for data backhaul, DW3000 for ranging, USB-C for programming, ~$25 per unit.

#### Anchor/Tag Architecture

| Positioning | Min Anchors | Recommended |
|-------------|-------------|-------------|
| 2D (x, y)   | 3           | 4 (corners) |
| 3D (x, y, z) | 4          | 5–6         |

**Placement**: Mount 4 anchors at ceiling corners (~2.5–3 m height), pointing down. This gives good geometric diversity (low DOP) and clear line-of-sight. The UWB coordinate system must be aligned with the speaker/room coordinate system — measure anchor positions in the same reference frame as speaker positions.

#### Update Rates

With DS-TWR at 6.8 Mbps:

| Tags | Anchors | Rate per Tag |
|------|---------|-------------|
| 1    | 4       | 50–80 Hz    |
| 2    | 4       | 25–40 Hz    |
| 3    | 4       | 17–27 Hz    |
| 4    | 4       | 12–20 Hz    |

A TDMA superframe schedules each tag to range with all anchors in sequence. With 2–3 listeners, 20–40 Hz per listener is achievable — adequate for position tracking.

#### Challenges

- **NLOS (body blockage)**: A listener's own body can block 1–2 anchors. Use 4+ anchors so 2–3 LOS measurements remain. The DW3000 provides CIR (Channel Impulse Response) data for NLOS detection.
- **Multipath**: UWB's ~2 ns pulses provide excellent temporal resolution, naturally separating direct path from reflections. Acoustic treatment in the atrium actually helps UWB too.
- **Antenna delay calibration**: Each DW3000 unit has a slightly different internal antenna delay (~514–516 ns). Calibrate per unit at a known distance to avoid systematic cm-level bias.
- **WiFi 6E coexistence**: Channel 5 (6.5 GHz) is near WiFi 6E. Prefer channel 9 (8 GHz) in dense WiFi environments.

#### Phone UWB (iPhone / Android)

- **iPhone (U1/U2 chip)**: Locked to Apple's MFi ecosystem. Cannot freely range with custom anchors. Not suitable for RTLS.
- **Android (Galaxy S21+, Pixel 6 Pro+)**: More open via `android.uwb` API (Android 13+), supports FiRa ranging sessions. Complex to set up and not all implementations are interoperable.
- **Recommendation**: Use dedicated DW3000 tags. Full control, no platform dependency.

### 3.3. Comparison & Recommendation

| Feature              | LD2450 Radar    | DW3000 UWB       |
|---------------------|-----------------|-------------------|
| Accuracy            | ±20–50 cm       | **±10 cm**        |
| Update rate         | 10 Hz           | **20–80 Hz**      |
| Max targets         | 3               | **Many (TDMA)**   |
| Target identity     | **None**        | **Unique tag ID** |
| Wearable required   | **No**          | Yes (tag)         |
| Orientation data    | No              | No                |
| Setup complexity    | Low             | Medium            |
| Cost per unit       | **$5–8**        | ~$25              |
| Total system cost   | ~$15–30         | ~$200–250         |

**Recommendation: DW3000 UWB** for primary position tracking.

Rationale:
1. **Identity persistence** is non-negotiable for multi-listener audio routing. Radar cannot reliably tell listener A from listener B.
2. **±10 cm accuracy** vs ±30 cm is the difference between 1.5° and 5° angular error at 3 m — meaningful for VBAP.
3. **20–40 Hz update rate** is sufficient; radar's 10 Hz is marginal.
4. Each listener already wears headphones — integrating a UWB tag is natural.
5. Total system cost (~$200) is very reasonable for the accuracy gained.

Radar remains useful as a supplementary presence detector or for tracking un-tagged people in the room.

---

## 4. Head Orientation Tracking (IMU)

Position tracking alone is insufficient for binaural rendering. We need head orientation (yaw, pitch, roll) at high update rates.

### Sensor Comparison

| Sensor     | DOF | On-chip Fusion | Quaternion Output | Max Rate | Price (breakout) |
|------------|-----|----------------|-------------------|----------|-----------------|
| **BNO085** | 9   | Yes (CEVA FSP200) | **Yes, calibrated** | 400 Hz | ~$20–25 |
| **BNO086** | 9   | Yes (improved) | **Yes, calibrated** | 400 Hz | ~$25–30 |
| BNO055     | 9   | Yes            | Yes               | 100 Hz  | ~$15–20 |
| ICM-20948  | 9   | DMP (lower quality) | Yes (via DMP)  | 225 Hz  | ~$10–15 |
| ICM-42688  | 6   | **No**         | **No**            | 32 kHz  | ~$8–12 |
| BMI270     | 6   | No             | No                | 1.6 kHz | ~$8–12 |

### Recommendation: BNO085

The BNO085's CEVA FSP200 sensor fusion engine is the gold standard for orientation sensing. It outputs calibrated quaternions directly — no fusion math needed on the ESP32. This is critical for low-latency, low-jitter orientation data.

**Key output modes:**
- **Rotation Vector** (9-DOF with magnetometer): Absolute heading, but magnetometer is unreliable indoors near speakers and electronics.
- **Game Rotation Vector** (6-DOF, accel + gyro only): No absolute heading, but no mag interference. Yaw drifts ~1–3°/min with good gyro.

**Recommendation: Use Game Rotation Vector** (6-DOF, no magnetometer).

Indoor environments are terrible for magnetometers:
- Speaker magnets cause massive distortion
- Steel in walls/floors, electronics, power supplies all interfere
- Distortion is spatially varying

Yaw drift can be corrected by:
- Periodic recalibration ("face the front wall and press button")
- Deriving heading from UWB position velocity vector (when walking)
- Occasional magnetometer reading in a "clean" area of the room

### Mounting on Headphones

- **Placement**: Top of headband, centered, close to the center of head rotation.
- **Coupling**: Must be rigid — any flex adds noise. 3D-printed clip is ideal.
- **Weight**: BNO085 breakout + ESP32 + 150 mAh LiPo ≈ 15–25 g. Acceptable.
- **Form factor**: ~30×30×15 mm on custom PCB, ~40×50×20 mm with breakout boards.
- **Battery life**: At 50 Hz reporting, ~4–8 hours with 150 mAh LiPo.

### Interface

BNO085 uses SHTP (Sensor Hub Transport Protocol) over I2C (400 kHz), SPI (3 MHz), or UART. Libraries:
- Arduino: SparkFun BNO08x library
- ESP-IDF: I2C/SPI drivers + SHTP port
- Reference: SlimeVR ESP32 firmware (BNO085 + WiFi, open-source)

---

## 5. Transport: MQTT vs Alternatives

### MQTT

| Aspect              | Value |
|---------------------|-------|
| QoS for position data | **QoS 0** (fire-and-forget) — stale positions are useless, no retransmit needed |
| Latency (WiFi, QoS 0) | 5–15 ms (ESP32 to NUC) |
| Latency (localhost) | < 0.5 ms |
| Throughput capacity | 50 msg/s per listener is trivial (~0.5% of Mosquitto's capacity) |
| ESP32 support       | Excellent (esp-mqtt in ESP-IDF, PubSubClient/AsyncMqttClient for Arduino) |

**Topic structure:**

```
atrium/listeners/{listener_id}/position      → packed binary: [x:f32, y:f32, z:f32] (12 bytes)
atrium/listeners/{listener_id}/orientation   → packed binary: [w:f32, x:f32, y:f32, z:f32] (16 bytes)
atrium/listeners/{listener_id}/status        → JSON: battery, tracking quality
atrium/sensors/{sensor_id}/raw               → raw ranges/readings for debugging
atrium/system/calibration                    → retained: anchor positions, room config
```

Use compact binary payloads (not JSON) for position/orientation — 12–28 bytes vs 50+ bytes in JSON.

### Broker Options

| Broker      | Language | Memory   | Throughput    | Notes |
|-------------|----------|----------|---------------|-------|
| **Mosquitto** | C      | ~2–5 MB  | ~100K msg/s   | De facto standard, trivial to install |
| NanoMQ      | C        | ~2–3 MB  | ~1M msg/s     | Multi-threaded, modern |
| rumqttd     | Rust     | ~5–10 MB | ~100K+ msg/s  | Could embed in Rust binary |

**Recommendation: Mosquitto.** Battle-tested, available in every package manager (`apt install mosquitto`), negligible overhead on the NUC.

### Rust MQTT Client

**rumqttc** — pure Rust, Tokio-native async, active maintenance by Bytebeam. Clean `AsyncClient` + `EventLoop` API.

```rust
use rumqttc::{MqttOptions, AsyncClient, QoS};

let mut opts = MqttOptions::new("atrium-engine", "localhost", 1883);
opts.set_keep_alive(Duration::from_secs(5));

let (client, mut eventloop) = AsyncClient::new(opts, 10);
client.subscribe("atrium/listeners/+/position", QoS::AtMostOnce).await?;

while let Ok(event) = eventloop.poll().await {
    // Parse position, send to audio thread via rtrb
}
```

### Alternatives Considered

| Transport  | Latency     | Pros                                | Cons |
|-----------|-------------|-------------------------------------|------|
| **MQTT**  | 5–15 ms WiFi | Structured pub/sub, standard, great ESP32 support | Broker hop |
| Raw UDP   | 1–5 ms WiFi  | Lowest latency, zero overhead       | No routing, build own framing |
| ZeroMQ    | 1–5 ms       | Brokerless pub/sub                  | No ESP32 support |
| WebSocket | 5–15 ms      | Full-duplex, good Rust support      | TCP head-of-line blocking |
| ESP-NOW   | < 1 ms       | Peer-to-peer, ultra-low latency     | ESP32-only, needs gateway to NUC |
| BLE       | 7.5–30 ms    | Low power                           | High latency, limited throughput |

**MQTT wins** on balance: structured pub/sub, excellent tooling and ESP32 support, reasonable latency. If sub-5 ms is ever needed, raw UDP is the fallback. ESP-NOW is interesting as a sensor-to-gateway transport (ESP32 radar → ESP32 gateway → USB → NUC).

---

## 6. VBAP Compensation Algorithms

Our existing `compute_gains_vbap` already computes speaker directions from the listener's actual position (not a fixed sweet spot). This is "reformulated VBAP." Two additional compensations are needed:

### Distance Compensation

Scale VBAP gains by inverse distance to each speaker to counteract level imbalance:

```
g_i_compensated = g_i_vbap × (d_ref / d_i)
```

Where `d_ref` is the nominal listening distance (e.g., mean of all speaker distances) and `d_i` is the actual distance from listener to speaker `i`. Then re-normalize for constant power.

### Delay Compensation

The most impactful improvement for off-center listening. Equalize arrival times at the listener's tracked position:

```
delay_i = (d_max − d_i) / 343.0    (seconds)
```

Where `d_max` is the distance to the farthest active speaker. Apply as a fractional-sample delay per speaker channel.

This restores the precedence-effect balance, preventing phantom images from collapsing toward the nearer speaker.

### Gain Interpolation

Between tracking updates, interpolate gains per-sample to avoid clicks:

```rust
let gain_step = (target_gain - current_gain) / block_size as f32;
for sample in block.iter_mut() {
    current_gain += gain_step;
    *sample *= current_gain;
}
```

For HRTF filter changes on the binaural path, crossfade between old and new convolution outputs over 128–256 samples.

### MDAP Fallback

When tracking confidence is low, widen the panning spread using MDAP (Pulkki, 1999) — activate more speaker pairs across a configurable angular spread. This trades localization sharpness for robustness to position uncertainty.

### Speaker vs Headphone Path

| Compensation           | Speaker (5.1 VBAP) | Binaural (headphones) |
|-----------------------|---------------------|----------------------|
| Listener position      | Updates VBAP gains + delays | Updates virtual source directions |
| Listener head rotation | **No** (speakers are physical, acoustic scene rotates naturally) | **Yes** (rotate source direction into head-relative frame for HRTF selection) |

---

## 7. System Architecture

### Hardware

```
 ┌─────────────────────────────────────────────────────────┐
 │                    Room (Atrium)                        │
 │                                                         │
 │   [Anchor A0]──────────────────────────[Anchor A1]     │
 │       ╲           ceiling corners           ╱          │
 │        ╲    UWB TWR ranging (DW3000)       ╱           │
 │         ╲                                 ╱            │
 │          ╲     ┌──────────────┐          ╱             │
 │           ╲    │  Listener 0  │         ╱              │
 │            ╲   │  ┌────────┐  │        ╱               │
 │             ╲  │  │UWB tag │  │       ╱                │
 │              ╲ │  │BNO085  │  │      ╱                 │
 │               ╲│  │ESP32-S3│  │     ╱                  │
 │                │  └────────┘  │    ╱                   │
 │                │  (on headband) │  ╱                    │
 │                └──────────────┘ ╱                      │
 │                                ╱                       │
 │   [Anchor A2]──────────────────────────[Anchor A3]     │
 │                                                         │
 │               [5.1 Speakers around room]                │
 └─────────────────────────────────────────────────────────┘
                           │
                     WiFi (MQTT)
                           │
                    ┌──────▼──────┐
                    │     NUC     │
                    │  Mosquitto  │
                    │  Atrium     │◄── rumqttc subscriber
                    │  Engine     │
                    │  (Rust)     │──► cpal audio output
                    └─────────────┘
```

### Data Flow

```
ESP32+DW3000 tag            ESP32+BNO085 (same board)
       │                            │
       │ DS-TWR ranging             │ SHTP I2C/SPI
       │ with 4 anchors             │ Game Rotation Vector
       │                            │
       ▼                            ▼
  Raw distances              Quaternion (w,x,y,z)
  (tag_id, anchor_id, d)
       │                            │
       └──────────┬─────────────────┘
                  │
            WiFi / MQTT QoS 0
                  │
                  ▼
         Mosquitto (on NUC, localhost)
                  │
                  ▼
         rumqttc async subscriber
                  │
                  ▼
         Position solver (trilateration + EKF)
         per listener, on NUC in Rust
                  │
                  ▼
         (x, y, z) + (qw, qx, qy, qz) per listener
                  │
                  ▼
         rtrb command queue → audio thread
                  │
                  ▼
         VBAP gain recalculation + HRTF selection
```

### Position Computation on the NUC

Run trilateration + Extended Kalman Filter in Rust (`nalgebra` for linear algebra):

1. Receive raw range measurements: `(tag_id, anchor_id, distance)`
2. Linearized least-squares trilateration:
   - Subtract last equation from all others to get `Ax = b`
   - Solve with least squares
3. Feed into per-listener EKF:
   - State: `[x, y, z, vx, vy, vz]`
   - Prediction: constant-velocity model
   - Update: range measurements
4. Output: smooth, continuous position at audio block rate

The EKF provides:
- Smoothing of noisy measurements
- Position prediction between UWB updates (important for interpolation)
- NLOS rejection via innovation gating
- Velocity estimates (useful for Doppler effects)

### Software Stack

| Component           | Technology |
|--------------------|-----------|
| Anchor firmware     | ESP-IDF or Arduino + DW3000 driver (C/C++) |
| Tag firmware        | ESP-IDF + DW3000 driver + BNO085 SHTP (C/C++) |
| Tag → NUC transport | WiFi + MQTT (esp-mqtt on ESP32) |
| MQTT broker         | Mosquitto (systemd service on NUC) |
| NUC MQTT client     | rumqttc (Rust, Tokio async) |
| Position solver     | Custom Rust (nalgebra for trilateration + EKF) |
| Audio engine        | Existing Atrium engine (cpal + rtrb) |

Alternative: write tag firmware in Rust via `esp-rs` / `esp-idf-svc` to share data types with the NUC engine.

---

## 8. Bill of Materials

### Per Listener Tracker (tag on headphones)

| Part                     | Cost    |
|--------------------------|---------|
| Makerfabs ESP32-S3+DW3000 | ~$25   |
| BNO085 breakout (Adafruit/SparkFun) | ~$22 |
| LiPo 150 mAh + charge IC | ~$4    |
| 3D-printed headphone mount | ~$1   |
| **Subtotal per listener** | **~$52** |

### Room Infrastructure (one-time)

| Part                     | Qty | Unit  | Total |
|--------------------------|-----|-------|-------|
| Makerfabs ESP32-S3+DW3000 (anchors) | 4 | ~$25 | ~$100 |
| USB power supplies       | 4   | ~$5   | ~$20  |
| Ceiling mounts (3D-printed) | 4 | ~$2  | ~$8   |
| **Subtotal infrastructure** | | | **~$128** |

### Total System Cost

| Setup                    | Cost |
|--------------------------|------|
| 4 anchors + 1 listener   | ~$180 |
| 4 anchors + 2 listeners  | ~$232 |
| 4 anchors + 3 listeners  | ~$284 |
| 4 anchors + 6 listeners  | ~$440 |

Optional add-on:
- LD2450 radar for presence detection / occupancy: +$10–15

---

## 9. Open Questions & Next Steps

### Questions to Resolve

1. **Tag form factor**: Separate unit on headband, or integrated into headphone housing? Separate is easier to prototype; integrated is cleaner.
2. **Power**: Battery (LiPo, 4–8 hr runtime) vs wired (USB to a belt pack)? Battery is cleaner; wired eliminates charging logistics.
3. **Position computation location**: On ESP32 tag (simpler MQTT payload, more tag compute) vs on NUC (tag sends raw ranges, NUC runs EKF)? NUC is more flexible and debuggable.
4. **Anchor-to-speaker coordinate alignment**: Manual measurement vs auto-calibration procedure?
5. **Fallback behavior**: What happens when tracking is lost? Freeze last known position? Fall back to DBAP? Widen MDAP spread?
6. **Multi-room**: Will there ever be multiple rooms with separate speaker arrays?

### Suggested Prototyping Order

1. **Get UWB ranging working**: Two Makerfabs boards, DS-TWR, verify ±10 cm accuracy.
2. **Add MQTT**: ESP32 publishes ranges → Mosquitto → simple Rust subscriber prints them.
3. **Trilateration**: Add 2 more anchors (4 total), implement least-squares position solver in Rust.
4. **EKF**: Add Kalman filtering for smooth position output.
5. **Integrate with audio engine**: Feed positions into VBAP gain recalculation via rtrb.
6. **Add BNO085**: Wire up IMU on one tag, publish quaternions, integrate with binaural HRTF selection.
7. **Delay compensation**: Implement per-speaker delay lines in the audio pipeline.
8. **Polish**: Calibration procedure, status UI, battery management.

### Relevant Crates

| Crate      | Use |
|-----------|-----|
| `rumqttc`  | MQTT async client |
| `nalgebra` | Linear algebra for trilateration + EKF |
| `serde` + `bincode` or `bytemuck` | Binary payload serialization |
| `rtrb`     | Already used — lock-free command queue to audio thread |

---

## 10. References

### VBAP & Spatial Audio

- Pulkki, V. (1997). "Virtual Sound Source Positioning Using Vector Base Amplitude Panning." *JAES* 45(6).
- Pulkki, V. (1999). "Uniform Spreading of Amplitude Panned Virtual Sources." *IEEE WASPAA*.
- Pulkki, V. (2001). *Spatial Sound Generation and Perception by Amplitude Panning Techniques.* Helsinki University of Technology, PhD.
- Lossius, T., Baltazar, P., & de la Hogue, T. (2009). "DBAP — Distance-Based Amplitude Panning." *ICMC*.
- Frank, M. & Zotter, F. (2012). "Exploring the Perceptual Sweet Spot in Sound Field Synthesis." *AES Convention 132*.
- Zotter, F. & Frank, M. (2012). "All-Round Ambisonic Panning and Decoding." *JAES* 60(10).

### Head Tracking & Latency

- Brungart, D.S., Simpson, B.D., & Kordik, A.J. (2004). "The Effects of System Latency on Dynamic Spatial Sound Synthesis." *AES Convention 117*.
- Wenzel, E.M., Miller, J.D., & Abel, J.S. (2000). "Effect of Increasing System Latency on Localization of Virtual Sounds." *AES Convention 108*.
- Lindau, A., Estrella, J., & Weinzierl, S. (2010). "Individualization of Dynamic Binaural Synthesis by Real-Time Manipulation of the ITD." *AES Convention 128*.

### Systems & Implementations

- SSR (SoundScape Renderer, TU Berlin) — open-source spatial audio renderer with tracking support.
- SPARTA (McCormack et al.) — VST suite with tracked Ambisonic decoding.
- IRCAM Spat / Panoramix (Carpentier, 2017) — tracked VBAP + binaural rendering.
- IEM Plug-in Suite (Graz) — Ambisonic tools with tracked decoders.
- SlimeVR — open-source BNO085 + ESP32 + WiFi body tracking (firmware reference).

### Sensor Datasheets

- Qorvo DW3000 datasheet — https://www.qorvo.com/products/p/DW3000
- CEVA BNO085/086 — via Adafruit/SparkFun product pages
- HiLink LD2450 — community documentation and protocol specs
- Qorvo application notes: APS013 (TWR), APS011 (antenna delay calibration)