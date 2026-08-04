# x1m4's engine — the #showcase and #chat channels, 2022-07 → 2026-08

**Source:** `temp/showcase` and `temp/chat` — two more single-channel Discord exports from the
same guild (`1003288330391273492`, Graphics Programming / originally the VoxelChain server).

| Channel | id | messages | x1m4's | range |
|---|---|---|---|---|
| #showcase | `1006843475494445137` | 20 284 | **4 851** | 2022-08-10 → 2026-08-01 |
| #chat | `1003298200909778984` | 20 016 | **4 456** | 2022-07-31 → 2026-08-03 |

Together with [x1m4-graphics-programming-channel.md](x1m4-graphics-programming-channel.md)
(1 854 msgs) that's **11 161 of his messages** across three channels. These two go back to
**2022-07**, eight months earlier than #graphics-programming, and #showcase is where he posts
his *own* progress — so this is the channel that fills the gaps the other doc flagged.

**What's here that isn't in the other two docs:** the engine's name and history, the
bits-per-voxel budget, the pre-CAGI CA-shadow lineage, the **complete audio architecture with
numbers**, the entity voxelizer's real algorithm, the two-year fluid-sim arc, the 2D
prototyping lab where everything is built first, the public demo URLs, and the reason 2025 is
nearly empty.

**Confidence marking:** ▸ = his words, quoted and dated. ○ = my inference.

---

## 1. Identity, lineage, and the missing year

**The engine is (was) called VoxelChain.** GitHub org `VoxelChain`, site `voxelchain.app`, his
personal GitHub is `maierfelix`. He came to regret the name:

> ▸ *"even though the chain part in voxelchain has nothing to do with crypto, it's an
> unfortunate and confusing choice"* (2023-10-17)

○ In later messages he stops using the name entirely and just says "my voxel engine". The
current engine is **not** VoxelChain — see the rewrite count below.

**He also works on a second, separate engine commercially.** 2023-01-16:

> ▸ *"btw I think I haven't mentioned it publicly here, 2 weeks ago I started working for
> https://twitter.com/voxraygames, so development on voxelchain will be slower for a while —
> but it's also a good thing since now I can collect ideas and get away from the 'tunnel
> vision' I recently got on voxelchain"*

VoxRay is a 5-person team (2023-04-24: *"we are 5 people"*), and IchBinAlex identified the
collaborator: *"the Lobster developer and xima are working together on the voxray engine"* →
▸ *"yep!"* (2023-10-19). ○ That's Wouter van Oortmerssen (`@wvo`), author of the Lobster
language — x1m4 posted his tweets. VoxRay shipped something called **Voxlands** ("*quite
insane to see someone you have watched the first time as a kiddo and now he plays something
that you were part of in making*", 2024-02-26). He uses "we" about VoxRay's grass and lighting
critiques, and says its lighting is the best part of it.

**He also does CAGI as contract work:**

> ▸ *"yup and runs in the browser too using webgpu — I implemented cagi into their engine last
> year"* (2025-06-24, about **Tesera**)
> ▸ offering the same to another dev: *"if you ever look for a collab regarding lighting then
> hit me up — I'm not sure what direction you want to move with your engine, but in case you
> want to stick closely to Minecraft then CAGI can also be done integer based simply being a
> directional extension for the old flood fill based lighting, so it can be used for mob
> spawning and redstone etc."* (2025-08-13)

**Why 2025 is nearly empty in all three channels** — he said it plainly:

> ▸ *"oh I'm currently working full-time for a company, but under big NDA, can't even say the
> name 💀 — and in my free time currently do some home automation"* … *"not really [a game],
> just doing graphics dev"* (2024-12-12)
> ▸ *"I don't want to get rich, I want to make a few bucks with this and get back to game and
> voxel engines asap"* (2025-03-25) / *"definitely coming back to my voxel engine though"*
> ▸ *"currently trying to learn game design in a 2d project, voxel engine is not abandoned :)"*
> (2024-11-18)

○ So the Dec 2024 → Oct 2025 quiet is a **paid full-time job plus a deliberate pivot to 2D
game-design practice**, not abandonment. Engine work resumes visibly 2025-10-14 (fluid sim) and
fully 2026-06 (mod system).

**The engine has been rebuilt from scratch 5–6 times:**

> ▸ *"also this is not the first iteration of the engine, the engine got thrown in the trash
> like 5-6 times and completely rebuilt from scratch before the current engine state. also lots
> of prototyping in 2d for exploring game ideas and simulation techniques such as the cellular
> automata gi"* (2024-03-23)

**Earlier lives:** WebGL first, then WebGPU; the simulation was **C + WebAssembly with
multithreading** before being ported to GPU compute around May 2023:

> ▸ *"the simulation engine was in C but a few months ago I ported everything to the GPU since
> even with multi-threading the C code couldn't anywhere match the parallel speed of the GPU"*
> (2023-10-19)
> ▸ *"small news btw, but over the weekend I managed to get the whole simulation engine running
> in a compute shader"* (2023-05-08)

He also maintained **WebGPU hardware-ray-tracing forks** before WebGPU shipped:
`maierfelix/dawn-ray-tracing`, `maierfelix/chromium-ray-tracing`,
`maierfelix/webgpu-examples/tree/master/ray-tracing` (2022-11-16, 2024-01-06).

---

## 2. World representation — the budget that rules everything

This is the single most load-bearing set of facts in the export, because every other decision
follows from it.

| Quantity | Value | Date |
|---|---|---|
| World size | **512³** main voxels (some msgs 384³–512³); with sub-voxels ≈ **4096³** effective | 2024-01-27, 2025-03-16 |
| Bits per voxel | **64, up to 96** once the mass sim lands | 2024-01-27, 2024-03-23 |
| ↳ breakdown | 32 bits general (material id, rotation, animation frame, electricity) + 32 bits mass sim (mass + xyz velocity) | 2024-01-27 |
| Sub-voxels | **8³ per main voxel** (≈ Minecraft texel scale); considered 13/15/16³; **16³ is the leak limit** | 2024-02-22, 2025-03-16 |
| Rotation | all 90° rotations (**24 distinct**, "up to 26") in **5 bits**, later 4 | 2023-01-16, 2024-04-01 |
| Acceleration structure | **non-sparse octree encoded into the MIP levels** of the volume, + sub-voxel (brickmap-like) tracing | 2025-07-16 |
| AS rebuild cost | full regen of **1024³** ≈ **10–15 ms** | 2022-11-15 |

The rotation trick is public: [gist 2807ad81904748e87d3aa806b094d782](https://gist.github.com/maierfelix/2807ad81904748e87d3aa806b094d782)
— ▸ *"that's how I then apply the rotation in shaders, it's absurdly fast"* (2023-01-16).

**MIP-pyramid AS, built without atomics** — he explained it twice, and shared a working
JSFiddle ([v4ud2bx9/27](https://jsfiddle.net/xma44/v4ud2bx9/27/), see `mipsFragmentSrc`):

> ▸ *"basically just fetch the 8 neighbors and then make the current voxel in the current mip
> level either 0 or 1 based on if any neighbor was solid or not"* … *"instead of doing 0 or 1
> you can also do the hashing part here — or even calculate an opacity factor, which I
> previously did for super fast large scale ambient occlusion"* (2023-12-30)
> traversal: *"do `pos >> 1` and read the voxel from mip level 1 — this essentially gives you
> the information if the area of 8 voxels is empty or not with just 1 texture read"* (2024-02-24)

**Why no compression, ever.** This is the recurring argument he makes against every voxel
engine that leads on voxel count:

> ▸ *"assume it's completely random 64bits per voxel, it's just too much entropy — the only
> potentially compressible part are the subvoxel material ids because there can be repetition
> along neighbours, but everything else like the 4 bit rotation, 8bit material data and
> especially the mass stored in voxels is impossible to compress efficiently in real-time"*
> (2024-04-01)
> ▸ *"with the mass sim I can't go even sparse anymore btw because air has mass too"* (2024-01-27)
> ▸ *"if I'd kept the lighting as part of the sim it would be even worse, as it completely blows
> up entropy"* (2024-04-01)
> ▸ *"the subvoxels are a storage optimization, not a performance one"* — they cost the same as
> tracing at 8× resolution (2024-01-27)

And the thesis he built out of it:

> ▸ *"hot take: if your voxel engine is too detailed, then the great voxel filter will make you
> abandon your engine. it's this weird thing I noticed that voxel engines focusing on rendering
> as much voxels as possible have a tendency to get abandoned. initially I thought as much
> voxels as possible is the way too to create something new and unique, but usually it requires
> some kind of compression and simplification of your voxel data, which inherently makes using
> the voxels for game related stuff harder"* (2024-02-18)

○ This is the cleanest statement of his whole design philosophy: **simulation capacity is
bought with memory, and compression spends the same budget.** He also says, repeatedly, that
non-sparse octrees are what he'd recommend: *"yup for performance I found them unmatched,
especially compared to sparse octrees"* (2025-03-15).

**Integer DDA.** He switched the traversal to integers for precision — *"that exact problem is
what made me switch to integer dda"*, *"integer for most of the stepping"*, keeping only the
distance `t` as float (2024-04-04, 2024-04-07).

---

## 3. The lighting lineage, in order

The other doc covers CAGI's rule and performance. What #showcase adds is the **chain of
predecessors**, which matters because CAGI is the endpoint of a five-technique search.

**1. Voxel cone tracing (first lighting in the engine).**
> ▸ *"the first lighting I had in my voxel engine was actually cone tracing, but had to ditch
> it because it's almost impossible to fix the light leaking"* (2024-03-21) — with the clearest
> explanation of cone tracing in the whole export: *"you start your cone at mip 0, and march it
> for let's say 16 steps, every few steps you increase the mip level you sample from, which
> makes the ray volumetric"*.

**2. LPV / SH world-space radiance cache** (the voxelchain-era demo). Covered in the other doc.

**3. CA shadows and CA skylight — 2023, well before CAGI.** This is the missing link:
> ▸ *"btw the cool thing is that the shadows aren't ray traced but instead are propagated with
> cellular automata — found this trick a few years ago and it's a nice alternative to ray
> tracing shadows which usually is very slow"* (2023-08-13)
> ▸ *"I found 2 good reasons for that: first tracing shadow rays to the sun for every pixel is
> really slow; second since the shadows and skylight are part of the simulation state, it's
> possible to find dark areas to e.g. let mobs spawn"* (2023-05-26)
> ▸ *"and I'm pretty sure it can be extended to be used for stuff like sky light and ambient
> occlusion too … you can probably create a whole fake gi solution with this, including bounced
> colors"* (2023-08-13) — **that is CAGI, predicted six months early.**

The **1-bit shadow CA**, fully explained (2024-07-06) and used in the live pixel demo:
> ▸ *"you set every solid cell to 1 and then for every non-solid cell, you lookup the neighbor
> cell towards the light direction and just propagate that information — you propagate shadows,
> not the actual light"*
> ▸ *"the shadow bit contains no directional information … for each cell, you just lookup the
> adjacent neighbor cell towards the light source, and then take the shadow bit of the neighbor
> cell and store it in the current cell"*
> ▸ limits: *"the angles in this are limited to 45 degree angles"*, *"the only limitation really
> is that it's limited to just 1 light source"*; soft shadows via supersampling; a 4-bit variant
> with a 4-neighbourhood blur exists.
> ▸ and the punchline: *"yeah making the shadow bit not just a 1bit state but also store
> directional information would be cool — but that's basically CAGI then"*

**Skylight** is not his invention either — it's from the same source as his terrain generator:
> ▸ *"I'm using the bwerness skylight method and it works really nice — vertical slice based
> skylight propagation and taking 9 samples of the upper hemisphere each step. I save it into a
> 32bit uint where 8 bits are used for the propagation and other 8 bits are used for filtering
> and temporal accumulation … it's pretty fast and takes about 0.5ms per slice"* (2023-09-20)
> ▸ *"the CA skylight is basically just propagating skylight vertically down into the world and
> when it hits stone or water it decreases — gives water a nice depth gradient for deep sea"* (2023-08-23)
> ▸ later refined: *"a sorta downwards flood fill with special diagonal weighting"* that also
> *"injects the skylight into the light volume each step"* (2024-03-24)
> ▸ AO idea from the same family: *"an inverse minecraft flood fill — e.g. stone cells have a
> starting value of 16 and propagate it into air and each step subtract 1 … and then you inject
> skylight into your world based on that value and you get nice ao (with bouncing!)"* (2024-03-24)

**4. Screen-space + world-space path tracing.** Peak state 2023-09-22: a side-by-side against a
2 spp / 3-bounce path-traced reference where ▸ *"aside from the colors being slightly off for
some reason, I see no major difference"*. Temporal factors at that point: **1/32 for both
caches, 1/128 for skylight, sunlight instant** (2023-09-20).

**5. CAGI.** The switch is dated precisely:
> ▸ *"currently porting my cellular automata GI into my voxel engine and it looks pretty insane
> — I think I'll completely move away from path tracing after this. the balance between
> performance and quality is just so much better with world space only lighting"* (2024-02-18)
> ▸ *"I actually completely dropped the ray tracing stuff in my engine except for gbuffer rays
> and stuff like ambient occlusion and reflections"* (2024-03-01)
> ▸ when told he'd rewrite lighting again in three months: *"this time I'm very sure I'll stick
> with it — the previous techniques all had various problems and were never even close to what I
> have now"* (2024-03-01). He was right; it's still CAGI in 2026.

### CAGI details this export adds

- **Two passes: propagation + diffusion.** ▸ *"I'm doing CAGI in two passes (propagation,
  filtering)"* (2024-12-21); ▸ *"the propagation step is at fixed angles, the diffusion step
  does the blurring and lifts that limitation … one diffusion step and also do the injection
  there for emission, sunlight and skylight"* (2024-07-16).
- **Not 4/6 directions of *propagation*** — a correction he makes explicitly: ▸ *"it's more than
  that btw, it's stored in 4 but that doesn't mean the propagation and bouncing is only 4
  directions. with only 4 directions it would look completely crap"*; ▸ *"mine fully preserves
  directionality of the main 6 face directions and uses some extra directional guiding of all 26
  neighbor directions"* (2024-02-19, 2024-03-20).
- **Sub-voxel leaking is solved with a precomputed per-face opacity map:** ▸ *"I pre-calculate
  an opacity map for each subvoxel model face (6 faces in total) and use that during propagation
  to dim the passed through lighting — you can use isotropic (same opacity for all 6 faces) too
  but it's definitely a lot less realistic than anisotropic opacity"* (2025-03-16). Limit:
  *"with my game scale I can use up to 16^3 large subvoxels without too much inner leaking"*.
- **Sunlight is injected from the shadow map, including into air:** ▸ *"in my voxel engine I
  inject sunlight from the shadow map — so not only the hit point at surfaces gets injected but
  also some sunlight within air"* (2024-07-16). Later he switched sun shadows to CA and **locked
  them to 45°**, killing the day-night cycle: ▸ *"wanted day-night cycle for a long time, but
  recently switched to a cellular automata based sunlight locking sun shadows to 45 degree
  angles :<"* (2024-08-13).
- **Known failure mode:** ▸ *"my sunlight is relatively bright compared to emissive blocks, so
  it sometimes messes up with directions of the lighting"* … *"brighter light sources tend to eat
  up less bright ones and screw up their directions"* (2024-07-16). Mitigation considered:
  handling emission / sun / sky in **separate** volumes.
- **Brute-force throughput ceiling:** ▸ *"512×256×512 where it gets at about 6-10ms"*; with
  interlaced updating *"crazy fast"*; **1/8 of the volume per frame globally, full 60 fps near
  the player** — *"so it's pretty much lod but for lighting"* (2024-03-28).
- **Free AO for free:** ▸ *"with the right surface sampling position (nudging the light sampling
  position slightly outwards from the surface) it's basic + free ambient occlusion"* (2026-04-25).
- **Cascading advice for others:** ▸ *"the important thing is having a proper lower res
  representation of your scene, which I found the hardest part of implementing cascading for any
  kind of volumetric light solution"* (2025-05-07).
- **Colour space:** 10-bit for CAGI, **16-bit for the rest of the pipeline, then AgX** (2024-03-20);
  the shipped tonemapper is `tony-mc-mapface` (2024-12-23). ○ Both appear; AgX was likely the
  2D prototype.
- **It briefly ran on floats.** In the *2D* prototype he switched to float textures because
  ▸ *"with integers everything has to be done with min/max operations … simple example is how
  would you do surface energy absorption with ints? you can't do something like energy * 0.9"* —
  and he accepted losing determinism there because *"I don't plan on using the lighting for the
  game logic in this prototype"* (2024-03-20). The 3D engine stays integer.

### The denoiser — voxel-face space, not screen space

This is his most transferable rendering idea and it's only explained here:

> ▸ *"my denoiser is mostly just a temporal and spatial filter and combines the path traced
> lighting with a simple box filter … few extra cases are handled, like on steep surface angles
> it blurs not only based on voxel ids but along the main voxel as well to prevent flickering"*
> (2023-12-31)
> ▸ *"it aggressively blurs the lighting over voxel faces and makes it constant by that over 1-2
> frames. it's relatively easy to denoise voxel faces, i.e. not like denoising just direct pixel
> neighbors, but instead project voxel faces from world-space back into screen-space which then
> lets you pick the lighting of neighbor voxel faces for every pixel — like not denoising per
> pixel, but per voxel face. it's like not screen-space anymore, but voxel-face space kinda"*
> (2024-03-20)
> ▸ *"sample per pixel but filter both temporally and spatially per voxel"* (2026-02-21)
> ▸ per-voxel diffuse, per-pixel specular: *"yup! per-voxel reflections can look a bit annoying
> in certain scenarios"* (2026-07-30)

The pixelated look is a *side effect*: ▸ *"I have a denoising pass though but it's mostly used
for making the light pixelated and hide some noise from jitter, not really to denoise the
lighting"* (2024-07-25). It also pixelates the water, which he kept because he liked it.

**LOD in the ray tracer** (2023-09-26): based on camera distance and surface angle he picks a
sub-voxel subdivision count *and* a mip level — close geometry gets 8 sub-voxels, distant
geometry becomes a single face, *"and the denoiser can easily blur the lighting over these faces
then. often flat surfaces like floors look horrible and killed all my previous denoising
attempts."*

**Checkerboard upscaling**, with the actual index formula:
> ▸ `((intUv.x + intUv.y) & 1) ^ (frameCount & 1)` (2023-09-20)
> ▸ *"the irradiance shading pass uses it and gets an almost 2× speed up without any noticeable
> quality impact"* (2023-10-18); pink debug overlay marks reconstruction failures, caused by TAA
> camera jitter. Also *"useful for cheap fake transparency and afaik teardown does transparency
> this way"*.

Bloom is Froyok's UE4 article: **6–7 downsamples with a 13-tap filter, upsample with 9-tap,
additive at 0.8**, subtle threshold on input (2024-02-28).

---

## 4. Audio — the complete architecture, with numbers

The single most valuable section for atrium. Nothing in the other doc has this level of detail.

**Arrival at the split** (2023-04-14, before any implementation):
> ▸ *"reverb seems complicated though, usually you need a feedback buffer and fft to implement
> reverb … I think in my case I'd instead use the webaudio reverb/delay convolver. so I generate
> and ray trace the sound on the GPU and then feed the sound + reverb intensity to the cpu"*

**Why on GPU at all** — not performance, *epistemics*:
> ▸ *"yes my world and game state are entirely on the GPU, so my only chance was doing the audio
> on the GPU too since the CPU just has no clue what's going on"* (2024-03-20)

**What the GPU computes:**
> ▸ *"for reverb which is probably the easiest effect, you just do the same as for skylight
> tracing, but interpret the result as an echo instead based on the average of the traced rays
> results"* (2023-12-31)
> ▸ *"mainly material reflectance and ray hit length, but I also influence it by skylight — the
> more skylight, the less reverb. and vegetation also reduces reverb more than other materials"*
> (2024-04-02)
> ▸ occlusion: *"for every nearby sound, trace a few rays from the sound towards the player and
> see if there is any blocker and calculate an occlusion value based on that"* (2024-01-04)
> ▸ *"and on the gpu you can go nuts with it, as compared to the cpu you can easily trace
> millions of rays"* (2023-12-31) / *"I can easily shoot millions of rays for each sound, no
> problem"* (2024-01-04)

**What the CPU does, and the real bottleneck:**
> ▸ *"ray tracing the sound is actually not the performance bottleneck lol — it's the CPU
> calculating stuff like reverb. didn't know this before, but stuff like reverb apparently is
> pretty expensive"* (2024-01-04)
> ▸ *"since recently the CPU now receives a sound queue from the GPU and applies effects like
> reverb, gain and cutoff"* (2024-01-08)
> ▸ *"with a convolution node and an impulse response buffer that was recorded in a cave
> environment"* (2024-04-02)
> ▸ *"I ended up sending everything back to the CPU and only use the GPU to spawn and sample the
> sound environment, then send back to the CPU and let the soundcard do the filtering"* (2025-04-06)

**The queue sizes — hard numbers:**
> ▸ *"my GPU queue is currently capped at 32 sounds per frame at max, and the CPU queue is capped
> at a maximum of 64 parallel sounds that can be played"* (2024-01-04)

**The cheat he takes, and knows he takes:**
> ▸ *"actually atm the sound is only ray traced once, so if you move the player quickly then the
> sound properties won't change along it (only position and volume do). I can fix that though,
> I'm just not sure if it's worth it as it would be a lot more expensive to calculate as for
> every active sound it would have to be ray traced every frame until the sound finishes"* (2024-01-04)

○ i.e. **acoustic parameters are frozen at sound-trigger time.** That is the opposite of
atrium's continuously-updated per-source solve, and it's the honest cost/benefit line to argue
about.

**Latency:**
> ▸ *"I'm only sending very small data from the gpu to the cpu and yes the audio is slightly
> delayed"*; *"my audio runs separate from drawing"* (2025-03-19)
> ▸ a Windows-specific gotcha: *"there is an enhanced audio option in windows and if you turn it
> off for chrome then there is a lot less audio lag"* (2024-01-06)

**Entity sounds are emitted from inside the per-entity behaviour shader** — the WGSL for this is
in the other doc (§6). Sound ids are mod-registry codegen. Footsteps had a delay bug traced to a
typo in the sound ray-tracing pipelines (2024-06-10).

**On the difficulty of audio, twice:**
> ▸ *"when I learned about audio I quickly realized it's so much more complicated than I
> expected"* (2024-10-30)
> ▸ *"not necessarily harder [than lighting], it got mostly solved about a decade ago (at least
> as far as I'm aware), but it's a lot less documented and less entry level friendly than
> lighting … it's easy to play audio on a machine, but synthesizing in real-time or have
> realistic ray traced audio is really hard too, just like realistic lighting is compared to
> ndotl lighting"* (2024-10-31)
> ▸ *"been using it [WebAudio] for my voxel engine and heck even reverb was quite tricky to get
> right (mostly for performance)"* (2026-04-23)

**Cave-ambience heuristic, reasoned out loud** (2024-03-01) — directly relevant to atrium's
ambience work:
> ▸ *"makes me wonder how you can reliably determine if the player is currently in a cave or not.
> I guess, you need a combination of environment evaluation similar to ray traced sound to
> determine the room size, determining the average light level for a spookyness factor and the
> average nearby block type to figure out the type of sound to play"* … then rejects biome-keying
> because mining out the biome would leave the sound playing, and lands on *"y level … the average
> block amount of the current and nearby chunks, and the light level"*.

**Synthesis — he did come back to it.** He wanted circuit-programmable synthesis (2023-04-25), tried,
and gave up: ▸ *"first wanted to also generate the sounds procedurally on the GPU but it's really
damn hard — will probably stick to pre recorded sounds instead"* (2023-12-31). Sounds at that point
were from Pixabay (2024-05-23) with ElevenLabs SFX considered (2024-06-17). **But by 2025 the shipped
build synthesises sounds at startup** — see [x1m4-devlog-channel.md](x1m4-devlog-channel.md) §1, plus
granular pitch shifting (2024-06-11). Treat the "gave up" line as true only for 2023–24. The aesthetic target is
worth noting: ▸ *"dinosaur sound with some kind of 8 bit sound filter … like a very deep, dark and
scary sound but in a 8 bit retro sound design"* (2024-05-31).

He also tracks the field: Teardown ▸ *"just noticed that teardown seems to have ray traced sound
environment evaluation too"* (2024-02-27), UE's SH-volume sound propagation, the Noita devs' own
spatialisation talk, and the SEUS audio mod.

---

## 5. Entities and items

**The algorithm, in his own full description** (2025-05-18, the best single account anywhere):
> ▸ *"the geometry I process is defined analytically as OBBs that support animation (translation,
> rotation, scale) and is generated by a tool called blockbench (it also outputs a skeletal tree).
> then in a compute shader, for every entity I apply the animations of each OBB and also calculate
> a billboard for voxel splatting (basically generating the min/max of a quad in screen-space).
> then using the voxel splat billboards, for every intersected pixel I loop through each OBB of
> the entity and check if there is an intersection or not. The OBB intersection check is slightly
> expanded, because voxels often slightly extrude the sharply defined OBB surface. after the
> intersection check, I calculate the start and exit of the DDA ray and then DDA the content of
> it. this actually took me a while — about a month to implement because making an on-the-fly
> shader-based voxelizer fast was far more complicated than I thought"*

Key properties:
- **World-space voxelization, not object-space** — ▸ *"they aren't part of [the grid], but they
  are ray traced as so, I call it world-space or true-grid voxelization … world-space voxelization
  was actually a nightmare to figure out performance wise, I think that's the reason why other
  games like teardown stuck with object-space voxelization for dynamic objects. and world-space
  is definitely a bit slower than object-space for sure, but the unusual look of it is worth it —
  it's almost like true 3d pixelart. or actually it is true 3d pixelart"* (2024-03-31)
- **The 3D-texture route was tried and abandoned** — ▸ *"first I voxelized into 3d textures but
  it scaled horrible"*; *"most voxel games voxelize objects into their own volume, but here I
  tried to voxelize everything into a fixed world grid … it can be scaled into the millions
  easily since the voxelization is done just within a shader"* (2025-05-18)
- **Not part of the lighting; inserted into a clipmap for occlusion/colour** — ▸ *"voxel clipmap
  is when you rasterize your entities into a multi-res voxel grid around the camera right"* →
  ▸ *"yep!"*; *"they have colors, but only if the source is visible in screen-space, otherwise it
  fallbacks to only occlusion"* (2024-03-19)
- Animation is **Blockbench-driven with a hand-written GPU animation function**, because
  ▸ *"blockbench supports code based animations, but on the GPU it's not possible (or at least not
  efficient) to dynamically execute code"* (2024-01-31)
- SDF shapes work in the same intersector: ▸ *"the ray tracer supports not only box SDFs but all
  kinds of SDFs"* (2024-02-29). He planned **forward-kinematic hair/fluff** using body OBBs as
  colliders — *"per hair joint that would be 2 analytic obb to point intersections"* (2024-04-01).
- **Pathfinding is flow fields, not A\*** — ▸ *"flow field is by far the most fitting choice on
  the gpu"*; *"literally just minecraft flood fill but used for path finding"* (2024-03-01/06).
  Explained for others 2025-10-26: flood-fill a distance field from the target, then read the
  gradient across neighbours.
- **Crowding via a force grid** — entities write into a grid; high values push others away
  (2024-05-23). Same mechanism as his particles.
- Items: stored in a storage buffer *"basically just particles"*, with a per-chunk sorted list or
  a grid-of-lists for spatial queries; deliberately de-emphasised in favour of conveyor belts
  (2024-05-23/28).

---

## 6. Fluids — a two-year arc, and the only sim he actually finished

Longest single engineering thread in the export. Dates matter here.

| Date | State |
|---|---|
| 2023-06-23 | First appearance, built with a collaborator: *"grid based, fully conservative and deterministic with integer data"*; underwater-base pressure already works |
| 2023-07-07 | Heat sim: lava↔stone, water↔steam by temperature; *"both lava and hot metals need such insanely high temperatures that it's tricky to integrate"* |
| 2023-08-11 | Basis published: [w-shadow.com CA fluid article](https://w-shadow.com/blog/2009/09/01/simple-fluid-simulation/) |
| 2024-03-29 | Velocity + multi-type + reactions + foam; *"no mass loss and the simulation also fully stabilizes after a few seconds, so idle areas can be completely culled"*; mass delta ≈ 8–16 per height level |
| 2024-04-07 | Explores **incompressible** via scanline vertical pressure + per-axis flood fill → velocity |
| 2025-05-14 | Buoyancy (oil < water) and compressibility mostly worked out |
| **2025-10-14** | ▸ *"finally got the simulation to a point where it's literally 100% stable — the energy in the sim correctly dissolves over time, there are no longer any oscillations and it's still fully conservative and deterministic … took me like 1 year on/off working on this to finally reach this point"* |
| 2026-06-25 | Port 2D → voxel engine starting |

**The mechanism, in his words:**
> ▸ *"the first and most important step is that you want mass to equalize with the direct
> neighbors — e.g. to equalize horizontally, you do current mass / 3. the other stuff is then just
> regular fluid dynamics, like converting mass transfer into flow/velocity and do a feedback loop
> which gives the wave behaviour"* (2024-03-24)
> ▸ *"there is only mass and a cell type, but the cells don't move in the regular sense, it's only
> mass that gets moved along neighbors if they have the same cell type. the only place where the
> cell types itself are handled is when two different cell types are next to each other, which then
> either leads to a material reaction (water→lava→stone), or a mass swap"* (2024-03-24)
> ▸ velocity was the unlock: *"before I had big problems of mass equalization being slow, but the
> velocity made it like 10x faster — no more giant mass hills forming like in noita"* (2024-03-24)
> ▸ **gravity is applied virtually, never written into state** — *"the velocity data is never
> touched by the gravity, instead it's just added on top during the mass exchange"*, specifically
> so the dirty cache still works (2024-03-24)
> ▸ mass cap at u32; foam = ▸ *"if a cell has an air cell neighbor and enough energy, it adds a
> foam intensity value based on it's vertical and horizontal energy, and that value is then spread
> along nearby water neighbors with flood fill and gets decreased every tick"* (2024-03-29)

**Two hard-won process lessons:**
> ▸ *"initially I wanted to implement this with pure double buffering but it turned out to be so
> insanely hard that I instead went with the single buffer noita checkboard style updating again"*
> (2024-03-24) — and the later general verdict: *"I've recently switched to a single-buffered
> simulation btw, solving in double-buffered sims scales so incredibly bad in terms of complexity.
> reduced my code like 10x and also made it like 10x more readable and performant"* (2024-11-28)
> ▸ *"I only port 2d prototypes over if I'm absolute sure that the main headaches in terms of
> functionality and performance are completely solved"* (2024-03-28)

**Water rendering was thrown away and redone** (2024-03-24): water moved to its **own texture,
traced separately from world voxels, with no sub-voxels** — *"should improve performance a lot
since the water is a lot easier to intersect with"*. Confirmed 2025-07-16: *"[ray marching] just
for the water, everything else is ray traced except the lighting which is CAGI"*. Smooth surfaces
come from Mytino's suggestion of treating the grid as a **density field with a mass threshold**
(2024-08-06).

**Rejected alternatives, with reasons:** MPM/particle-in-grid (*"lacks very precise/predictable
behaviour if you want to work on a per pixel level like Noita does"*, 2023-12-05); reintegration
tracking (*"definitely not conservative when I tried running it locally"*, 2023-06-23); LBM
(*"hard to do with integers … you loose mass"*, 2024-03-24); particles generally (4 M with a
uniform grid vs *"+100 million"* grid-based, 2025-05-15).

---

## 7. Physics ambitions — CA all the way down

- **CA collision / rigid grouping**, worked out live 2024-03-22: per cell-group, look up the
  neighbour along the translation vector; if *all* group cells can move, move. Two groups closing
  on the same gap need a **lock bit** implemented as an atomic counter — if >1 group targets a
  cell, nobody moves. ▸ *"first pass does atomic add for collision detection and the next pass
  then performs the move"*. He explicitly avoids atomic swaps: *"atomic swap/movement sounds pretty
  slow, with atomic add it should be a ton more efficient and deterministic"*.
- ▸ *"the CA handles the grouping (aka glueing stuff together) and in result can turn groups into
  virtual objects; if you provide an inertial tensor and stuff then it might be possible to very
  efficiently solve rigid body in parallel with it"* (2024-03-17)
- **Structural integrity** — the same two-flood-fill idea as the other doc, plus the concrete
  reference: the Valheim/Medieval-Engineers style diagram, and the open question ▸ *"they seem to
  start propagation by the ground layer, but I wonder if it's possible to not define any ground
  layer but use some kind of extra vertical propagation instead"* (2023-08-17). Still unbuilt as
  of 2024-02-13: *"really the only thing I still have to solve with the mass sim is structural
  integrity"*.
- **A separate 2D GPU physics solver exists** (2024-12-05): **1 million bodies**, later
  sphere-sphere collisions and much larger worlds (2025-05-20). He spent time on whether it could
  be married to the falling-sand grid and concluded rotation is the blocker: ▸ *"if you extract a
  group of pixels from the sim, then there is no 100% chance that you can successfully re-inject
  them"*, and ▸ *"regarding gameplay I'm also wondering if leaving out rotations could simplify
  things, I always disliked the massive chaos both technical and gameplay wise that rotations
  introduce"*.
- **Grass/vegetation animation** is not geometry at all: ▸ *"I just distort the ray origin when
  entering the subvoxel volume … with a simple function that distorts based on the main and sub
  voxel world pos and frame time, and a vertical gradient so the higher a sub-voxel the more it
  gets distorted"* (2023-04-06). Wind is ▸ *"4 sines and a bunch of fracts"* fed x/y/z + time
  (2024-03-23). Fix for clipping: leave a 1–2 sub-voxel gap to the model bounds (2024-03-27).
  ○ Contrast with atrium's hierarchical gust model — his is cheaper and deliberately unphysical.

---

## 8. Terrain generation — the algorithm, named and public

Everything traces to one 2017 blog post and one Processing sketch:

> ▸ *"I have a terrain generator that can generate quite interesting terrains in just a few lines
> of code, it's diamond square fractal based but extended with cellular automata"* (2023-03-27),
> inspired by [softologyblog voxel-automata-terrain](https://softologyblog.wordpress.com/2017/05/27/voxel-automata-terrain/)
> ▸ *"it's an (unusual) extension of the diamond square fractal, which uses CA rules instead of
> averaging for upscaling"* (2026-02-13)
> ▸ **his cleaned-up port is open source**: [VoxelChain/voxelchain-terrain-generator](https://github.com/VoxelChain/voxelchain-terrain-generator/blob/main/src/index.ts)

○ **Correction from reading that source (2026-08-03):** it is **not diamond-square**, despite how he
describes it. `src/index.ts` runs **log₂(resolution) hierarchical subdivision passes**, and at each
level applies four families of **totalistic CA rules** — cube, 6 faces, 12 edges, 3 outer faces — each
a 4D lookup table indexed by *how many* neighbours hold each state (`rule[count0][count1][count2][count3]`),
not by their arrangement. `lambda` is **rule-table sparsity**: per entry, `random() > lambda` leaves it
empty, otherwise it gets a random state 1..N. Default 4 states (0 = empty + 3 terrain variants). The RNG
is entirely external via a `randomCallback`, which is how he swaps in his own deterministic PRNGs. So
"diamond square + CA" is his shorthand for "subdivision hierarchy + counting rules" — a meaningfully
different algorithm from diamond-square averaging, and the thing to reimplement if you want his terrain.

### He uses *two* Softology algorithms, not one

Easy to conflate. Both are Softology (Jason Rampe), and they are the same *technique family* applied to
different problems:

| Post | Used for | How |
|---|---|---|
| [voxel-automata-terrain](https://softologyblog.wordpress.com/2017/05/27/voxel-automata-terrain/) (2017) | **terrain** — `voxelchain-terrain-generator`, `strata-voxel`, the in-engine generator | subdivision hierarchy + rule tables |
| [accretor-cellular-automata](https://softologyblog.wordpress.com/2018/01/12/accretor-cellular-automata/) (2018) | **grown structures** — the video `8cWogE6dJoY`, the 2026-02 structure experiments | growth outward from a seed + rule tables |

▸ *"don't have the rules anymore, but they aren't too hard to find — it's based on this"* (2023-09-13,
re `8cWogE6dJoY`) · ▸ *"it uses cellular automata though, based on this implementation"* (2023-09-23) ·
▸ *"don't think so, but it's based on this algorithm"* (2026-02-13, asked whether he'd release a version
people could generate with themselves).

**The accretor rule**, from the post: a 4D table `Rule[state][faceCount][edgeCount][cornerCount]`, where
the 26 neighbours are **segmented into 6 faces / 12 edges / 8 corners and counted separately** instead of
lumped into one count. Filled randomly at **~20 % density** *"to avoid overly blobby structures"*. A cell
may only activate if it shares **at least one face** with an existing neighbour, keeping structures fully
connected. Seeded from a 5×5×5 random block at the centre; halts at the grid boundary.

○ **This explains his terrain generator's shape.** Its four rule families — `_cubeRule`, `_faceRule`,
`_edgeRule`, `_outerFaceRule` — *are* the accretor's neighbourhood segmentation, carried onto a
subdivision hierarchy instead of a growth front. And his `lambda` (0.35) is the accretor's ~20 %
rule-fill density under another name, with the identical purpose: **control what fraction of the rule
table is productive, because a dense table produces blobs.** That turns `lambda` from a magic number
into the one knob that matters.

○ The face-connectivity requirement also explains two asides that otherwise look unrelated: that his
generated worlds are *"3d printable"* (his word, 2023-05-25), and his long-running interest in
**structural integrity** — a CA that can only grow face-connected structures is already halfway to a
connectivity solver.
> — *"I found his source code really hard to understand, so at some point I cleaned it up and
> refactored everything"* (2023-08-13)
> ▸ **it tiles seamlessly**: *"his algorithm can be used for generating infinite terrains without
> seams between chunks … the idea was to skip over chunk edges (like slightly zooming out and then
> run the algo)"* (2023-08-13)

The pipeline he described 2026-02-13: *generate terrain grid → smoothen with a cave CA algorithm →
inject and propagate sunlight → inject and propagate skylight*. Vegetation and trees are further
CA passes, gated on the world state going quiet (dirty-rect), and trees grow over multiple ticks
from a stem value that encodes final height (2023-11-29, 2024-04-30).

○ Relevant to your `docs/voxel-automata-terrain.md`: the Bwerness sketch he keeps linking is the
same one, and he's read its GI section too — he singles out
[lines 176–189](https://bitbucket.org/BWerness/voxel-automata-terrain/src/master/ThreeState3dBitbucket.pde)
as *"another GI technique that I can't get out of my head — basically a super low approximation by
instead of shooting rays uniformly you only shoot them at fixed angles like 25 degree. the code
takes it even further though by shooting the GI rays upwards, but also propagate the result
downwards every step … like a cool mix of flood fill and ray tracing"* (2024-03-20).

---

## 9. Determinism and multiplayer — the whole plan

- **Verification by hashing:** ▸ *"I use it in my simulation engine to hash the world state on the
  GPU to detect if it stays deterministic"* (2023-10-23), via `atomicAdd` murmur — *"even millions
  of voxels each doing an atomicAdd was insanely fast"* (2023-12-30). His PRNG gists:
  [triple PRNG](https://gist.github.com/maierfelix/d25d674b8129a4cb39f734a9b25b2c39) (matching C/JS/GPU
  results) and an earlier [TEA](https://gist.github.com/maierfelix/ad8b40306e08ea705139cc49bc75e6d7).
  Float construction from a hash, the right way (2024-04-07):
  ```wgsl
  const IEEE_MANTISSA = 0x007FFFFFu;
  const IEEE_ONE = 0x3F800000u;
  fn CreateFloat(hash: u32) -> f32 {
    return bitcast<f32>((hash & IEEE_MANTISSA) | IEEE_ONE) - 1.0;
  }
  ```
- **Network model:** ▸ *"a GPU server that feeds new players the current world state … since the
  simulation is fully deterministic, only the player inputs have to be shared across the network"*
  (2023-10-23). Target ~1 000 players (2023-08-04). Client prediction idea: a small **32³ fake
  world volume** the player interacts with immediately, resynced against the real state every few
  frames.
- ▸ *"if you only partially load a world, you no longer have a fully deterministic simulation"*
  (2023-01-16) — which is *why* the worlds are finite.
- **Dirty buffers, with code.** He wrote out the whole pattern for someone (2024-11-07): a
  1-bit-per-chunk mask read at `srcPos / 8` with early return, `atomicAdd` on change, and a
  separate dirty-update shader running at 1/8 sim scale that caps the state at 2 and fades by 1
  per tick. Same message: *"btw if you want multiplayer support, only deal with integers in your
  sim — like instead of `randomFloat() > 0.5` do `randomInt() > 0xFFFFFFFF / 2`"*.
- **Checkerboard updating explained properly** (2023-05-25): *"the gaps between the checker tiles
  act as a barrier to prevent threads from ever being able to collide with each other … though this
  is only a problem if you do single buffered simulations. for double buffering, as long as you
  update in a source→destination fashion, you don't have these problems. the other way would be
  using atomics, but it's insanely slow."* Max travel distance per tick is set by the checker size
  (3–4 cells in his case), and a per-cell bit compared against the update cycle prevents
  double-updates.
- The 2D engine's split: ▸ *"the cell sim is single buffered and uses dirty + checkerboard
  updating and the flow sim is double buffered and uses only dirty"* (2023-05-25).

---

## 10. Platform, tooling, workflow

| Topic | Detail |
|---|---|
| Language split | ▸ *"10% typescript, 90% wgsl, 0% rust"* (2024-03-30). Earlier: TS + C + GLSL with a GLSL→WGSL converter; now a custom shader compiler with include support for both WGSL and GLSL, plus a hand-rolled WGSL preprocessor (2023-09-10). |
| Shader hot reload | node.js watcher on `shaders/` → `tint` CLI → `hot-shaders/` → app polls and recompiles the pipeline. Production build packages shaders; dev mode starts from the packaged set then hot-reloads (2025-01-06). ▸ *"it's such a blessing to have everything in shaders since everything can be hot reloaded instantly"* (2025-10-28). |
| Profiling | webgpu-inspector extension (*"basically what renderdoc does"*) + WebGPU timestamp queries; Nsight-on-Chrome works for WebGL only (2024-09-28). |
| Shipping | Shaders are **obfuscated inside the binary** — ▸ *"it's incredibly hard and time consuming to reverse them"* (2025-07-17). The public build has a ~1 min first-run shader-compile wait. |
| UI | HTML/CSS, which is a stated reason he stays in the browser: ▸ *"the major thing that kept me away from going native with my voxel engine is actually UI"* (2024-03-23). Inventory/position/pointed-block are read back GPU→CPU purely to render the UI (2024-07-01). Later moved toward `html-in-canvas`. |
| Buffer limits | ▸ `maxBufferSize` / `maxStorageBufferBindingSize` must be raised in `requestDevice` — he hit a Chrome regression that broke his 16384² 2D world overnight (2023-05-28). |
| Perf folklore | Copying with a shader beat `copyBuffer` by ~1.3× (2024-04-05); float textures were often slightly faster than uint (2024-04-05); ▸ *"only use atomics if the stuff you do is well parallelized, doesn't have too much branching, and doesn't depend on the atomic operation itself too much"* (2024-04-04). |

**His development discipline** — stated twice, and it's the most actionable thing in the export:

> ▸ *"it's hard to write good gpu code architecture, that's why I'm literally doing all new GPU
> features in a completely separated project from the main voxel engine — like in a limited fork of
> the voxel engine with barely any features where I can efficiently try out new things and
> prototype ideas. as soon as the prototype is almost 100% stable like with the entity system, and
> I have a full understanding of every part of it, I mostly can just copy and paste the code over
> in the main engine … because if your prototype code has any flaws in it, it's really really hard
> to get rid of them later"* (2024-04-02)

And on the engine-vs-game trap he's openly stuck in:
> ▸ *"at least he has half a game and isn't hard stuck on the engine part like me"* (2023-05-08)
> ▸ *"still stuck in engine hell"* (2024-02-22)
> ▸ *"engine only atm, I'm not good at designing games so that's something I'd hire"* (2025-07-16)
> ▸ *"depending on what your goal with a voxel engine is, you end up with a full-blown game engine
> at some point which is incredibly difficult and which is the reason why most voxel engines end up
> as 'just' tech demos"* (2025-10-18)

---

## 11. The 2D lab — where every technique is built first

Not a side hobby; it's his method. **CAGI, the fluid sim, the physics solver, flow-field
pathfinding, terrain gen and the interpolation trick were all built in 2D first.**

> ▸ *"2d in general is just a lot easier on all levels"*; *"in 2d you always see everything"*;
> *"I only port 2d prototypes over if I'm absolute sure that the main headaches … are completely
> solved"*

**Public, playable artifacts** (all WebGPU):

| URL | What |
|---|---|
| `voxelchain.app/pixels/?seed=3364760640` | The 2D falling-sand/circuit engine. 16384² capable (shipped at 2048² after the Chrome buffer-limit change); ~2–3 ms at 16384² on his 3080. Shadows are the **1-bit CA** technique. |
| `voxelchain.app/automata-explorer/` | The CA terrain generator, interactive. Lighting brute-forced at **512 spp in one frame** (2026-02-15). |
| `maierfelix.github.io/strata-voxel/` | Seed-encoded-in-URL CA structure explorer; iteration 7 for overview, 10–11 to find structures (2026-02-16/17). |
| `maierfelix.github.io/ds/` | 2D port of the terrain generator with infinite world size. |
| `github.com/VoxelChain/voxelchain-terrain-generator` | The terrain algorithm, open source. |

**The 2D circuit/material system** is worth knowing about because it's the ancestor of the WGSL
mod system. Cell behaviour was authored in TypeScript via a `MaterialCompiler` — `defineInput` /
`defineOutput` / `compileProgram` with density comparators and SWAP outputs — and **compiled to
truth tables** so executing a cell's behaviour is a lookup (full sand example posted 2023-05-17).
He later abandoned truth tables as too limited (2023-08-03) in favour of shader-authored behaviour,
which is exactly where the 2026 mod system landed.

**The 2D factory/base game** (Oct 2024 – Nov 2024, 8192² tested, GPU-driven, deterministic): bases
built around a "heart" core, movable by CA collision, flood-fill for ownership *and* reused for
pathfinding, conveyor belts that snap logically but interpolate visually, sawblade enemies,
tower-defence PvP, deliberately **no rotation**. ▸ *"what's pretty cool about this movement and
collision system is that it's not entity based, but cellular automata based"* (2024-03-15).

**Smooth movement from a snapped grid** — the trick, stated repeatedly and never in the other doc:
> ▸ *"in my voxel engine I use a texture centered around the camera that is used for animations and
> also smooth block movement … the interpolation is basically the current position, the destination
> and some delta based on how long the movement would take. the texture calculates this delta value,
> converts it into a shifting vector which is then applied during the ray voxel intersection"*
> (2024-11-18)
> ▸ why it matters: *"voxel snapping works on a small voxel scale, but has too much flickering when
> being close — probably also the reason why minecraft converts blocks affected by gravity into
> entities"*

---

## 12. Nameless — the other person worth reading in #showcase

You flagged him; he's the second-largest poster in #showcase (2 264 msgs there, 2 462 across all
three channels; a later `NamelessFractals` account adds 220). He is a **professional real-time
path-tracing engineer** — he says outright *"I am doing ReSTIR DI + GI for work"* (2023-05-17) and
*"at work I've implemented restir DI and GI and a radiance cache and honestly it's insane how fast
it converged with those algorithms — some scenes even 5k samples is not enough without these"*
(2024-06-15). He is x1m4's technical opposite: **everything he does is stochastic and ReSTIR-based,
where x1m4 abandoned exactly that.** Their arguments across 2023–2024 are the most useful thing in
the channel.

**Three projects:**
1. **A private Minecraft path-traced shader** (Iris, uses 3D textures) — ReSTIR GI + world-space
   radiance cache + SVGF, *"60 FPS on a rtx 4080"*, 130 with clouds off (2023-05-30).
2. **A real-time path-traced fractal renderer** — C# + OpenTK/OpenGL, own renderer, WPF GUI, 4
   fractals as SDFs, ReSTIR GI + radiance cache, *"50fps on a rtx 4080 at full resolution"*
   (2024-06-18). This is his main project from 2024 on.
3. **Commissioned fractal software** — ▸ *"these guys reached out to me to program a fractal
   software for them"* (2023-09-14), a UE5 plugin vendor.

**Techniques worth extracting from him:**
- **World-space radiance cache that bootstraps itself from screen space** (2023-07-18):
  ▸ *"in the primary ray, I write to the cache the position and normal. Then in a compute shader I
  check if the current cell has been initialized and I shoot a ray; if the cell that the ray's
  position hits hasn't been initialized, I write the position and normal to it, otherwise I add
  that cell's radiance to the current one — so progressively it fills up the cache"*. 1 bounce in
  the cache + 1 in the main path tracer. Known artifact: *"it's visible in the reflections"*.
- **Volumetrics from the same cache** (2023-07-19): for air cells, shoot one uniform-sphere ray,
  accumulate with last frame, then ray-march and sample. He later judged this *"less than
  favorable"* because the lighting lives in a low-res 3D texture, and moved to **path-traced volumes
  at 1 spp + ReSTIR** ([volumetric ReSTIR paper](https://dqlin.xyz/data/paper/volumetric_restir_supplemental.pdf)).
- **Emissive fog by 3D flood fill** — his own writeup (2024-03-05): *"every frame, if the block is
  emissive I set it in a 3d texture as the colour of the emission and the alpha as the strength. If
  the block is not emissive and not air, I set 0. If the block is empty I average the colour and
  strength from nearby blocks and subtract the averaged strength with some number. Finally I
  traverse the 3d texture with raymarching/dda and offset the position to remove the blocky look,
  `exp(-emissiveStrength)*emissiveColour`"*. ○ Note this is a flood-fill light volume — the two of
  them converge on the same primitive from opposite directions.
- **A genuinely clever leak fix** (2023-09-20): voxelize via the shadow pass into a 3D texture, then
  *"in a compute shader I only write to the cells that are empty and have at least 1 neighbour that
  is filled"* — surface-only probes.
- **Bayer-matrix amortisation**: 360° sky rendered at **1/16 of pixels per frame**, plus a
  ray-marched shadow map that replaced *four* shadow rays (visible, 2 bounces, radiance cache) with
  one texture read, which then made volumetrics cheap (2024-02-22).
- **Ray guiding for low-roughness specular** (2024-06-07): shoot a specular ray in a 1/16-size
  buffer; on a light hit store object id + triangle id + barycentrics and feed those into RIS in the
  ReSTIR DI pass, jittered by roughness. He then discovered this is just how it's supposed to be
  done. Shadertoy: `l3y3Ry`.
- **His ReSTIR practicalities**, given as advice: clamp `M`; reset the temporal reservoir on
  disocclusion; you can drop to one visibility ray or zero if AO substitutes; separate diffuse from
  specular (doubles rays); adding the radiance cache fixed his **boiling** (2024-06-18).
- **Clouds**, his own explanation (2024-06-07): fixed-step ray march, sample 3D noise (Worley +
  Perlin, precalculated as the industry does), and on `>0` accumulate density along a ray toward the
  sun; multi-scattering via *Oz the Great and Volumetric*. He moved from a flat cloud box to proper
  sphere intersection (2024-04-20).
- **He never got SVGF working**, and said so for a year: ▸ *"I am fucking sick of the svgf paper, I
  just can't seem to get it to work perfectly, mainly the variance calculations — either too blurry,
  or too noisy"* (2024-03-20). His proposed replacement was **CA-based shadow classification** feeding
  a shadow-aware denoiser — x1m4 talked him into looking at voxel-face-space denoising instead. He
  declared ▸ *"I think I finally nailed denoising"* on 2024-07-14.

○ **Why he's worth a doc of his own:** he is the counterfactual to x1m4. Same problems, opposite
method, both with numbers on the same class of hardware. If you ever want to know what atrium's
lighting would cost done stochastically instead of by propagation, his messages are the data.

---

## 13. Timeline correction and remaining gaps

Merging all three channels changes the picture from the first doc:

| Period | What was actually happening |
|---|---|
| 2022-07 → 2022-12 | VoxelChain (WebGL), sub-voxels, non-sparse octree, WebGPU-RT forks. Already benchmarking others' engines against his traversal. |
| 2023-01 | Joins **VoxRay Games** (5 people, with the Lobster author). |
| 2023-05 | Simulation engine C+wasm → **GPU compute**. 2D pixel engine + public demo. |
| 2023-06 → 2023-12 | Fluid/heat sim v1 with a collaborator; SH radiance-cache path tracer peaks; ReSTIR started; **ray-traced sound begins** (2023-12-31). |
| 2024-01 → 2024-04 | Sound queue lands; **CAGI replaces path tracing** (Feb); entity splatting + voxelizer; fluid v2 with velocity. |
| 2024-05 → 2024-11 | Items, conveyor belts, portals, sky model; 2D factory-PvP prototype; single-buffer conversion. |
| **2024-12 → 2025-09** | **Full-time NDA graphics job.** 2D physics solver and particle-life on the side. Engine work near-zero. |
| 2025-10 → 2026-02 | Fluid sim declared 100 % stable; CA terrain-gen demos published. |
| 2026-06 → 2026-08 | WESL mod system, per-type indirect dispatch shipped, fluid port to 3D beginning. |

**Still not covered by any export you have:**

1. **The voxelgamedev (VGD) server, keyword `cagi`.** Unchanged as the top priority — he says it
   three times, and this export adds the names of people who reimplemented it (`sweg`, `👾Rareș👾`,
   `Dapper Core`, `bob08022010`) plus the ones he taught directly (`bonisdev`, `KosmosisDire`).
2. **His own YouTube channel and the video comment threads** — he references his videos constantly
   (`f2_5RREfH-g`, `yf3ckx4O4sM`, `myVI8WlcvBg`, `BJgKkk7oLz0`, `kciz8Ab9c_c`, `w-pgOnpZefg`) as the
   canonical demos of each era. The videos themselves would date features better than anything here.
3. **The `#early-access` channel** he mentions creating for testers, and a deleted `#jobs` channel.
   Early-access builds and their feedback are not in these three channels.
4. **Attachments.** ~600 of his messages here carry images/videos, filenames only. The ones that
   would settle open questions: `wefdsfsdf.mp4` (2023-05-26, first CA shadows), `23rsdfdasf.mp4`
   (2024-02-19, first CAGI in 3D), `sdnjf89sdf.mp4` (2024-05-23, **ray-traced item sound**),
   `4tdfsgsdfg.mp4` (2024-07-06, 1-bit shadow CA), `1000000.mp4` (2024-12-05, 1 M-body physics),
   `wefaerfq23.mp4` (2025-10-14, stable fluid).
5. **The collaborator** on the fluid sim and the 2D pixel engine (`316239158584803328`) — two people
   built the mass sim, and his messages would double the detail.

---

## 14. What's newly worth stealing, beyond the first doc's list

1. **Denoise in voxel-face space, not screen space.** Project voxel faces back to screen space and
   filter across *faces*; make lighting constant per face. Cheap, stable on flat floors (which killed
   his earlier denoisers), and the pixel-art look is a free side effect.
2. **Propagate shadow, not light, when you only need occlusion.** 1 bit per cell, one neighbour
   lookup toward the light. Extend to N bits for penumbra. Costs almost nothing.
3. **A LOD for *lighting*.** Update 1/8 of the light volume per frame globally, 60 fps near the
   player. Same idea as shadow-map cascades, applied to a simulation.
4. **Precompute a per-face opacity map for any sub-grid representation** and use it during
   propagation. It's what makes a coarse light volume not leak through fine geometry.
5. **Apply gravity virtually.** Never write it into cell state, or you invalidate every dirty chunk
   every tick. Add it as a bias during the exchange step.
6. **Single-buffer + checkerboard beats double-buffer for anything that moves mass.** His measured
   verdict: 10× less code, 10× more readable, faster. Reserve double buffering for signal propagation.
7. **Sub-voxels are a storage optimization, not a performance one.** Tracing 8³ sub-voxels costs
   what tracing at 8× resolution costs. Budget accordingly.
8. **Entropy is the real constraint on voxel data.** If cells carry simulation state, compression and
   sparsity are both off the table — and that is a feature, not a failure.
9. **Prototype every GPU feature in a stripped fork, not in the engine.** Only port when you fully
   understand it, because GPU-driven flaws are near-impossible to remove later.
10. **For audio: freeze acoustic parameters at trigger time** if you can accept it. It turns a
    per-frame per-source solve into a one-shot trace, and his GPU/CPU queue caps (32 in, 64 playing)
    show where the real ceiling is — the CPU convolution, not the tracing.
