# How to run things

Everything runs from the repository root. Stop any running engine with **Ctrl+C**.

## Listen on speakers

```sh
cargo run --release                                   # scenes/default.yaml
cargo run --release -- scenes/nature.yaml             # any scene file
cargo run --release -- scenes/wisp.yaml               # one wisp flying through the room
cargo run --release -- --ui scenes/nature.yaml        # with the terminal dashboard
```

## Listen on headphones

Headphone scenes use `layout: stereo` + `render_mode: hrtf`.

```sh
cargo run --release -- scenes/nature-headphones.yaml  # the meadow, binaural
cargo run --release -- scenes/frog-headphones.yaml    # one frog, front-right
cargo run --release -- scenes/wisp-headphones.yaml    # one wisp flying around your head
```

## Control it from a browser or a phone

The engine listens for control on port 3333 whenever it runs. The page in
`web/` talks to it: switch render mode and speaker set, master gain, mute
sounds, see levels and where everything is.

```sh
cargo run --release -- scenes/nature.yaml   # terminal 1: the engine
npm --prefix web install                    # once
npm --prefix web run dev                    # terminal 2: the page, http://localhost:6006
```

On a phone on the same Wi-Fi, open `http://<this-machine>:6006`; the dev
server prints the address under "Network". The page finds the engine on the
same host. To point it elsewhere: `http://localhost:6006/?engine=nuc.local:3333`.

## Head tracking

Your head turn steers the listener, so the world stays put when you turn.

```sh
# Sony WH/WF-1000XM5/XM6 — one command, the engine starts the Sony bridge too
cargo run --release -- --head-tracking=sony scenes/frog-headphones.yaml
cargo run --release -- --head-tracking=sony scenes/wisp-headphones.yaml

# Any other tracker that sends OpenTrack UDP to 127.0.0.1:4242 — start it yourself
cargo run --release -- --head-tracking scenes/frog-headphones.yaml
```

Look straight ahead when it starts: that is "centre". Test: in the frog scene,
turn your head right and the frog moves to straight ahead.
The wisp scene is the harder test: it flies around and overhead, so turn your
head to follow it and it should stay where it is in the world.

### Sony setup (once, and after firmware or setting changes)

1. Sony Sound Connect app: **Bluetooth Connection Quality → Prioritize Stable
   Connection**. "Prioritize Sound Quality" (LDAC) switches the sensor off.
2. Mac: **forget the headphones and pair again** after changing that setting or
   updating firmware, otherwise macOS does not see the sensor.
3. First run builds the bridge into `target/sony-head-tracker/` (needs Xcode).
   Then enable **SonyBridge** under System Settings → Privacy & Security →
   Input Monitoring.

```sh
sh scripts/sony-head-tracker.sh probe     # is the sensor visible? expect "verified Android tracker=yes"
sh scripts/sony-head-tracker.sh rebuild   # fresh build of the bridge
sh scripts/sony-head-tracker.sh           # bridge on its own (rarely needed)
```

The bridge log is `$TMPDIR/atrium-sony-bridge.log`.

## Analysis tools

```sh
cargo run --release --bin analyze_sample -- "<recording>" [segment_start_s segment_end_s]...
cargo run --release --bin analyze_rain -- "research papers/soft_rain.mp3"
cargo run --release --bin analyze_synth
cargo run --release --bin analyze_bubbles
cargo run --release --bin spectrogram -- "<recording>" <out_dir> [wind|rain]
```

## Profiling

```sh
cargo run --release --features profiler -- scenes/default.yaml --profile fmt        # span timing in the terminal
cargo run --release --features profiler -- scenes/default.yaml --profile perfetto   # → trace.pftrace
cargo run --release --features profiler -- scenes/default.yaml --profile flame      # → tracing.folded
cargo run --release --features memprof -- scenes/default.yaml                       # allocation tracking
```

## Checks

```sh
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets
python3 scripts/dep-cycles.py             # module cycles in src/
```

## Ports

| Port | What |
|---|---|
| UDP 4242 (127.0.0.1) | head tracking in (OpenTrack) |
| UDP 4243 (127.0.0.1) | Sony bridge JSON debug stream |
| TCP 3333 | WebSocket telemetry + control (the page in `web/` is its client) |
| TCP 6006 | The control page's dev server (`npm --prefix web run dev`) |
