# Plan

Build a hex-grid ecology sim mirroring `../traffic`'s architecture: pure Rust
sim core → wasm bridge → wgpu 3D renderer → Next.js worker/OffscreenCanvas
shell. Status as of 2026-09-26.

## P0 — Scaffold ✅
git init, Cargo.toml (cdylib+rlib, unused/dead_code deny), module wiring,
.gitignore, docs.

## P1 — Sim core ✅
`rng.rs`/`clock.rs` copied from traffic; `hex.rs` (axial math, disk tables,
cube-round picking); `world.rs` (proximity bitfield → death pass → growth
pass; uniform lifespans at birth; paint; seeding). 30 native tests including
statistical bands (grass rate ∈ [3.5, 4.5] %, tree rate ∈ [0.7, 1.3] % on
eligible cells) and same-seed determinism over 1000 ticks.

## P2 — Render math ✅
Orbit camera (f64, WebGPU clip space) with `pick_ground` inverse verified
against the forward projection; baked meshes with per-vertex color/weight for
multi-material instancing; `Instance` (32 B, layout pinned by test);
`scene.wgsl` naga-validated; prev/cur scale growth animation.

## P3 — Bridge + Renderer ✅
`Simulation` (advance with 64-tick catch-up ceiling, camera controls,
`paint_at` = ray → hex → world.paint, copy-out instance buffers); `gpu.rs`
one-pipeline renderer with depth buffer and growable instance buffers.
Gotcha found: `wgpu::Instance::new` locks to WebGPU whenever `navigator.gpu`
exists, even with no adapter — switched to
`util::new_instance_with_webgpu_detection` so WebGL2 fallback engages.

## P4 — Web shell ✅
Static-export Next.js app: protocol/camera/hud (11 node --test cases),
engineSession/session/worker per traffic's split, EngineCanvas with
orbit/pan/zoom/paint gestures, imperative HUD, controls panel (play/pause,
step, speeds, brush, seed+reseed, reset view). Verified headless: WebGL path
renders, 5 paint clicks → exactly +5 trees; WebGPU path verified up to
adapter/device/loop (headless swiftshader cannot present WebGPU frames — raw
red-clear probe confirmed environmental).

## P5 — Polish ✅
Sprout minimum scale for instant paint feedback; base soil slab under tile
seams; `npm run build` static export green.

## P6 — Tunables panel ✅
`Params` struct in `world.rs` (sanitized engine-side, `tree_range` disk
rebuilt on change), `set_params` bridge call, `params` Control, `simParams.ts`
(defaults + clamping + field specs, node-tested), Parameters `<details>`
section in the panel with per-field inputs and a Defaults reset. Lifespan
changes apply to plants born after the change; seeding %s apply on Reseed
(reseed keeps tunables). Verified headless: growth %s at 0 → extinction by
tick ~1300; grass at 50 % → 3,963/4,096 tiles grass.

## P7 — Integration tests + equilibrium search ✅
`examples/equilibrium.rs` probe swept 12 configs × 3 seeds over 12k ticks:
defaults are a stable tree monoculture (trees ~2940, grass → 0–4); range
affects colonization speed, not the balance point; coexistence needs slower
tree pressure (tree_p ≤ ~0.1 %, or shorter tree life / longer grass life at
0.25 %). `tests/ecology.rs` (11 tests, ~1.3 s with `[profile.dev]
opt-level = 1`) pins those regimes plus constraints: p=1.0 saturates exactly
the range disk in one tick, zero growth → fully bare after max life, absurd
params ≡ sanitized params, reproducibility from seed+params+click history,
and mid-run param shifts moving the world between regimes.

## P8 — Coexistence presets in the UI ✅
`PRESETS` + `matchingPreset` in `simParams.ts` (node-tested: clamp-stable,
unique, detectable; coexistence presets provably gentler on tree pressure),
Preset dropdown atop the Parameters section with the measured long-run
populations as a hint line; flips to Custom on any manual edit. Verified
headless: selecting "Slow trees" landed the live sim at trees 798 / grass 522
vs the probe's predicted ~820 / ~520.

## P9 — Realism overhaul (proximity features from real ecology) ✅
Replaced the flat-radius rules with: mature-tree seed-rain kernel (J-C dip at
d1, peak d2, exponential tail, multi-source compounding capped at 4),
maturity age gating seed/shade/crowding/fire-immunity, a light gate on BOTH
understories (gap-phase regeneration — this was the key to breaking the
closed-forest absorbing state), clonal grass creep + spontaneous seed rate,
sod competition factor, crowding self-thinning, and fire (lightning ignition,
d1 spread through grass + saplings, mature trees as firebreaks, 🔥 brush,
ember/scorch rendering, 🔥 HUD count). Params grew to 14 fields.
Re-swept: defaults = robust savanna (tree 0.3 %, no lightning; grass ~435 /
trees ~725 on 5 seeds); fire creates true bistability (forest or grassland by
seed) and a fire trap against late maturity; closed forests resist burning
(hysteresis — fire presets want a Reseed). 67 engine tests + 20 web tests,
integration suite pins all four regimes plus the fire trap.

## P10 — Gradual death transitions ✅
No more pop-out removals: (1) senescence — lifespans are known at birth, so
plants wilt (scale → 62 %, color → straw/grey-brown) across their last 20 %
of life and old-age deaths arrive telegraphed; (2) standing dead — every
death records visual-only `Remains` (kind, charred flag, age & wilt at
death) and renders as a husk starting at exactly the size the plant last
drew, shrinking to zero over 25 (grass) / 50 (tree) ticks via the existing
prev/current shader lerp; (3) ash — burn-outs stain the ground for 60 ticks,
fading ember → ash-grey → soil. Also fixed per user report: the 🔥 brush now
torches ANY plant including mature trees (bark only resists *spreading*
fire). Instance counts in the bridge now come from the built frame since
husks pad the streams. All visual-only — equilibria, protocol, and regime
tests unchanged. 71 engine tests.

## P11 — Mycelium decomposition + regrowth block ✅
Fixed decay timers replaced by saprotrophic mycelium: per-husk colonization
grows each tick (+2 per established husk neighbor — inoculum proximity),
rot rate scales with it, charcoal colonizes 4× slower, mushrooms (new 4th
instance stream/mesh) fruit on established wood. Husks now BLOCK regrowth
until fully rotted — the layer's one sim effect, negative-feedback only —
fixing the sprout-through-snag artifact. Fire brush now torches mature
trees (bark only resists spread). Re-swept: savanna leaner (grass ~285 /
trees ~633, still 5-seed robust); charcoal firebreaks make 1 % forests
fireproof, so the grassland preset now pairs slow trees (0.1 %) with heavy
lightning, and the lottery (defaults + lightning 0.05 %) splits
grassland-vs-savanna by seed; the fire trap (maturity 120 → extinct under
fire that maturity 40 survives) re-verified. 76 engine tests.

## P12 — Nutrient cycling + soil visualization ✅
Per-tile fertility store (cap 1000): completed decomposition deposits (tree
800, thatch 150, charred ¼ — charcoal stays locked), fire mineralizes an
immediate ash flush (tree 250 / grass 100), occupied tiles drain 4/tick,
idle soil leaches 1/tick. Establishment × `1 + nutrient_boost·fertility`
(new 15th Param, default 1.0, clamp 0–5). Ground color lerps to dark loam
with fertility (ash fades to reveal the flush). Sweep: bounded and monotone
— boost 0 exactly reproduces prior regimes, savanna shifts to grass ~246 /
trees ~705 (5-seed robust), and the ash flush makes the fire regimes MORE
decisive: grassland preset now extinct-on-all-5-seeds, lottery still cleanly
bistable. 80 engine tests + 20 web.

## P13 — Player-seeded worlds (creep-only grass) ✅
Per user direction: grass_seed_p, seed_tree_p, seed_grass_p all default 0 —
the world boots empty and every founding plant is painted by hand; grass
spreads only by creeping. That made the old defaults extinction-prone
(grass min 0 on all seeds), so the ecology was retuned and re-swept:
clonal 8 % + tree 0.2 % gives a robust creep-only savanna (grass ~690 /
trees ~480, 5 seeds). Consequences embraced: creeping grass cannot survive
under a closed canopy (moist forest → grass extinct) or recolonize after
local wipeout — extinction is permanent without replanting. Lottery preset
rebuilt at tree 0.5 % + lightning 0.05 % (forest 3/5 seeds, grassland 2/5);
fire trap re-verified. Regime tests use a `planted()` proxy (2 %/10 %
scatter) for the user's brushwork. 81 engine + 21 web tests.

## P14 — Thunderclouds ✅
Storm entities spawn off-map (deterministic draws: heading, skew, speed
0.2–0.4, radius 6–10), drift linearly across, and dissipate after exiting.
Rain core (75 % of radius) soaks tiles for 30 ticks — wet fuel can't ignite
or catch spread, burning tiles are doused (a parked storm even re-douses a
player's torch), wet soil grows ×1.5; lightning rolls per storm per tick
and strikes anywhere under the cloud, so dry edge strikes escape the rain.
Two new Params (storm_rate 0.3 %, storm_lightning_p 6 %, → 17 total).
Rendering: cloud + bolt meshes (6 instance streams), clouds interpolate
drift via `pos(tick+alpha)` (linear paths, no stored prev), soft ground
shadows and cool wet tint, ⛈ HUD badge. Sweep: savanna unchanged-to-
stabilized under default storms (grass ~695 / trees ~479, 5 seeds); all
other regimes hold. 88 engine + 22 web tests.

## P15 — Climate: sun, rain, and hazard mortality ✅
Global deterministic climate (two incommensurate sinusoids per signal,
seeded phases, `climate_swing` param) drives everything: growth × water ×
sun, fire spread ×(0.4+1.2·dry) and ignition ×(2·dry)², storm spawn
×(0.2+1.6·moisture) — all exactly 1 at neutral (0.5/0.5), so lab tests run
with swing 0. Lifespans replaced by per-tick hazard `stress/mean_life`
(stress = 0.4+1.2·drought): lifetimes are unbounded geometrics — pinned by
a test showing trees outliving the old 500 cap while >6-mean-life elders
stay rare and the mean tracks the param. Renders as global light level
(shader `Globals.light`), sky dimming, drought browning/wilt of all
vegetation, ☀/💧 in the HUD. Params 18 (mean-life renames + climate_swing).
Sweep: savanna breathes seasonally (grass ~638 [273..1210], trees ~487,
5 seeds); grassland cycles with the seasons; lottery becomes a spectrum of
fates (grassland / embattled savanna / woodland by seed); fire trap
re-verified under climate. 88 engine + 22 web tests.

## P16 — Tree species (acacia / oak / pine / willow) ✅
Species as a curated archetype table of multipliers on the global tree
params: growth, maturity, mean life, shade tolerance, ash affinity
(serotiny), drought-sensitivity exponent, water-affinity exponent,
fireproofing age, crowding, dispersal, rot, and nutrient return. Seed rain
is per species (offspring breed true); contested tiles roll one combined
establishment draw, then pick the winner proportionally. Balancing journey
worth remembering: oak's shade tolerance re-opened the closed-canopy
absorbing state (2,900-tree monocultures) — fixed with the real costs of a
climax tree: heavy seeds (dispersal 2), space hunger (4× crowding), a thin
shade edge (0.15), drought sensitivity 1.25, and a trimmed lifetime edge
(2.2×). Result: default savanna = 4-species pioneer mix with grass never
extinct; moist forest = true succession (pioneers → oak canopy by t≈9000,
pinned by the flagship integration test); fire regimes exclude oak
(pioneers + serotinous pines own the burn zones). Four silhouettes (flat-top
acacia, broad oak, pine spire, willow curtain) as separate instanced
streams; husks keep their species' shape; species picker in the panel.
96 engine + 22 web tests.

## P17 — Lighting: shadow map, procedural bump, hemisphere ✅
scene.wgsl rewritten: hemisphere ambient + warm directional sun gated by a
2048² depth-only shadow pass from an orthographic sun matrix
(`camera::light_view_proj`, corner-coverage unit test); hardware-PCF
comparison sampler; vegetation casts, clouds keep analytic soft shadows.
Procedural bump = world-space value-noise normal perturbation (no UVs).
Surface format now prefers non-sRGB (sRGB targets were re-encoding authored
colors brighter on WebGPU than WebGL — the "grass too bright" report).
Grass palette darkened ~30%. Ray tracing: not available in browsers; shadow
mapping delivers the visible part. 84 unit + 13 integration engine tests.

## P18 — Rounder trees: tessellation, specular, rim, bark bump ✅
Cones/cylinders tessellated up (canopies 8→16–18 segments, trunks 6→10–12);
camera eye added to Globals; Blinn-Phong sheen (shininess 26) + fresnel rim
— the curvature cue for low-poly solids; bump split into two domains
blended by n.z: ground-plane clod noise on up-facing surfaces, anisotropic
bark grain (high frequency around the axis, low along it) on steep ones,
strongest on mat_w=1 trunk/stem vertices. WGSL uniformity lesson from the
user's browser: never branch around textureSampleCompare — sample
unconditionally (above-frustum geometry compares lit via negative ref).

## Ideas / not done
- Deploy workflow (GitHub Pages) like traffic's `.github/workflows/deploy.yml`
- Touch pinch-zoom gesture; hover tile tooltip
- Population sparkline; age histogram overlay
- Share params via URL blob like traffic's `?c=`
