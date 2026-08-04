# A CAGI-style light propagation kernel, annotated

**Source:** `temp/general-programming/0104-update.rs` — posted by **TooManyLimits, 2024-06-04 21:33**
in the Graphics Programming server: *"Here's the update function (rust) if anyone wants to see our
algorithm."* The cell type came eight hours earlier (13:49), replying to x1m4's integer-rotation gist
with *"yep, that's how i implemented my version of the light CA."*

**Not x1m4's code.** It's an independent implementation by another server member, in the same idiom.
That makes it the only readable, complete propagation kernel in the corpus — x1m4 never posted his.
Cross-referenced against his prose rule at the end.

**Scope:** 2D, 4 directions. It is the *core kernel only* — no skylight injection, no per-face
opacity, no transmittance, no dirty-chunk logic, no cascades.

---

## The cell

```rust
#[derive(Clone, Copy, Debug)]
pub enum RgbDirectionalCell3 {
    Air([[u8; 4]; 3]),
    Block(Vec3),
    Light([[u8; 4]; 3])
}
```

Indexing throughout is `[c][dir]`, so the layout is **3 channels × 4 directions × u8 = 12 bytes per
air cell**. `Vec3` for blocks is the reflectance colour. ○ The `3` in the name reads as a version
number, not a dimension.

Three consequences worth noting up front:

- **`Block` and `Light` return themselves unchanged.** So `Light` is a *constant* source with a fixed
  per-direction profile — an omnidirectional lamp is all four directions set equal — and blocks are
  fully opaque. There is no partial transmittance, which is exactly the gap x1m4 fills with his
  precomputed per-face opacity maps.
- **Only `Air` propagates.** The simulation lives entirely in empty space, which is why x1m4 says his
  version *"doesn't care about solids anymore and only deals with opacity and light transfer."*
- **`u8` per component** caps dynamic range at 255. x1m4 uses 10 bits per direction per channel for
  exactly this reason, and still calls 10-bit his memory ceiling.

---

## The kernel, step by step

```rust
fn update(&self, neighborhood: MooreNeighborhood<'_, Self>) -> Self {
  let input = match self {
    Self::Block(_) | Self::Light(_) => return *self,
    &Self::Air(data) => data
  };
  let mut output = input.clone();
```

`input` is the pre-tick snapshot; `output` is what gets written. Keeping both matters in step 3.

### 1. Decay

```rust
for dir in 0..4 { for c in 0..3 { output[c][dir] = output[c][dir].saturating_sub(DECAY); } }
```

Unconditional loss per tick, before any gathering. **`saturating_sub` is the whole trick**: it is
x1m4's `max(LOSS, LIGHT) - LOSS` written idiomatically, and it's why the field is monotone-bounded and
therefore *settles*. Multiplicative decay (`light * 0.98`) never quite reaches zero in fixed point and
leaves cells forever dirty; subtractive decay hits zero exactly and lets dirty-chunk culling work.

### 2. Direct neighbours — advection plus lateral diffusion

```rust
match neighborhood.direct_neighbors[(dir+2)%4] {
  Self::Air(light) | Self::Light(light) => {
    for c in 0..3 { output[c][dir] = output[c][dir].max(light[c][dir].saturating_sub(DIRECT_ATTENUATION)); }
    for side_dir in [(dir+1)%4, (dir+3)%4] {
      for c in 0..3 { output[c][side_dir] = output[c][side_dir].max(light[c][side_dir].saturating_sub(SIDE_LIGHT_ATTENUATION)); }
    }
  }
```

`(dir+2)%4` is the **opposite** direction, so computing your `dir` component reads the neighbour
*behind* the flow. If `dir` is up, you read the cell below. That's correct advection, and it matches
the comment: *"Collect 'up-ness' from neighbors below."*

The second inner loop is the part that repays attention. From that same behind-neighbour it also pulls
the two **perpendicular** components, at a separate `SIDE_LIGHT_ATTENUATION`. So upward-moving light
drags its sideways components upward with it. This is x1m4's *"add slight diffusion for each axis"* —
implemented as max-with-attenuation rather than his `mix()`, but doing the same job: keep beams from
staying perfectly collimated, which is what produces soft gradients instead of hard cones.

Two non-obvious properties:

- **Each component gathers from three of the four neighbours.** Over the four `dir` iterations,
  `output[c][s]` gets `max`'d against the behind-neighbour for `dir == s` (main term) and against two
  lateral neighbours (side term). Wider stencil than it looks.
- **The opposite component is never copied** — *"but not their 'down-ness'."* This is the stability
  guarantee. Back-flow between adjacent cells is what creates the push-pull oscillation x1m4 warned
  about: *"two neighbor cells keeping pushing and pulling against each other resulting in a cycle."*
  Forbidding it is what makes a single-buffered pass safe here.

### 3. Reflection off solids

```rust
  Self::Block(color) => {
    for c in 0..3 {
      output[c][dir] = output[c][dir].max(((input[c][(dir+2)%4] as f32 * color[c]) as u8).saturating_sub(1));
    }
  }
```

Take **your own** light heading *into* the block — `input[c][(dir+2)%4]`, the opposite direction —
tint it by the block colour, lose 1, emit it back out along `dir`. A mirror flip with colour bleed.
This is x1m4's `lightpy * neighbor_solid_color * bounce_loss`.

Note `input`, not `output`: the pre-decay snapshot. Using `output` here would double-count the decay
and make reflections darker than intended.

○ **This is the one place floats appear**, and the `as u8` is a truncating cast — so each bounce loses
up to an extra ~0.5 LSB on top of the explicit `-1`. For a deterministic engine it would need to be a
fixed-point multiply, e.g. `((input as u16 * color_u8 as u16) >> 8) as u8`. Worth flagging because
x1m4's entire determinism argument rests on there being no float ops in the propagation.

### 4. Diagonals, with three-tier corner occlusion

```rust
for (check_dir, diagonal_neighbor_dir) in [((dir + 3) % 4, (dir + 1) % 4), ((dir + 1) % 4, (dir + 2) % 4)] {
  match neighborhood.diagonal_neighbors[diagonal_neighbor_dir] {
    Self::Air(light) | Self::Light(light) => {
      for c in 0..3 {
        if light[c][check_dir] == 0 { continue; }
        match (neighborhood.direct_neighbors[diagonal_neighbor_dir],
               neighborhood.direct_neighbors[(diagonal_neighbor_dir + 1) % 4]) {
          (Self::Block(_), Self::Block(_)) => {}
          (Self::Block(_), _) | (_, Self::Block(_)) =>
            output[c][dir] = output[c][dir].max(light[c][dir].saturating_sub(DIAGONAL_PARTIAL_OCCLUDED_ATTENUATION)),
          (_, _) =>
            output[c][dir] = output[c][dir].max(light[c][dir].saturating_sub(DIAGONAL_ATTENUATION)),
        }
      }
    }
    Self::Block(_) => {}
  }
}
```

**This is the most valuable part of the file, because x1m4 never describes it.**

The gate — `if light[c][check_dir] == 0 { continue; }` — accepts a diagonal contribution only if the
diagonal neighbour has light flowing along the *other* axis, i.e. actually heading your way. A cheap
directional plausibility test.

Then the two **direct** neighbours that bracket that diagonal are inspected, and the attenuation is
chosen by how sealed the corner is:

| Bracketing neighbours | Attenuation | Meaning |
|---|---|---|
| both `Block` | *nothing* | corner sealed — light cannot cut the diagonal |
| exactly one `Block` | `DIAGONAL_PARTIAL_OCCLUDED_ATTENUATION` | grazing a wall edge |
| neither | `DIAGONAL_ATTENUATION` | open diagonal |

The `both Block → nothing` case is the fix for the classic Minecraft flood-fill leak, where light
seeps through the diagonal join between two walls. Getting this wrong is what produces the square
halos x1m4 hit and fixed in 2024-03 (*"I just didn't output the emission diagonally, but only face
wise"*).

So the kernel has **four** independent tuning constants — `DIRECT_ATTENUATION`,
`SIDE_LIGHT_ATTENUATION`, `DIAGONAL_ATTENUATION`, `DIAGONAL_PARTIAL_OCCLUDED_ATTENUATION` — plus
`DECAY`. Neither x1m4's prose nor any doc mentions that this is the tuning surface. It is the thing
you would actually spend your time on.

### One defect

**The function never returns.** The body ends after the loops with no `Self::Air(output)`. Almost
certainly trimmed when pasted, given the `-> Self` signature, but worth knowing if you copy it.

---

## Why the index arithmetic looks like that

The `(dir+2)%4` / `(dir+1)%4` / `(dir+3)%4` pattern isn't arbitrary — it's x1m4's direction-index
convention, which he posted **43 minutes before** TooManyLimits replied *"yep, that's how i implemented
my version of the light CA"* ([gist 29ca6fa3…](https://gist.github.com/maierfelix/29ca6fa3a2ae5f0e26404b8cab3a83d3),
2024-06-04 13:06):

```js
const DIRECTIONS_90 = [[0, 1], [1, 0], [0, -1], [-1, 0]];   // 0=+Y 1=+X 2=-Y 3=-X
function RotateDirectionCW90(dir)  { return (dir + 1) & 0b11; }
function RotateDirectionCCW90(dir) { return (dir + 3) & 0b11; }

const DIRECTIONS_45 = [[0,1],[1,1],[1,0],[1,-1],[0,-1],[-1,-1],[-1,0],[-1,1]];
function RotateDirectionCW45(dir)  { return (dir + 1) & 0b111; }
function RotateDirectionCCW45(dir) { return (dir + 7) & 0b111; }
```

So in the kernel:

- `(dir+2)%4` = two CW rotations = **180°**, the opposite direction. That's why it reads the neighbour
  *behind* the flow.
- `(dir+1)%4` and `(dir+3)%4` are exactly `RotateDirectionCW90` / `RotateDirectionCCW90` — the two
  perpendiculars. That's the `side_dir` pair.

The actual trick in the gist is that **a power-of-two direction count lets you rotate by masked
addition** — `& 0b11` instead of `%`, no modulo, no branch, no table lookup. That's what he means by
*"simple but handy integer rotation useful for cellular automata related things."*

○ And the 45° table is the interesting one for anyone extending this: it **interleaves cardinals and
diagonals**, so even indices are cardinal and odd are diagonal, and `dir & 1` tests which. That would
let you collapse the kernel's separate direct-neighbour and diagonal-neighbour loops into one pass over
8 directions — at the cost of storing 8 components per channel instead of 4.

Unrelated but worth knowing, since it's the same idiom one dimension up: his **3D** rotation
implementation is [gist 2807ad81…](https://gist.github.com/maierfelix/2807ad81904748e87d3aa806b094d782)
— a `uvec3[24]` swizzle table plus a `bvec3[24]` flip table, applied as flip-then-swizzle
(`1.0 - pos.c`, then `pos[rs.x], pos[rs.y], pos[rs.z]`). All 24 orientations, 5 bits of storage, no
matrix multiply.

---

## Properties that fall out

- **Everything is `max`, never `+`.** Overlapping lights take the brighter, they don't sum. That is
  precisely the limitation x1m4 describes — two equal lights either side of a wall wash each other's
  shadows out — and it is inherent to max-propagation, not a bug. The alternative is additive with
  clamping, which is how x1m4 got his *"gateway to heaven"* runaway-brightness bug in 2023.
- **It converges.** Every path loses at least `DECAY` per tick and back-flow is forbidden, so the field
  is monotone-bounded and reaches a fixed point. This is the property that makes dirty-chunk culling
  viable, and it is what x1m4 means by *"it stabilizes in a finite time"* and *"once an area didn't
  change compared to the previous frame, it can be completely culled from further updates."*
- **Cost:** 8 neighbour cells read (4 direct + 4 diagonal) × 12 bytes, 12 bytes written. x1m4's figure
  — *"in 2d it should be between 10 and 20 reads per cell (if bouncing etc. is enabled)"* — matches
  once you count component accesses rather than cells.
- **Near-integer-exact.** `saturating_sub` and `max` on `u8` are exact; only the reflection multiply
  isn't. Fix that and the whole kernel is deterministic across vendors, which is the entire point of
  x1m4's design.

---

## Mapping to x1m4's own rule

His prose version (2025-12-24, #graphics-programming):

> ▸ *"propagate each axis by looping through each adjacent neighbor and do
> `max(srclightpy, neighborlightpy) * propagation_loss`, add slight diffusion for each axis e.g.
> `mix(lightpy, lightny, x)`, bounce light from surfaces with color for each axis by doing e.g.
> `lightpy * neighbor_solid_color * bounce_loss`"*

| x1m4 | This kernel |
|---|---|
| `max(src, neighbor)` | `output[c][dir].max(light[c][dir]…)` — identical |
| `* propagation_loss` | `.saturating_sub(DIRECT_ATTENUATION)` — subtractive, matching his `max(LOSS,LIGHT)-LOSS` |
| per-axis diffusion `mix(lightpy, lightny, x)` | the `side_dir` loop with `SIDE_LIGHT_ATTENUATION` |
| `light * neighbor_solid_color * bounce_loss` | the `Block(color)` reflect branch |
| — | **the three-tier diagonal occlusion test** (his only in the *"I just didn't output the emission diagonally"* bugfix note) |
| 10 bits × 6 dirs × RGB | 8 bits × 4 dirs × RGB |
| 6 directions (3D) | 4 directions (2D) |

**They agree on every point they both cover**, which is a genuine independent corroboration of the
rule I reconstructed from prose. The kernel adds the diagonal-occlusion detail; x1m4 adds dimension,
precision, opacity, injection and cascading.

---

## What porting to 3D actually costs

This is where the two designs diverge, and it explains a choice of x1m4's that otherwise looks
arbitrary.

Going 4 → 6 directions is trivial: storage becomes 3 × 6 u8 = **18 bytes/cell** (x1m4's 10-bit version
is 180 bits ≈ 23 bytes). The problem is the diagonals. A 2D Moore neighbourhood is 8 cells; a **3D
Moore neighbourhood is 26**. The diagonal loop above would grow from 4 diagonal neighbours to 12 edge
neighbours plus 8 corner neighbours, each needing its own occlusion bracket test — and the bracket test
itself gets harder, because sealing a 3D corner means checking three faces, not two.

○ **That is almost certainly why x1m4 runs two passes instead of one.** He describes CAGI as
*"propagation, filtering"* — a propagation pass at fixed face directions, then a separate diffusion
pass that *"does the blurring and lifts that limitation"*. A single-pass full-Moore stencil at 26
neighbours per cell is too many reads; splitting it lets the cheap 6-neighbour pass run every tick and
lets the diffusion pass approximate what the diagonal terms would have done. His remark that he
*"fully preserves directionality of the main 6 face directions and uses some extra directional guiding
of all 26 neighbor directions"* reads, in this light, as exactly that compromise.

**So: this kernel is what CAGI looks like in 2D as one pass. x1m4's is what it becomes in 3D as two.**

---

## If you were to build this

In rough order of what will cost you time:

1. **Tune the five constants,** not the structure. `DECAY` sets range; `DIRECT_ATTENUATION` vs
   `SIDE_LIGHT_ATTENUATION` sets how collimated beams stay; the two diagonal constants set how much
   light rounds corners. This is the whole aesthetic.
2. **Replace the float reflect** with a fixed-point multiply, or you lose determinism for nothing.
3. **Get the corner-seal case right first.** Both-blocked → zero contribution. Everything else is
   cosmetic; this one is the difference between "lighting" and "light leaks through my walls."
4. **Add subtractive-only loss everywhere.** Any multiplicative decay anywhere and your dirty-chunk
   culling stops paying off.
5. **Then** add what this kernel omits, in x1m4's order: skylight/sunlight injection into the same
   volume → per-face opacity from sub-cell occupancy → dirty-chunk masks (1 bit per 8×8) → cascades.

Cross-references: [x1m4-index.md](x1m4-index.md) for the assembled CAGI picture,
[x1m4-graphics-programming-channel.md](x1m4-graphics-programming-channel.md) §3 for his rule and
performance numbers, [x1m4-archived-voxelchain-channel.md](x1m4-archived-voxelchain-channel.md) §1–2
for the origin and the ~1-cell-per-tick propagation speed.
