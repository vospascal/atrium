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

## Crate architecture: narrow seams, enforced by visibility

A crate boundary exists for one reason: to bound the blast radius of a change. Everything
below serves that, and nothing below is worth doing for its own sake.

**A boundary is only as real as what consumers route through it.** That is measurable, so
measure it — do not argue it. After any extraction, grep for the crate's implementation
types outside the crate:

```sh
rg -n "ImplType|OtherImplType" --glob '!crates/<crate>/**' --glob '!target/**' .
```

Any hit means the boundary is decorative and the work is not done. Two worked examples in
this repo, both instructive:

- `voxel-environment` passes. Consumers name `EnvironmentRequest` and `EnvironmentGpu`;
  changing the LUT set is a one-file edit. Before the seam existed, `voxel-rt` named
  `AtmosphereBindings`, `AtmosphereUniform` and `LutConfig` across four files — the facade
  was present and routed around.
- `voxel-color` fails. `ColorAdapter` has five implementations and zero consumers outside
  the crate; `voxel-rt` calls `OutputFormat::resolve` directly. Three of the five impls are
  18-line presets delegating to a fourth. The crate is good; that layer of it is not.

This is an Atrium project convention layered on top of—not a replacement for—the
[Rust API Guidelines](https://rust-lang.github.io/api-guidelines/). Every crate must still
follow those guidelines for visibility, private fields and invariants, common trait
implementations, standard conversion traits, object-safe traits where trait objects are
intended, input validation, documentation, and future-proof public APIs.

The default layout:

```text
crates/<crate>/
  src/
    lib.rs          # stable public facade and reexports only
    api.rs          # stable contracts, requests, outcomes — no backend types
    state.rs        # runtime state and configuration
    gpu.rs          # GPU/backend binding and resource conversion
    <domain>/       # one implementation file per mapping/provider/algorithm — PRIVATE
    adapters/       # only once a SECOND implementation exists; see below
  shaders/<domain>/ # one WGSL file per implementation plus common/dispatch files
```

Rules:

- **Visibility does the enforcing; traits only describe.** Making a module private is what
  makes reaching into it impossible. A trait with one implementation describes a contract
  nobody can violate anyway. Prefer `mod foo;` with one `pub` type over
  `pub mod foo;` plus a single-impl trait — and reach for the trait when a second
  implementation or a `&dyn` consumer actually arrives.
- **`adapters/` is pulled into existence by a second implementation, never prescribed.**
  Prescribing it up front is what produced a 5-line `gpu.rs` doc-comment placeholder and a
  fabricated `AnalyticProvider` whose `shader_source` returned a module reading four
  textures it would never populate — selecting it would have bound nothing, and nothing
  caught that because nothing constructed it. One honest implementation in a private
  `<domain>/` module beats two where one is invented to fill a directory.
- **No speculative surface.** A `pub fn` with no caller is the same failure as a placeholder
  file. If keeping one alive requires inventing a consumer, delete it instead.
- **Keep the decision pure.** Separate *what should happen* from *doing it*, so the policy
  is testable without a device. `EnvironmentRequest::invalidation_since` is the model: it
  turned "what does a head turn cost" from a frame capture into five unit tests.
- `lib.rs` exposes stable contracts and selected implementations; details stay behind module
  boundaries. Compatibility reexports are allowed during migration, but they do not replace
  the facade — and per the project's standing rule, they are removed in the same change,
  not left as a forwarding layer.
- Public consumers depend on resolved contracts, not on backend, platform, output-depth, or
  algorithm `match` statements scattered through the renderer.
- Each independently selectable mapping, provider, curve, node, or conversion gets its own
  source file. A large `mod.rs` may declare and reexport modules, but must not become a
  catch-all implementation file. Where Rust needs one exhaustive `match`, put it in a thin
  `dispatch.rs` that delegates to the per-implementation files — the compiler still refuses
  an unhandled variant, so completeness stays machine-checked while bodies stay separated.
- WGSL follows the same split: shared helpers, one file per implementation, and a dispatch
  file. If an aggregate shader is needed, generate it from those fragments — `concat!` of
  `include_str!` keeps it a `&'static str` with no second source of truth and no build
  script. Never `include_str!` across a crate boundary by relative path; that bypasses the
  facade and breaks the moment the other crate reorganises its shaders.
- **The crate README is a design record, not a summary.** State what is deliberate and
  counter-intuitive so a future reader does not "fix" it — `voxel-color`'s "decisions that
  look wrong and are right" is the model, and each entry should say what it cost to learn.
  Record known gaps honestly, including what blocks them.
- A plan is not completion evidence. Before reporting completion, verify the promised tree
  with `find`/`rg --files`, run the relevant crate tests, and run dependent crate checks. If
  a promised module is still a placeholder or missing, report the work as partial and
  continue or say exactly what remains.
- Small/trivial crates may opt out of the layout with a short justification in the crate
  README; the exception must not be used to keep a growing implementation monolithic.

### Module cycles

Rust forbids crate cycles, so a dependency cycle inside a crate is a boundary you cannot
draw later. `scripts/dep-cycles.py` reports them. Two things it taught us worth
remembering: strip comments and `#[cfg(test)]` before believing a dependency graph — doc
links and test-only imports inflated `voxel-rt` from one cycle to an apparent 32-module
tangle — and a cycle is usually one type living in the wrong module, not a design problem.
