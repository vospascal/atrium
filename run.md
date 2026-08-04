# How to run things

The app crate is `voxel` (it holds `main.rs`, the window and the panels).
`voxel-rt` is now a renderer *library* with no binary — that is where the examples live.

## Audio engine

cargo run --features bevy --no-default-features -- --bevy scenes/wind-only.yaml

## Voxel engine

cargo run -p voxel --release

cargo run -p voxel --release -- --studio
cargo run -p voxel --release -- --studio --project studio-project

cargo run -p voxel --release -- --mode world   --project studio-project
cargo run -p voxel --release -- --mode studio  --project studio-project

`--mode world|studio` is the canonical switch; `--studio` is the shorthand.
Studio mode boots the isolated material-preview scene instead of the generated world.
Open the node editor with the `▸ Graph Studio` button in the bottom strip.
`O` opens the settings window, `P` the performance panel.

Headless project validation (exits non-zero on diagnostics):

cargo run -p voxel --release -- --validate-project --project studio-project

## Examples — these stay on `voxel-rt`

cargo run -p voxel-rt --release --example cache_report
cargo run -p voxel-rt --release --example probe_project
cargo run -p voxel-rt --release --example sync_project

cargo run -p voxel-rt --release --example bench_dda          # every section
cargo run -p voxel-rt --release --example bench_dda -- 4 5   # sections 4 and 5 only
cargo run -p voxel-rt --release --example bench_dda -- --no-collapse

PNGs land in `target/bench_dda/`.

## Checks

cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets
python3 scripts/dep-cycles.py                 # module cycles in voxel-rt
python3 scripts/dep-cycles.py crates/voxel-graph/src

---

## Sample output — cache_report

cacheability of 29 graphs

material-01.vgraph.json      2 layer(s), 0 clock/event source(s)
    cacheable
    cacheable
material-03.vgraph.json      2 layer(s), 0 clock/event source(s)
    cacheable
    cacheable
material-04.vgraph.json      1 layer(s), 0 clock/event source(s)
    cacheable
material-06.vgraph.json      2 layer(s), 1 clock/event source(s)
    cacheable
    cacheable (gain)
material-26.vgraph.json      1 layer(s), 1 clock/event source(s)
    cacheable (gain + drift)
material-27.vgraph.json      4 layer(s), 0 clock/event source(s)
    cacheable
    cacheable
    cacheable
    cacheable

6 graph(s) author pattern layers, 12 layer(s) total, 0 not cacheable
