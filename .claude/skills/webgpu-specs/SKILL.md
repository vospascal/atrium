---
name: webgpu-specs
description: Download WebGPU and WGSL specifications for use as a reference
allowed-tools: "Bash(sh .claude/skills/webgpu-specs/download.sh)"
---

Vendored from the wgpu repository
(`gfx-rs/wgpu@7a65558:.claude/skills/webgpu-specs`). Re-sync from upstream
rather than editing in place.

Run `sh .claude/skills/webgpu-specs/download.sh` to download the
WebGPU and WGSL specifications if they are not present or if they have
been updated. You do not need to change directory before running the script.

After the specs are downloaded, you can search in `target/claude/webgpu-spec.bs`
and `target/claude/wgsl-spec.bs` for relevant sections of the specification.

When referencing the specifications, prefer to use named anchors rather than
line numbers. For example, to reference the "Object Descriptors" section, which has the
following header:

```
### Object Descriptors ### {#object-descriptors}
```

Use the URL <https://gpuweb.github.io/gpuweb/#object-descriptors> so the user
can click to navigate directly to that section.

For the WGSL specification, the base URL is <https://gpuweb.github.io/gpuweb/wgsl/>.

If necessary, read additional content from the file to find the header preceding
the text you want to reference. You may provide line numbers as additional
context, but always make every effort to provide the user with a clickable link.

## Why this repo has it

`voxel-rt` is a hand-written WGSL compute renderer with generated shader code,
uniform/storage buffers whose Rust layouts are pinned by size tests, and a lever
system that string-patches shader constants. Every one of those is a place where
"what the spec actually says" beats recall — the std140 array-stride rule has
already cost this project two layout bugs.
