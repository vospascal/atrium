use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let shader_dir = manifest_dir.join("shaders").join("tonemap");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("out dir")).join("tonemap.wgsl");
    let fragments = [
        "common.wgsl",
        "reinhard.wgsl",
        "hdr_knee.wgsl",
        "hable.wgsl",
        "bt2390.wgsl",
        "gt7.wgsl",
        "dispatch.wgsl",
    ];

    let mut source = String::new();
    for fragment in fragments {
        let path = shader_dir.join(fragment);
        println!("cargo:rerun-if-changed={}", path.display());
        source.push_str(&fs::read_to_string(path).expect("read tonemap fragment"));
        source.push_str("\n\n");
    }
    fs::write(output, source).expect("write assembled tonemap shader");
}
