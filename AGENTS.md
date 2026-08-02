# Project instructions

## WebGPU and WGSL references

This repository vendors the `webgpu-specs` reference skill at
`.claude/skills/webgpu-specs/` (from `gfx-rs/wgpu@7a65558`). For any task that
implements, reviews, debugs, or explains WebGPU or WGSL behavior, read and
follow `.claude/skills/webgpu-specs/SKILL.md`.

When a specification check is needed, run:

```sh
sh .claude/skills/webgpu-specs/download.sh
```

Search the downloaded WebGPU and WGSL sources under `target/claude/`, and cite
the relevant named GPUWeb specification anchor in user-facing explanations.
