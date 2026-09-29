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

## P19 — Cloud taxonomy + wind field ✅
Four genera in CLOUD_TABLE (cumulus 7.0 / cumulonimbus 8.5 / nimbostratus
10.5 / cirrus 14.0) with per-kind radius, rain core, lightning gate, and
shadow depth; genus drawn per spawn with moisture-weighted odds. Cloud
motion reworked from fixed velocities to a deterministic wind field
(seeded meandering direction + breathing speed) with per-layer Ekman shear
(speed multipliers 0.85–2.3×, veer up to 0.7 rad); clouds spawn upwind,
integrate wind per tick (renderer interpolates pos − vel·(1−alpha)), exit
downwind. Puffy vs sheet meshes (7th instance stream); ⛈ now counts only
rain-bearers; "Cloud frequency" default 0.8 %. Regimes re-probed: savanna/
forest/grassland hold, lottery spectrum re-pinned (woodland seed 9).
100 engine + 22 web tests.

## P20 — Canopy race + root network ✅
Shade-avoidance: `tree_nbrs` (all-age trees at d1) feeds a per-tree
etiolation accumulator while young, frozen at maturity (form set in youth);
`Instance.slim` (36-byte instance now) stretches height ×(1+0.45·slim) and
narrows width ×(1−0.35·slim) in the vertex shader; husks carry the form.
Root network: pairwise nutrient diffusion (1/16 of the difference per
linked tree pair per tick), kin nursing (same-species mature neighbor →
seedling hazard ×0.6), root-competition drain (+⌊tree_nbrs/2⌋ per tick).
Regimes re-probed: all hold (forests a touch denser; fire-trap and
lottery bands widened). New tests: crowded vs open form + freeze, diffusion
rich→poor and isolation, kin > stranger > alone survival, and the
population statistic "forest trees ≫ slimmer than savanna trees".
104 engine + 22 web tests.

## P21 — Species differentiation + evolution ✅
Probe (`examples/species_probe.rs`) diagnosed: pine went extinct under
fire (no seed release on burn), acacia≈pine, oak couldn't colonize open land
yet monopolized forests, willow had no niche. Implemented from field
ecology: pine serotiny (burn-out of a mature pine seeds its ash disk);
water-table map (seeded gaussian basins; boosts tile water, damps fire);
willow wet-ground germination, flood tolerance (waterlogging kills other
species' seedlings/adults in saturated basins), root resprouting; oak
masting (weather-cued synchronized years) + jays; generalized long-distance
dispersal for all species; acacia nitrogen fixation (offsets own drain,
enriches ring, fixed-fertility establishment — no runaway); windthrow
(gust × etiolation). Heritable genome [vigor, hardiness] with
vigor-weighted fecundity (first cut applied vigor to the tile's blended
rain → vigor collapsed everywhere; individual fecundity fixed it).
Result: four-species niche partitioning in savanna and harsh climate,
willow basin refuge in forests, fire lottery now savanna↔woodland (no pure
collapse). Tried & reverted: pioneer gap bonus (broke savanna). Known
limit: pine/acacia still can't hold windthrow gaps in closed oak forest.
114 engine + 23 web tests.

## P22 — Terrain + grass functional types + biodiversity ✅
New `sim/terrain.rs`: one seeded 3-octave elevation field → catena
groundwater, aspect heat (surface normal vs sun), soil depth. Grass split
into four functional types (bunch C4 / sod C3 / sedge / annual) with
unimodal temperature × water niches, per-kind creep, shade, drought
sensitivity, flammability, fire resprouting, seedling competition; annuals
spread by a persistent seed bank. Trees gained thermal optima, a centered
deep-soil competitive hierarchy (oak ↑ deep, pine ↑ thin), and sapling
root-crown fire resprouting (oak, acacia). Probe-driven balancing
(`examples/niche_probe.rs`): first cut *lowered* diversity — heat was
compressed (gain 2.5 → 7.5), climate swings moved the niches wholesale
(fix: niche fit on site conditions with the swing damped to 35 %), sod
invaded bunch ground through shade, oak lacked any advantage (shade
tolerance 0.15 → 0.45). Legacy check caught 1−∏ grass pressure combination
breaking exact niches-off equivalence → additive. Result: effective types
2.2 (flat) → 4.6–4.9, all eight types in their textbook homes; heavy
lightning now leaves valley willow refuges instead of a seed lottery.
Render: hex-column terrain (2.5× vertical exaggeration), bedrock tint on
thin soils, per-kind grass meshes, ray-marched height-field picking,
relief-aware shadow box with terrain casting. HUD 🌿 diversity readout,
grass-kind brush picker, Flat plain / Fire savanna presets.
103 unit + 16 integration + 24 web tests.

## P23 — Encroachment penalties + distinct canopies + cloud optics ✅
User noticed oak spreading "randomly" and crowding others. Measured first
(`examples/oak_probe.rs`): long-distance seed bypassed every establishment
filter, but it was NOT the main cause — the moist forest was ~95 % oak even
with it off (oak regenerated under itself and outlived everything). Added:
shared `tree_establishment` site filter for local AND long-distance seed;
jay caching within 12 hexes; conspecific negative density dependence;
own-canopy shade intolerance (oak); pine needle litter (allelopathy);
sod thatch vs annuals; root water competition; oak-wilt-style pest
outbreaks spreading through same-species root grafts; deer browsing by
palatability with growth setback; harsher lean-year acorn predation.
First cut wiped oak out entirely (the penalties compound on a poorly
dispersed species — that's Janzen-Connell's point); oak was given its
real counter-traits: jays (long_distance 6), taproot drought tolerance,
acorn seed reserves that push through sod. Result: moist forest mixed
(oak leads late, wilt outbreaks cycle it; 1.5 → ~4.4 effective types),
default keeps all four species. Visuals: distinct canopy palette (khaki
acacia / leafy oak / blue-green pine / silver-sage willow, each aging
toward its own duller shade), bronzed thinning infested crowns, stunted
browsed saplings, wilting root-starved saplings, rust needle carpet,
mast-year acorn cast, 🐛 HUD count. Clouds: Beer–Lambert translucency
per genus with ragged edges and cirrus streaks in a blended pass.
Known: pine rarely survives the heavy-lightning preset now (willow
valley refuges carry it). 113 unit + 17 integration + 25 web tests.

## P24 — Configurable map size (default 16× area) ✅
`hex` constants → runtime `Grid` owned by the World (`Params.width/height`,
8–512 per side, applied on reseed; default 256×256). Tests and probes pin
`Grid::LEGACY` (64×64) — all 132 engine tests passed unchanged, confirming
exact legacy equivalence. Scaling work: storm rain/windthrow and cloud
shadows scan only a bounding box per cloud (were O(storms × tiles));
bridge builds instance streams once per frame instead of once per getter
(~14×); per-frame wall-clock tick budget (≥ 12 ms or 60 % of the frame)
with a measured achieved speed in the HUD; camera-following shadow
frustum; renderer `set_grid` for the soil slab; camera distance limits
widened. Cloud spawn ∝ linear size keeps cover/area (3.7 vs 5.0 per 4k
tiles at steady state). Presets no longer carry the map size. Known: the
tick is ~115 ns/tile spread evenly across passes — 32× on 256² is
CPU-bound; a profiler-guided pass (or a worker-parallel tick) is the next
lever.

## P25 — Climate zones, rivers & floods, grazing, bounded dispersal ✅
Probe first (`examples/area_probe.rs`): the big map gave insurance (no
extinctions) but not more diversity — the fixed-scale hills just tiled.
Added map-scale altitude (range ∝ map size) with a lapse rate, regional
rain, frost/treeline/montane soils; priority-flood drainage → river
channels, riparian corridors, flood pulses laying sediment; willow's
recruitment box, anoxia in saturated marsh, halved resprouting, riparian
browsing; grazing piospheres. Result (256², seed 7): γ 4.12 → 5.25, β
1.35 → 1.77, willow from a 3.1 % wet-ground blanket to a 1.0 % river band.
User then spotted pine/acacia exploding across the map: measured it
(`spread_probe`: 19 founders → 7,552 pines, front at the map edge) — the
uniform map-wide long-distance seed, sane at 64² (~640 m) but 2.5 km at
256². Replaced by per-species truncated 2Dt kernels from field dispersal
data plus a size-dependent fecundity ramp: 550 pines, front ~48 tiles by
t=4000. Oak: user suggested presets; `oak_presets` sweep (48 combos, all
cores) found oak recruitment-limited, held back by pests and deer, helped
by grazing → Oak woodland and Oak mosaic presets. Legacy tests re-pinned
(slower recruitment); new `tests/landscape.rs` (zonation, rivers, floods,
willow band, diversity, grazing, bounded spread). Snowline uses a damped
climate so cold years don't blanket the lowlands.
Known: oak stays scarce under default pressures; small maps sort acacia
by temperature rather than excluding it from their modest heights.

## P26 — Physiology from ../plant, inspector, roots view, cloud genera ✅
Ported the individual-plant mechanisms that matter at landscape scale:
per-tree carbon reserves (Liebig income × species thermal bell vs Q10
upkeep acclimated to the site climate), age-deepening roots with species
taproots (tree water includes only the groundwater the roots reach),
duration-based flood tolerance, climate-triggered pests (beetles on
starved pines, wet-year root rot on oaks), annual masting that exhausts
oaks, deciduous spring light, death-cause attribution (named hazard
components) with cause-tinted husks and a HUD tally. Calibration: first
cut starved everything (no respiration acclimation, adult-sized sapling
upkeep, mast cost ≈ the whole budget, crowd shading too strong); fixed
with acclimated Q10, sapling cost 0.6, mast 0.015, crowd light 0.05, and a
longer flat background lifetime once mechanistic deaths are modeled.
Result (128², seed 7): γ 4.10 → 4.58, oak 5 → 80 trees, all four species
thriving. UI: 🔍 inspector card, 🌱 roots view (glass ground + root
meshes), ☠ death line, 🍂 seasonal speed, heat-scaled sunlight/sky/sun
disc, edge-tree lean, four distinct cloud meshes (culled back faces,
depth-writing with fringe discard). Renderer takes one packed stream
buffer instead of 30 arguments.

## P27 — Dynamic clouds ✅
Clouds carry water (evaporation from wet ground/rivers, orographic lift;
losses to rain, lee descent, dry-air evaporation) and change genus:
cumulus → cumulonimbus (heating or forced ascent) → anvil cirrus;
cirrus → nimbostratus (front) → fair-weather cumulus; drying clouds
evaporate. In-place convective initiation and gust-front daughter cells;
orographic rain on windward slopes. Rain and lightning gated by stage.
Tuning via `examples/cloud_probe.rs`: first cut barely formed clouds
in place and put all rain in the hot lowlands (towering needed warm
ground); added forced-ascent towering and orographic rain; then multicell
chain reactions overcrowded the 256² sky (each storm ≈ one daughter) —
damped to static-comparable cloud counts and rain totals. Render morphs
between genus meshes (opacity cross-fade + altitude glide), fades drying
clouds; HUD sky census + weather ticker. Legacy map: dynamics off.

## P28 — Weather visuals, biomes, new species ✅
Sim: climate gradients (latitude cooling, coast-to-interior drying,
scaled with map size), five biome plants (spruce, birch, creosote,
reeds, cactus — gated by `biomes`, legacy seed pools preserved),
suitability-weighted founders (without it the new specialists never met
their climates: spruce 0, acacia 512 → 58), snowfall → snowpack →
meltwater and melt freshets, pyrocumulus, strike and in-cloud flashes,
Whittaker biome classification. Render: rain/snow shafts with virga (own
pipeline, animated streaks), branching bolts, real-time flash (decays in
wall-clock time, rate-limited, edge-triggered — the first cut strobed the
sky white at 32× and even while paused), wind sway, heat shimmer, storm
light, conditions-dependent haze, valley fog, silver linings, wet gloss
and drought cracks (new Instance field), smoke plumes, cap clouds, biome
overlay; stream layout now derived from species/grass counts. A missing
vertex attribute (the new gloss) panicked the renderer at startup — now
guarded by a native shader↔layout test. UI: 7 trees / 6 grasses,
🗺 Biomes overlay + legend + Whittaker chart, regional presets.
Result (256², seed 7): γ 4.92 → 5.90 with biomes; deserts ~9% of the
map with cactus and creosote, reed marshes, boreal spruce–pine–birch.
Known: no rainbows — physically impossible with the fixed 56° sun.

## Ideas / not done
- A moving sun (day/season elevation) — would allow rainbows and long
  evening shadows
- True speciation: reproductive isolation between diverged lineages (currently traits blend freely)
- Pioneer persistence in closed forests (gap dynamics beyond windthrow)
- Herbivores (browsers) for acacia's thorn/ant defenses
- Deploy workflow (GitHub Pages) like traffic's `.github/workflows/deploy.yml`
- Touch pinch-zoom gesture; hover tile tooltip
- Population sparkline; age histogram overlay
- Share params via URL blob like traffic's `?c=`
