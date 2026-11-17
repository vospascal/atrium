# How to run things

## Audio engine

```sh
cargo run --release                                  # scenes/default.yaml
cargo run --release -- scenes/river-only.yaml        # any scene file
cargo run --release -- --ui scenes/nature.yaml       # with the terminal dashboard
```

Telemetry and control are served over WebSocket on port 3333 (no client ships right now).

## Checks

```sh
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets
python3 scripts/dep-cycles.py                 # module cycles in src/
```
