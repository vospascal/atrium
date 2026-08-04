# Voxel Automata Terrain — diamond-square with cellular-automata rules

An evaluation of BWerness's
[voxel-automata-terrain](https://bitbucket.org/BWerness/voxel-automata-terrain/src/master/ThreeState3dBitbucket.pde),
brought to our attention by x1m4 (2025-07-16) as the generator he pairs with his
*"voxel DDA with a non-sparse octree encoded into the mip levels"* traversal.

**Status: EVALUATED, nothing committed.** This is the doc to start from when the
topic comes up, not an approved arc. Written from the actual `.pde` source, not from
what diamond-square usually is.

## What the algorithm actually does

Diamond-square's *structure* with cellular-automata *rules* substituted for the
averaging step.

- **Three states** per cell: `0` empty, `1` and `2` two filled variants.
- **A `(2^L)+1` cubed lattice** — `K = (1<<L)+1`, so 65³ at `L = 6`. The `+1` is
  classic diamond-square: cells sit on a corner lattice, not in cell centres.
- **One top-down loop**, `for (int w = K-1; w >= 2; w /= 2)`, halving the step each
  level — an octree walked from the root down.
- **Three phases per level**, the 3D generalisation of diamond-square's two:

```
  w                    phase 1: CUBE            phase 2: FACE         phase 3: EDGE
  o---------o          o---------o              o----o----o           o----o----o
  |         |          |         |              |         |           |    o    |
  |         |          |    *    |              o    *    o           o----o----o
  |         |          |         |              |         |           |    o    |
  o---------o          o---------o              o----o----o           o----o----o
  8 corners known      centre from the          face centres          edge centres
                       8 corners                from 6 neighbours     from 6 neighbours
                       cubeRule[n1][n2]         faceRule[n1][n2]      edgeRule[n1][n2]
```

- **Rules are lookup tables indexed by NEIGHBOUR COUNTS**, which is the whole trick:
  `cubeRule[9][9]` — how many neighbours hold state 1 (0..8) crossed with how many
  hold state 2 — plus `faceRule[7][7]` and `edgeRule[7][7]` for the six-neighbour
  cases. ~179 entries of `{0,1,2}` in total.
- **Deterministic by default.** `flipP = 0.0`; the optional stochastic flip is off,
  so a world is a pure function of the rule tables plus the corner seeds.
- **Rules serialise to base-62 strings** like `"jxDFmQJZgXwLxmg83f9dQ5DSst"`. That is
  the entire world, ~26 characters.
- **Two generators for the tables**: `randomRule(lambda)` for a target fill density,
  and `randomIsingRule(beta, mag)` — Boltzmann-weighted sampling with inverse
  temperature and magnetisation.

## The insight worth taking, whatever we decide about the look

**The generation hierarchy IS the traversal acceleration hierarchy.**

x1m4 traverses a non-sparse octree encoded in mip levels and generates with this.
Those are the same octree. Level `w` of the subdivision is mip level `log2(w)` of the
acceleration structure.

So you never *build* the pyramid — you fall out of it. Two consequences:

- Coarse occupancy exists **before** the fine detail does, which is exactly the order
  streaming wants: you can know a region is empty without having generated its
  contents.
- The generator and the traversal cannot disagree, because they are one structure
  rather than two that must be kept in sync.

That argument is independent of whether we like what this particular rule family
produces, and it is the strongest reason to read the technique.

## Why it fits this engine specifically

- **Embarrassingly parallel per level.** Every cube centre in a level depends only on
  the previous level, so each of the three phases is a parallel map with no
  cross-dependency. That is a direct answer to **ledger 4.14** (the 827 ms
  single-threaded attribute build) and it is a compute dispatch per phase per level,
  not a CPU loop.
- **A biome is a string.** `randomIsingRule(beta, mag)` gives two continuous dials —
  coherence and fill — instead of 179 hand-authored table entries. Turn the knobs,
  keep the base-62 string, that is the biome. Cheap to store, cheap to blend between,
  trivially shareable.
- **Nothing to persist.** A world is its seed and its rule string. For a streamed
  world, regeneration beats storage by a wide margin.
- **Ledger 4.5** already records the position this sits in: SDF/noise math is welcome
  as a *generator*; we stay voxel-authoritative. This is a generator.
- **Our brickmap is already dense two-level.** The subdivision's last levels map onto
  brick and sub-voxel naturally.

## The catches, stated honestly

**It is globally top-down, which is the opposite of what an infinite world wants.**
You cannot generate a distant chunk without the whole hierarchy above it. Partly
mitigable — level *k* holds only `(2^k+1)³` cells, so the top levels are tiny and
cacheable, and you subdivide only the branches you need. But tiling independent
blocks reintroduces diamond-square's classic **seam problem**, and that needs
solving rather than hand-waving. This is the single biggest open question.

**Three states is structure, not material.** Our table has 26 rows. States would map
to materials, or serve as a skeleton with materials assigned afterwards by
height/biome/exposure. Either way it is a second system, not free.

**It produces alien fractal architecture, not geology.** No rivers, no strata, no
semantic control, no "put a village here". The current world is an authored
sky-plateau with a biome gradient and a river. Adopting this trades *controllable*
for *striking*. That is a look decision and a big one — worth prototyping before
anyone argues about it.

**It is not shared code with CAGI**, despite both being cellular automata. CAGI
iterates a *fixed* grid to convergence; this *subdivides* a growing one. Same mental
model, different machinery, no reuse.

## A cheap evaluation path

Ordered so the look question — the one that decides everything — gets answered first
and cheapest.

### S0 — port the reference, headless, and look at it

Port `ThreeState3dBitbucket.pde` to a Rust example that generates at `L = 6..8` and
dumps to `.vox` or straight into the existing brickmap viewer. No engine integration,
no materials, one state → one colour.

**Gate:** do we actually want this look? Generate a dozen Ising rule strings across
the `(beta, mag)` space and decide. If the answer is no, stop here — total cost is a
few hundred lines.

### S1 — states to materials

Map `{1, 2}` onto real rows and feed the brickmap through the existing
`VoxelSource` seam. Still one 65³–513³ block, no streaming.

**Gate:** it renders, traverses and lights through the shipped path with no special
casing.

### S2 — parallelise the levels

Three parallel maps per level on the CPU. Measure against the 827 ms baseline that
ledger 4.14 already records.

**Gate:** a real number for the speedup, recorded. This is worth doing even if the
look is rejected, because the *shape* — parallel-per-level — applies to any
hierarchical generator we write.

### S3 — generate the acceleration structure as a by-product

The point of the whole exercise. Emit the brick occupancy bitgrid and the coarse
levels **during** generation instead of deriving them afterwards.

**Gate:** identical brickmap to the derive-afterwards path, at lower total cost.

### S4 — tiling and streaming

Only if S0–S3 pass. The seam problem, the coarse-level cache, and the interaction
with `docs/streaming-plan.md`.

## Open questions

- **Seams.** Can the rule tables be made to tile, or does it need a shared coarse
  level across blocks — and if the latter, how far up does the shared prefix have to
  go before seams are invisible?
- **Controllability.** Is there a way to constrain the result (a river, a floor at
  y = 0, a cave entrance here) without destroying the fractal character? Seeding the
  coarse levels by hand is the obvious lever and is unexplored.
- **Does the Ising parameterisation actually span an interesting space**, or do most
  `(beta, mag)` pairs produce mush? S0 answers this by inspection.
- **Which levels are worth keeping.** The subdivision produces every level; the
  traversal wants some of them as a mip pyramid. Which, and at what cost, is the
  bridge between this doc and `docs/cagi-cascades-plan.md`.

## Related

- `docs/cagi-cascades-plan.md` — the same "use the hierarchy you already have"
  argument, applied to the light volume rather than to geometry.
- `docs/xima-engine-dossier.md` — the other engine we track, and the source of the
  directional-CA and multi-resolution notes.
- `docs/streaming-plan.md` — the `VoxelSource` seam any generator plugs into.
- Ledger **4.5** (noise/SDF as a generator, not a renderer) and **4.14** (the
  single-threaded attribute build this technique's shape would fix).
- Diamond-square background:
  <https://www.youtube.com/watch?v=4GuAV1PnurU>
