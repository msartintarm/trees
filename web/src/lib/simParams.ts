// The ecology tunables as plain data: defaults matching the engine's
// `Params::default()`, UI-side clamping (the engine sanitizes again), and the
// field specs the panel renders from. Probabilities travel as fractions
// (0..1) but display as percentages.

export type SimParams = {
  grassSeedP: number;
  grassClonalP: number;
  shadeStrength: number;
  treeGrowthP: number;
  treeRange: number;
  treeMaturityAge: number;
  sodFactor: number;
  crowdingP: number;
  grassMeanLife: number;
  treeMeanLife: number;
  fireIgnitionP: number;
  fireSpreadP: number;
  nutrientBoost: number;
  stormRate: number;
  stormLightningP: number;
  climateSwing: number;
  waterTable: number;
  mutationRate: number;
  terrain: number;
  grassNiches: number;
  pestStrength: number;
  browse: number;
  competition: number;
  climateZones: number;
  rivers: number;
  grazing: number;
  physiology: number;
  seasons: number;
  cloudDynamics: number;
  biomes: number;
  seedTreeP: number;
  seedGrassP: number;
  /** Map size in tiles per side (applies on Reseed). */
  width: number;
  height: number;
};

export const DEFAULT_PARAMS: SimParams = {
  grassSeedP: 0.0,
  grassClonalP: 0.08,
  shadeStrength: 1.0,
  treeGrowthP: 0.002,
  treeRange: 3,
  treeMaturityAge: 40,
  sodFactor: 0.4,
  crowdingP: 0.002,
  grassMeanLife: 30,
  treeMeanLife: 250,
  fireIgnitionP: 0.0,
  fireSpreadP: 0.85,
  nutrientBoost: 1.0,
  stormRate: 0.008,
  stormLightningP: 0.06,
  climateSwing: 0.7,
  waterTable: 1.0,
  mutationRate: 0.03,
  terrain: 1.0,
  grassNiches: 1.0,
  pestStrength: 1.0,
  browse: 1.0,
  competition: 1.0,
  climateZones: 1.0,
  rivers: 1.0,
  grazing: 1.0,
  physiology: 1.0,
  seasons: 1.0,
  cloudDynamics: 1.0,
  biomes: 1.0,
  seedTreeP: 0.0,
  seedGrassP: 0.0,
  width: 256,
  height: 256,
};

/// Named configurations surfaced in the panel. Populations quoted in `hint`
/// are long-run means measured by the engine's example probes
/// (`equilibrium`, `niche_probe`); `engine/tests/ecology.rs` pins each
/// regime with band assertions.
export type Preset = { key: string; label: string; hint: string; params: SimParams };

export const PRESETS: Preset[] = [
  {
    key: "defaults",
    label: "Savanna parkland",
    hint: "climate zones from lowland acacia & bunchgrass savanna up through pine to alpine meadow, willows along the rivers (~4.3–5 effective types; oak scarce)",
    params: DEFAULT_PARAMS,
  },
  {
    key: "oak-woodland",
    label: "Oak woodland",
    hint: "where oak thrives most (~18% of the map, ~⅔ of the trees): fast recruitment, few oak-wilt outbreaks and deer, grazers keeping the sod open for acorns (wood pasture)",
    params: { ...DEFAULT_PARAMS, treeGrowthP: 0.01, pestStrength: 0.3, browse: 0.3, competition: 0.5 },
  },
  {
    key: "oak-mosaic",
    label: "Oak mosaic",
    hint: "oak strong (~7% of the map, ~40% of the trees) alongside everything else — the most diverse mix measured (~4.7 effective types)",
    params: { ...DEFAULT_PARAMS, treeGrowthP: 0.005, pestStrength: 0.3, browse: 0.3, competition: 0.5 },
  },
  {
    key: "coast-desert",
    label: "Coast to desert",
    hint: "the climate gradients rule: wet coastal forest grades through savanna and grassland to a cactus-and-creosote desert inland, spruce and birch in the cold north (best on 256²+)",
    params: { ...DEFAULT_PARAMS, climateZones: 0.4 },
  },
  {
    key: "mountain-island",
    label: "Mountain island",
    hint: "altitude rules: savanna foothills, pine and spruce belts, alpine tundra and snowy peaks, rivers radiating to the shore",
    params: { ...DEFAULT_PARAMS, biomes: 0.4 },
  },
  {
    key: "river-delta",
    label: "River delta",
    hint: "low, flat and wet: channels, reed beds and willows, frequent floods laying fresh sediment",
    params: { ...DEFAULT_PARAMS, terrain: 0.4, climateZones: 0.2, waterTable: 1.0, stormRate: 0.012 },
  },
  {
    key: "flat-plain",
    label: "Flat plain",
    hint: "no relief, rivers, climate zones, grazers, or grass types: nothing to sort by (~2.5 effective types)",
    params: { ...DEFAULT_PARAMS, terrain: 0, grassNiches: 0, climateZones: 0, rivers: 0, grazing: 0, physiology: 0, seasons: 0, cloudDynamics: 0, biomes: 0 },
  },
  {
    key: "moist-forest",
    label: "Moist forest",
    hint: "mixed forest (~30% tree cover): oak leads late but oak wilt and self-shading keep pine & willow in the gaps",
    params: { ...DEFAULT_PARAMS, treeGrowthP: 0.01 },
  },
  {
    key: "fire-grassland",
    label: "Fire-swept grassland",
    hint: "the sward surges in wet years and burns in droughts; trees extinct",
    params: { ...DEFAULT_PARAMS, treeGrowthP: 0.001, fireIgnitionP: 0.0005 },
  },
  {
    key: "fire-savanna",
    label: "Fire savanna",
    hint: "burns sweep the uplands into bunchgrass & annuals; willows survive in the wet valleys",
    params: { ...DEFAULT_PARAMS, fireIgnitionP: 0.0005 },
  },
];

/** Map size is independent of the ecology: presets neither set nor match
 * it (`matchingPreset` returns the preset matching every other field, or
 * null for a custom mix). */
const MAP_KEYS: ReadonlySet<keyof SimParams> = new Set(["width", "height"]);

export function matchingPreset(params: SimParams): string | null {
  const same = (a: SimParams, b: SimParams) =>
    (Object.keys(DEFAULT_PARAMS) as (keyof SimParams)[]).every((k) => MAP_KEYS.has(k) || a[k] === b[k]);
  return PRESETS.find((p) => same(p.params, params))?.key ?? null;
}

/** A preset's ecology applied onto the current params, keeping the map size. */
export function applyPreset(preset: Preset, current: SimParams): SimParams {
  return { ...preset.params, width: current.width, height: current.height };
}

export type ParamField = {
  key: keyof SimParams;
  label: string;
  /** percent: stored as 0..1, displayed as 0..100. int: stored as-is. */
  kind: "percent" | "int";
  min: number; // in display units
  max: number;
  step: number;
  /** Seeding fields only take effect on the next reseed. */
  appliesOnReseed?: boolean;
};

export const PARAM_FIELDS: ParamField[] = [
  { key: "grassSeedP", label: "Grass seed %/tick", kind: "percent", min: 0, max: 100, step: 0.1 },
  { key: "grassClonalP", label: "Grass creep %/neighbor", kind: "percent", min: 0, max: 100, step: 0.5 },
  { key: "shadeStrength", label: "Canopy shade %", kind: "percent", min: 0, max: 100, step: 5 },
  { key: "treeGrowthP", label: "Tree growth %/tick", kind: "percent", min: 0, max: 100, step: 0.1 },
  { key: "treeRange", label: "Seed-rain range", kind: "int", min: 1, max: 8, step: 1 },
  { key: "treeMaturityAge", label: "Tree maturity (ticks)", kind: "int", min: 0, max: 10000, step: 5 },
  { key: "sodFactor", label: "Sod factor %", kind: "percent", min: 0, max: 500, step: 5 },
  { key: "crowdingP", label: "Crowding death %/nbr", kind: "percent", min: 0, max: 100, step: 0.1 },
  { key: "grassMeanLife", label: "Grass mean life (ticks)", kind: "int", min: 1, max: 100000, step: 1 },
  { key: "treeMeanLife", label: "Tree mean life (ticks)", kind: "int", min: 1, max: 100000, step: 5 },
  { key: "fireIgnitionP", label: "Lightning %/grass tile", kind: "percent", min: 0, max: 100, step: 0.001 },
  { key: "fireSpreadP", label: "Fire spread %", kind: "percent", min: 0, max: 100, step: 5 },
  { key: "nutrientBoost", label: "Nutrient boost %", kind: "percent", min: 0, max: 500, step: 10 },
  { key: "stormRate", label: "Cloud frequency %/tick", kind: "percent", min: 0, max: 100, step: 0.05 },
  { key: "stormLightningP", label: "Storm lightning %/tick", kind: "percent", min: 0, max: 100, step: 1 },
  { key: "climateSwing", label: "Climate swings %", kind: "percent", min: 0, max: 100, step: 5 },
  { key: "waterTable", label: "Water table %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "mutationRate", label: "Mutation %/birth", kind: "percent", min: 0, max: 20, step: 0.5 },
  { key: "terrain", label: "Terrain relief %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "grassNiches", label: "Grass niches %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "pestStrength", label: "Pest outbreaks %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "browse", label: "Deer browsing %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "competition", label: "Neighbor competition %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "climateZones", label: "Climate zones %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "rivers", label: "Rivers & floods %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "grazing", label: "Grazing %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "physiology", label: "Tree physiology %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "seasons", label: "Seasons %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "cloudDynamics", label: "Dynamic clouds %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "biomes", label: "Biomes (gradients + biome plants) %", kind: "percent", min: 0, max: 100, step: 10 },
  { key: "seedTreeP", label: "Seed trees %", kind: "percent", min: 0, max: 100, step: 0.5, appliesOnReseed: true },
  { key: "seedGrassP", label: "Seed grass %", kind: "percent", min: 0, max: 100, step: 0.5, appliesOnReseed: true },
  { key: "width", label: "Map width (tiles)", kind: "int", min: 8, max: 512, step: 8, appliesOnReseed: true },
  { key: "height", label: "Map height (tiles)", kind: "int", min: 8, max: 512, step: 8, appliesOnReseed: true },
];

function clampField(field: ParamField, display: number): number {
  if (!Number.isFinite(display)) {
    const d = DEFAULT_PARAMS[field.key];
    return field.kind === "percent" ? d * 100 : d;
  }
  const v = Math.min(field.max, Math.max(field.min, display));
  return field.kind === "int" ? Math.round(v) : v;
}

/** Stored fraction/int → the number shown in the input. */
export function displayValue(field: ParamField, params: SimParams): number {
  const v = params[field.key];
  // Round percents to a fine precision so 0.003 shows as 0.3, not 0.3000001.
  return field.kind === "percent" ? Math.round(v * 1e6) / 1e4 : v;
}

/** Apply an edited input value (display units), clamped, back onto params. */
export function withFieldValue(field: ParamField, params: SimParams, display: number): SimParams {
  const v = clampField(field, display);
  return { ...params, [field.key]: field.kind === "percent" ? v / 100 : v };
}

export function clampParams(p: SimParams): SimParams {
  return PARAM_FIELDS.reduce(
    (acc, f) => withFieldValue(f, acc, displayValue(f, p)),
    { ...DEFAULT_PARAMS, ...p },
  );
}
