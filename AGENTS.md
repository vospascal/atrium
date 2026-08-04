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

## Crate architecture: facade and adapters are required

New crate work and meaningful refactors MUST use an adapter-first design. Do not leave
the architecture as a plan or a docs-only seam: the promised files and modules must exist
in the working tree before the work is described as complete.

This is an Atrium project convention layered on top of—not a replacement for—the
[Rust API Guidelines](https://rust-lang.github.io/api-guidelines/). Every crate must still
follow those guidelines for visibility, private fields and invariants, common trait
implementations, standard conversion traits, object-safe traits where trait objects are
intended, input validation, documentation, and future-proof public APIs.

The default layout is:

```text
crates/<crate>/
  src/
    lib.rs          # stable public facade and reexports only
    api.rs          # stable contracts, requests, outcomes, adapter traits
    state.rs        # runtime state and configuration
    gpu.rs          # GPU/backend binding and resource conversion
    adapters/       # concrete policies/backends
    <domain>/       # one implementation file per mapping/provider/algorithm
  shaders/<domain>/ # one WGSL file per implementation plus common/dispatch files
```

Rules:

- `lib.rs` exposes stable contracts and selected adapters; implementation details stay
  behind module boundaries. Compatibility reexports are allowed during migration, but
  they do not replace the facade.
- Public consumers depend on adapter traits and resolved contracts, not on backend,
  platform, output-depth, or algorithm `match` statements scattered through the
  renderer.
- Each independently selectable mapping, provider, curve, or conversion gets its own
  source file. A large `mod.rs` may declare and reexport modules, but must not become a
  catch-all implementation file.
- WGSL follows the same split: shared helpers, one file per implementation, and a
  dispatch file. If an aggregate shader is needed for compatibility, it must be
  generated from those fragments rather than maintained as a second source of truth.
- A plan is not completion evidence. Before reporting completion, verify the promised
  tree with `find`/`rg --files`, run the relevant crate tests, and run dependent crate
  checks. If a promised module is still a placeholder or missing, report the work as
  partial and continue or say exactly what remains.
- Small/trivial crates may opt out only with a short justification in the crate README;
  the exception must not be used to keep a growing implementation monolithic.
