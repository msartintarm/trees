# Tree Simulator

A hex-grid forest ecology simulation in the browser: a pure Rust sim core
compiled to WebAssembly, rendered in 3D with wgpu (WebGPU, WebGL2 fallback),
driven by a Next.js static-export shell. The architecture mirrors the sibling
`../traffic` project: everything numeric is a pure, natively-tested module;
everything browser-shaped is a thin gated layer around it.

## The rules (design in one screen)

The ecology models real grass–tree spatial dynamics; every mechanism below
is a `Params` field, live-editable in the panel:

| Rule | Mechanism |
| --- | --- |
| Grid | pointy-top hexes (odd-r offset) raised into terrain columns; **256×256 by default** (65,536 tiles), any size 8–512 per side via *Map width / height* in the panel (applies on Reseed; the camera re-frames). Rates are per tile, so densities and regimes are size-independent; cloud spawning scales with the map's linear size so cloud cover per area holds. The shadow map follows the camera, so shadows stay sharp on any map |
| Time scale | vegetation runs at ≈ one year per tick (lifespans of centuries, maturity in decades); oak mast years are annual events that can't come back to back. Weather is stylized: each cloud crossing stands for a season's storm track |
| Tree physiology | `physiology` param. Every tree carries a **carbon reserve**: income = light × a species **thermal bell** × water (Liebig), upkeep = respiration climbing with heat above the species' optimum (**Q10 = 2**, acclimated to the site's climate), plus pest load and oak mast crops. An emptied reserve kills (starvation) — the slow multi-year decline after droughts, deep shade, or heat. **Roots deepen with age** toward each species' taproot (acacia, oak deep; willow shallow), so old trees tap the groundwater while saplings live on rain. **Flood tolerance is by duration** (willow ≫ oak > pine, acacia). **Climate-triggered pests**: bark beetles swarm carbon-starved pines, root rots hit oaks in wet years. Starving crowns thin and pale |
| Death causes | every tree death is attributed to the hazard component that contributed most — old age, drought, starvation, frost, root rot, pests, crowding, fire, windthrow, browsed, flood scour — tallied in the HUD (☠ line) and stamped on the husk: beetle-killed pines go rust red ("red attack"), drought snags bleach, frost kills brown, drowned wood darkens |
| Tile inspector | the 🔍 Inspect tool shows every factor on a clicked tile: altitude, temperature (°C), soil and groundwater, soil depth, fertility, understory light, and for a tree its carbon reserve, thermal fit, root reach, water, waterlogging, pests and genome; bare ground names the best-suited tree; husks their cause of death |
| Roots view | 🌱 Roots turns the ground to glass and draws each tree's root system beneath it — taproot depth from its age and species, blue where it reaches groundwater, brown where it lives on rain — with the groundwater glowing blue under the soil |
| Seasons | `seasons` param: deciduous canopies (oak, willow; acacia partly) are bare through spring, so the understory beneath them gets light that evergreen pine never lets through. At the 🍂 speed (0.05×, a year every two seconds) the scene shows the year: spring flush, russet oaks and golden willows in autumn, bare twigs and a lower snowline in winter, straw-colored dormant grass |
| Heat & light | heat (from the climate's sun signal) makes the sunlight stronger, warmer and more glaring, hazes and pales the sky, and swells the visible sun disc from mild white to a large gold glow |
| Phototropism | edge trees lean out toward open light, away from crowding neighbors |
| Biomes | `biomes` param: map-scale climate gradients — a colder north and a drier continental interior away from a seed-chosen coast, scaled with map size like the mountains — plus biome plants: **spruce** (boreal climax: cold-hardy, shade-tolerant, shallow-rooted, fire-sensitive), **birch** (boreal pioneer: fast, deciduous, wind-seeded), **creosote** (desert shrub: drought-proof, deep-rooted, frost-tender), **reeds** (marsh beds), **cactus** (desert floor). Founders are drawn from the regional pool by site suitability. Tiles classify into Whittaker biomes (wetland, tundra, boreal forest, temperate forest, grassland, savanna, desert) — the 🗺 Biomes overlay tints them, with a legend, shares, and a live temperature × water chart; the inspector names each tile's biome. On 256²: ~5.9 effective types vs ~4.9 without biomes |
| Weather visuals | rain curtains under raining clouds (dark narrow shafts under thunderheads, broad grey veils under rain sheets), **snow** falling on cold ground and building a **snowpack** that melts into wet ground and spring floods, **virga** streaks that evaporate before the ground under drying clouds and cirrus; branching lightning whose flash is a **local** light around the strike (the ground, trees and storm cloud nearby — the sky never flashes), a subtle in-cloud glow, and a ⚡ Flashes toggle to turn all flashing off; **wind sway** of grass and crowns with gusts; **valley fog** after wet, cool, still spells; **cap clouds** on moist windward peaks; **distance haze** that is clear after rain, milky and warm in heat and drought, grey under overcast; **storm light** (overcast dims and cools the sun, greys the sky); **silver linings** on backlit cloud edges; rain-slick **wet sheen** and puddles; **drought cracks**; **heat shimmer**; **smoke plumes** over fires and **pyrocumulus** fire clouds |
| Dynamic clouds | `cloud_dynamics` param. Every cloud carries water: it gains from evaporation off moist ground, groundwater and rivers and from being forced up windward slopes, and loses it by raining, sinking down lee slopes, and (small cumulus) evaporating over hot dry ground. Clouds **form** in place (cumulus bubbling up over warm, moist or windward ground; daughter cells at a thunderhead's gust front), **turn into each other** — cumulus towers into cumulonimbus (surface heating or forced ascent), a spent thunderhead collapses leaving its anvil as cirrus, cirrus thickens and lowers into nimbostratus as a front arrives, spent nimbostratus breaks up into fair-weather cumulus — and **disappear** as they dry out. Rain and lightning follow the stage (a thunderhead rains and strikes only once grown; the rain shaft shrinks as the cloud rains out), and moist clouds forced up a ridge rain on the windward side and arrive dry in the lee (a rain shadow). Transitions morph visibly (old form thinning as the new grows in, gliding between altitudes); the HUD shows the sky by genus and a ticker announces each transition |
| Cloud genera | each genus has its real shape: **cumulus** as fields of small flat-based cauliflower heaps; **cumulonimbus** as a dark-based tower with a flat anvil streaming downwind; **nimbostratus** as a broad dark slab with ragged scud beneath; **cirrus** as long fibrous hooked streaks along the high wind — shaded with bright tops over grey bases |
| Climate zones | map-scale mountains (`climate_zones`) whose altitude range grows with the map (gentle on 64², full on 256²+): temperature falls with altitude (lapse rate), a regional rain field (wetter on the mountains), frost limits per species (acacia tender … pine hardy), a treeline, thinner montane soils, and a snowline that creeps down in cold years. Result on 256²: warm lowland acacia & bunchgrass savanna → pine belt → alpine sod & sedge meadow |
| Rivers & floods | a drainage network from priority-flood routing (Barnes et al. 2014): tiles draining ≥ 350 upstream tiles become open-water channels (nothing roots, fire can't cross) with wet riparian corridors; wet seasons flood the floodplains, scour grass and seedlings, and leave sediment bars. Willow follows the real **recruitment box** — it recruits only on bare, freshly flooded sediment — suffocates in permanently saturated marsh (sedge takes it), resprouts less, and is browsed by riparian herbivores: a band along the rivers instead of a blanket |
| Grazing | herbivores crop palatable grass (sod ≫ annuals > sedge, bunchgrass), heaviest near water (piospheres); grazed swards render short. Keeps sod from blanketing the map and holds the sod mat open for big seeds (wood pasture) |
| Dispersal limits | long-distance seed follows a truncated fat-tailed 2Dt kernel per species (Clark et al. 1999; tile ≈ 10 m): pine ~50 m typical / 600 m cap, oak (jays) ~80 m / 400 m, acacia (ungulates) ~60 m / 400 m, willow ~100 m / 800 m. Seed output grows with tree size (15 % of a full crop at maturity → 100 % at 3× maturity age), so invasions advance as fronts instead of exploding across the map |
| Terrain | one seeded fractal elevation field (`sim/terrain.rs`) with everything derived from it: **catena** groundwater (valley bottoms saturate, ridges drain), **aspect heat** (sun-facing slopes hot and dry, shaded slopes cool), and **soil depth** (thin and rocky on ridges and steep slopes, deep in the bottoms). Rendered with a 2.5× vertical exaggeration; clicks pick by ray-marching the tile tops. `terrain` param = 0 gives the legacy flat world |
| Grass functional types | four kinds (`world.rs::GRASS_TABLE`) with **unimodal** niche responses to site temperature × water (niche fit uses the terrain with only 35 % of the climate swing, so the map, not the weather, decides who lives where): **bunchgrass** (C4, hot dry slopes, drought-tolerant, flammable, resprouts after fire, gappy tussocks that let seedlings through), **sod grass** (C3, cool slopes, shade-tolerant, browns in drought, rhizome mat that blocks seedlings), **sedge** (saturated valley bottoms, flood-tolerant), **annuals** (short-lived colonizers that build a persistent soil **seed bank** and flush on burns and drought gaps; perennials overgrow them on undisturbed ground). Paint any kind with the grass picker. `grass_niches` = 0 gives one generic grass |
| Tree niches | trees fit a thermal optimum (acacia hot, oak cool, pine and willow broad) and a **soil competitive hierarchy**: demanding species (oak) win deep soil, stress-tolerators (pine, acacia) keep the thin ridges. Oak and acacia saplings **resprout from the root crown** after fire (pine relies on serotiny) |
| Neighbor competition | grounded penalties on encroachment (`competition` param): **conspecific negative density dependence** (Janzen-Connell / plant–soil feedback — seedlings do worse among adults of their own kind), **own-canopy shade intolerance** (oak seedlings fail beneath oaks: the oak regeneration problem), **pine needle litter** (acidic allelopathic carpet suppressing other seedlings and grass, drawn as rust-brown ground), **sod thatch** smothering annuals, and **root water competition** (established roots drain dry soil around saplings, which visibly wilt). Big-seeded oak and acacia push through sod on their seed reserves |
| Pests | specialist outbreaks (`pest_strength`): oak wilt spreads tree-to-tree through **root grafts** between same-species neighbors, new outbreaks start in dense same-species stands, the load builds until the host dies; resistant species (acacia) often recover. Infested crowns bronze and thin; 🐛 in the HUD counts them. Dense monocultures thin themselves into gaps that pioneers refill |
| Browsing | deer (`browse`) eat saplings by palatability (oak, willow ≫ thorny acacia, resinous pine): some browsing kills, the rest sets growth back — stunted saplings stay in reach longer (the browse trap) and render smaller |
| Long-distance seed | jays cache acorns within 12 hexes of the parent; wind seed goes map-wide — and every far-flung seed faces the full site filter (niche, soil, shade, competition, mast-year predation), so seed landing in another species' habitat rarely takes. Lean-year acorn crops are mostly eaten (predator satiation drives masting) |
| Biodiversity | 🌿 in the HUD: effective number of plant types, e^Shannon over the 4 tree species + 4 grass kinds (grass counts as one type when niches are off). Flat plain ≈ 2.2, default landscape ≈ 4.6–4.9 |
| Tree seed rain | each **mature** tree deposits a distance kernel over its range disk — Janzen-Connell dip at d1, peak at d2, exponential tail; sources compound as `p = 1 − (1−p₁)^W`, capped at `SEED_RAIN_CAP`. Seed rain is per species: offspring inherit their parent's kind |
| Species | four varieties on distinct strategy corners (`world.rs::SPECIES_TABLE`, curated multipliers — not panel knobs). **Acacia**: baseline pioneer, drought-hardy. **Oak**: climax — slow, late-maturing, long-lived, shade-tolerant seedlings, early fireproof bark, heavy acorns (dispersal 2), space-hungry (4× crowding), slow-rotting nutrient-rich wood. **Pine**: fast, flammable past maturity, but 3× recruitment on ash (serotiny). **Willow**: booms on wet ground (water exponent 2), dies hard in drought, fast and short-lived |
| Maturity | trees younger than 40 ticks don't seed, shade, crowd, or resist fire |
| Light gate | canopy proximity scales **both** grass growth and tree recruitment (gap-phase regeneration); full block at d1 of a mature tree by default |
| Grass | creep-only by default: no spontaneous seeding, 8 %/tick per adjacent grass tile — swards advance as fronts from what you plant, and a locally extinct sward stays gone |
| Sod competition | tree establishment on grass ×0.4 (>1 = nurse-plant mode) |
| Self-thinning | >2 mature neighbors → extra 0.2 %/tick death per excess neighbor |
| Canopy race | shade-avoidance: every tree (any age) shades its ring; a young tree accrues etiolation while hemmed in, frozen at maturity — so forest-grown trees are permanently up to 45 % taller and 35 % narrower, open-grown ones short and broad. Husks keep the form |
| Water table | the terrain's catena groundwater (see Terrain): tops up effective water for growth, damps fire spread, shows as damp cool soil. `water_table` param scales it |
| Species behaviors (from field ecology) | **pine serotiny** — a mature pine that burns releases its cone bank onto the ash around it; **oak masting** — synchronized boom years (every ~2–5 years, weather-cued) with lean years in between, plus **jay caching** of acorns map-wide; **willow** — seed needs wet ground (basins or fresh rain), roots tolerate flooding (other species drown in saturated basins), dying mature willows **resprout from the root**; **acacia nitrogen fixation** — offsets its own soil draw, enriches its ring, and gives it an edge on poor soil; **long-distance dispersal** for every species (jays, winged and cottony seed, animal-carried pods); **windthrow** — storm gusts topple mature trees, tall thin forest-grown ones first |
| Evolution | every tree carries heritable [vigor, hardiness]: vigor trades fecundity (more seed, individually) for a shorter life; hardiness flattens drought stress both ways. Offspring take the vigor-weighted parental mean of the arriving seed plus a small mutation (`mutation_rate`). Selection is real but weak against drift — hardiness rises under harsh climates; lineages show as canopy tints (bright = vigorous, glaucous = hardy) |
| Root network | adjacent trees are linked (root grafts / mycorrhizae): soil nutrients diffuse from rich to poor tiles along tree–tree pairs each tick; mature trees nurse same-species seedlings beside them (hazard ×0.6); overlapping root plates drain shared soil faster |
| Fire | grass is fuel: ignitions spread tile-to-tile through contiguous grass and saplings; mature trees survive and act as firebreaks. Background lightning off by default; the 🔥 brush, storm bolts, and fire presets supply ignition |
| Climate | two deterministic signals — ☀ sun and 💧 moisture — drift through wet years and droughts (shown in the HUD; `climate_swing` scales the amplitude). They multiply growth, mortality, flammability, and storm frequency, all calibrated to exactly 1 at the neutral climate. Droughts brown and wilt the whole map, dim the light, quadruple ignition; wet years flush green and brew storms |
| Mortality | a per-tick hazard (`weather stress / mean_life`), not a lifespan: lifetimes are unbounded geometrics, so a tree CAN live forever — but survival decays exponentially and in equilibrium very few grow old. Droughts are when most of the dying happens |
| Cloud optics | clouds are translucent by **Beer–Lambert** transmittance e^(−τ): cirrus τ≈0.7 (a see-through fibrous veil), cumulus τ≈4 with ragged thinning edges, nimbostratus τ≈6, cumulonimbus τ≈14 (opaque). Optical depth tapers to a noise-broken rim; a separate alpha-blended pass draws them over the scene |
| Weather | four cloud genera at stacked altitudes (`world.rs::CLOUD_TABLE`), spawned by the weather (moist air brews the rain-bearers): **cumulus** (low, fair-weather, dappled shadow), **cumulonimbus** (the thunderhead: rain core + lightning — dry edge strikes can outrun its own rain), **nimbostratus** (wide grey sheet, soaks ~its whole footprint, no lightning), **cirrus** (high, fast, nearly shadowless). ⛈ counts the rain-bearers |
| Wind | a seeded, slowly-meandering surface wind drives all cloud motion, with Ekman-style shear per layer: higher layers move faster and veer further (cirrus ~2.3× speed, 0.7 rad off the surface wind — low scud and high wisps visibly cross). Clouds spawn upwind, integrate the wind each tick, and exit downwind |
| Genesis | the world starts **empty** — you paint the founding trees and grass with the brushes (runs replay exactly from seed + params + click history) |
| Nutrient cycle | decomposition returns biomass to a per-tile fertility store (rotted tree ≫ thatch; charcoal keeps most carbon locked) and fire mineralizes an immediate ash flush; fertility multiplies establishment by up to `1 + nutrient_boost`, living plants draw the store down, idle soil leaches back. Rendered as the soil darkening toward loam |
| Death & decay | old age is telegraphed (plants wilt and shrink past 80 % of their lifespan); every death leaves a standing husk — grey snag, straw thatch, or charcoal — that **blocks regrowth** until saprotrophic mycelium finishes it. Colonization builds faster beside other colonized wood (inoculum proximity), mushrooms fruit on established wood, and charcoal resists rot ~4×, so burn snags linger and old burn scars break up fuel. Ash stains the ground for 60 ticks (visual only) |
| Clock | 10 ticks/s at 1×; speeds 0.5–32×, pause, single-step |

Runs stay reproducible from seed + params + click history (stateless
counter-based RNG). Note the hysteresis: fire presets need a Reseed to show
their regime — an established closed forest has too little contiguous fuel
to burn down, which is faithful to the real thing.

All randomness is a stateless SplitMix64 hash of `(seed, cell, tick, stream)`
(`sim/rng.rs`), so the same seed replays the same forest, and there is no RNG
state to save.

## Layout

```
engine/                 Rust crate → wasm (cdylib + rlib)
  src/sim/              pure core, cargo-testable natively
    rng.rs              counter-based stateless randomness
    clock.rs            fixed-dt play/pause/speed clock
    hex.rs              axial hex math, disks, picking, world bounds
    world.rs            cell states + the per-tick ecology rules
  src/render/           pure render math + WGSL + wasm-only GPU
    camera.rs           orbit perspective camera, ground-plane ray picking
    geometry.rs         baked meshes: tile prism, tree, grass tuft, base slab
    scene.rs            world → per-frame instance streams (prev/cur scale)
    scene.wgsl          the one pipeline (naga-validated under cargo test)
    gpu.rs              wgpu Renderer (wasm32 only; WebGPU or WebGL2)
  src/bridge.rs         wasm-bindgen Simulation — marshalling only
web/                    Next.js 15 static export
  scripts/build-wasm.mjs  wasm-pack driver with content-hash skip cache
  src/lib/              pure tested modules (node --test, no DOM/wasm)
    protocol.ts         typed main↔worker Control/FromWorker contract
    simParams.ts        ecology tunables: defaults, clamping, panel field specs
    camera.ts           backing-store sizing + gesture→control mappings
    hud.ts              snapshot → HUD strings
    engineSession.ts    wasm load + rAF loop + applyControl switch
    session.ts          worker(OffscreenCanvas)-or-inline façade
  src/worker/engineWorker.ts  thin relay around engineSession
  src/components/EngineCanvas.tsx  canvas, input, panel, HUD
```

Rendering/lighting: hemisphere ambient (cool sky above, warm bounce below) +
a directional sun gated by a 2048² shadow map rendered from the sun's view
(vegetation casts real shadows; clouds keep their soft analytic ones);
Blinn-Phong specular sheen and a fresnel rim light (the cue that makes the
16–18-segment cones read as smooth solids); and dual-domain procedural bump —
world-space value-noise normal perturbation, soil clods on up-facing
surfaces and anisotropic bark grain (stretched along the trunk axis) on
steep ones — i.e. bump/normal mapping without UVs or textures. The surface
prefers a non-sRGB format so authored colors aren't re-encoded brighter on
WebGPU than on WebGL. True ray tracing isn't available in browsers; the
shadow map covers its main visible benefit here.

Per-frame data crosses the wasm boundary as copied-out byte buffers
(`bytemuck`-cast `Instance` arrays); smooth growth comes from shipping the
previous and current scale and lerping in the shader by the clock's alpha —
the same model as traffic, no shared memory views.

## Run the tests (the fast inner loop)

```
cd engine && cargo test        # unit + whole-world integration tests, native
cd web && npm test             # protocol/camera/hud/simParams via node --test
```

`engine/tests/ecology.rs` asserts long-run *regimes* against measured bands:
the default savanna coexists on every seed, faster trees close into moist
forest, strong lightning leaves willow refuges in the wet valleys, every
plant type holds its own corner of the landscape, relief + grass niches add
more than one effective type over the flat plain, the fire trap selects against
late maturity, and a clonal-free grassland matches the analytic birth–death
balance (~485 grass tiles once husk-blocked turnover is accounted for). The
bands come from the probe:

```
cargo run --release --example equilibrium      # sweep configs, print stats
cargo run --release --example species_probe    # per-species niches, solo vs mixed
cargo run --release --example evolution_probe  # selection on heritable traits
cargo run --release --example niche_probe      # where each type lives + diversity, flat vs terrain
cargo run --release --example oak_probe        # tree composition per regime, each competition mechanism knocked out
cargo run --release --example scale_probe      # tick/frame cost and densities at 64² … 512²
cargo run --release --example landscape_probe -- 256 7 8000   # zonation, rivers, willow, α/β/γ diversity
cargo run --release --example spread_probe -- 256 4000        # invasion front from one founding stand
cargo run --release --example oak_presets -- 6000             # where oak thrives (sweep, all cores)
cargo run --release --example cloud_probe -- 256 3000         # cloud genera, lifecycle events, rain by altitude
cargo run --release --example biome_probe -- 256 7 6000       # biome coverage and who lives where

Tests and probes run on the original 64×64 calibration map
(`Params::legacy_map()`, `Grid::LEGACY`): all regime bands were measured
there, and nothing in the ecology depends on map size.
```

Measured regimes (12k-tick runs, multiple seeds) — selectable in the panel's
Preset dropdown (`simParams.ts::PRESETS`; reads Custom once you hand-edit):

| Preset | Config | Long-run |
| --- | --- | --- |
| Savanna parkland (default) | tree growth 0.2 %, clonal 8 %, no base lightning, full relief + grass niches | ~4.8 effective types: pine on thin ridges, oak on deep mid-slopes, acacia on sunny slopes, willow + sedge in the valleys, sod on the shady slopes, bunchgrass and annuals on the hot open ground; grass ≈ 1,000–1,500, trees ≈ 550–900 |
| Oak woodland | tree growth 1 %, pests 30 %, browsing 30 %, competition 50 %, grazing on | where oak thrives most: ~18 % of the map, ~⅔ of the trees (wood pasture) |
| Oak mosaic | tree growth 0.5 %, pests 30 %, browsing 30 %, competition 50 %, grazing on | oak strong (~7 %, ~40 % of trees) inside the most diverse mix measured (~4.7 effective types) |
| Coast to desert | biomes on, gentle mountains | wet coastal forest grading through savanna and grassland to a cactus-and-creosote desert inland; spruce and birch in the cold north (best on 256²+) |
| Mountain island | mountains on, gentle gradients | altitude rules: savanna foothills, pine and spruce belts, alpine tundra, snowy peaks |
| River delta | flat, wet, stormy | channels, reed beds and willows, frequent floods |
| Flat plain | terrain, grass niches, climate zones, rivers, grazing all off | nothing to sort by, ~2.5 effective types |
| Moist forest | tree growth 1 %, no base lightning | a **mixed forest** (~1,200 trees, ~4.4 effective types): oak leads late, but its seedlings fail beneath its own canopy and oak-wilt outbreaks sweep dense stands, so pine and willow hold the gaps (without neighbor competition it collapses to ~95 % oak) |
| Fire-swept grassland | tree growth 0.1 %, lightning 0.05 % | the sward surges in wet years and burns in droughts; trees extinct |
| Fire savanna | defaults + lightning 0.05 % | burns sweep the uplands into bunchgrass and annuals; the wet valleys don't carry fire, so riparian willow stands survive on every seed |

Regimes are measured from a "planted" scatter (2 % trees, 10 % grass — the
probe's stand-in for your brushwork); presets themselves start empty.

An emergent worth knowing: charcoal's rot resistance makes old burn scars
into firebreaks, so a fast-growing (1 %) forest is effectively fireproof —
fires fragment their own future fuel. Converting forest to grassland takes
*slow* trees plus frequent lightning, which is why the fire presets pair
them.

## Run it in the browser

```
cd web
npm install
npm run dev                    # builds the wasm first (cached by content hash)
```

Open http://localhost:3000 — click to plant, drag to orbit, shift/right-drag
to pan, wheel to zoom. `npm run build` produces the static export in `web/out`.


**Performance on big maps.** A tick costs ~115 ns per tile natively (≈7 ms
at 256², ≈30 ms at 512²; wasm somewhat more). Each frame gets a wall-clock
tick budget, so at high speeds a big map slows down gracefully instead of
freezing — the HUD then shows the achieved speed, e.g. `▶ 32× (≈6×)`.
Instance streams are built once per frame (`prepare_frame`).
## Deliberate differences from traffic

No threads build or COOP/COEP shim, no serde/`import` feature, no GPU compute,
no Canvas-2D fallback renderer (wgpu's WebGL2 backend covers non-WebGPU
browsers, chosen via `new_instance_with_webgpu_detection` because a present
`navigator.gpu` does not guarantee a usable adapter), no ASCII view, no
map/tools pipeline.
