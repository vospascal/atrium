cargo run --features bevy --no-default-features -- --bevy scenes/wind-only.yaml

cargo run -p voxel-rt --release -- --studio

cargo run -p voxel-rt --release -- --studio --project studio-project


cargo run -p voxel-rt --release -- --mode world --project studio-project
cargo run -p voxel-rt --release -- --mode studio --project studio-project