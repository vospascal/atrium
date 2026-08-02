cargo run --features bevy --no-default-features -- --bevy scenes/wind-only.yaml

cargo run -p voxel-rt --release -- --studio

cargo run -p voxel-rt --release -- --studio --project studio-project


cargo run -p voxel-rt --release -- --mode world --project studio-project
cargo run -p voxel-rt --release -- --mode studio --project studio-project









cargo run --release -p voxel-rt --example cache_report
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
