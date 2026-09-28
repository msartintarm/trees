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
  seedTreeP: number;
  seedGrassP: number;
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
  stormRate: 0.003,
  stormLightningP: 0.06,
  climateSwing: 0.7,
  seedTreeP: 0.0,
  seedGrassP: 0.0,
};

/// Named configurations surfaced in the panel. Populations quoted in `hint`
/// are long-run means measured by `engine/examples/equilibrium.rs` (5 seeds
/// for the default, 3 for the rest, 12k ticks); `engine/tests/ecology.rs`
/// pins each regime with band assertions.
export type Preset = { key: string; label: string; hint: string; params: SimParams };

export const PRESETS: Preset[] = [
  {
    key: "defaults",
    label: "Savanna parkland",
    hint: "grass ≈ 630 with a mixed pioneer woodland (pine, acacia, oak groves, willow)",
    params: DEFAULT_PARAMS,
  },
  {
    key: "moist-forest",
    label: "Moist forest",
    hint: "succession: pioneers first, then oaks close a dense canopy (~1,900)",
    params: { ...DEFAULT_PARAMS, treeGrowthP: 0.01 },
  },
  {
    key: "fire-grassland",
    label: "Fire-swept grassland",
    hint: "the sward surges in wet years and burns in droughts; trees extinct",
    params: { ...DEFAULT_PARAMS, treeGrowthP: 0.001, fireIgnitionP: 0.0005 },
  },
  {
    key: "fire-lottery",
    label: "Fire lottery",
    hint: "fire keeps the oaks out: grassland or a pioneer woodland, by seed",
    params: { ...DEFAULT_PARAMS, fireIgnitionP: 0.0005 },
  },
];

/** Key of the preset matching `params` exactly, or null for a custom mix. */
export function matchingPreset(params: SimParams): string | null {
  const same = (a: SimParams, b: SimParams) =>
    (Object.keys(DEFAULT_PARAMS) as (keyof SimParams)[]).every((k) => a[k] === b[k]);
  return PRESETS.find((p) => same(p.params, params))?.key ?? null;
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
  { key: "stormRate", label: "Storm frequency %/tick", kind: "percent", min: 0, max: 100, step: 0.05 },
  { key: "stormLightningP", label: "Storm lightning %/tick", kind: "percent", min: 0, max: 100, step: 1 },
  { key: "climateSwing", label: "Climate swings %", kind: "percent", min: 0, max: 100, step: 5 },
  { key: "seedTreeP", label: "Seed trees %", kind: "percent", min: 0, max: 100, step: 0.5, appliesOnReseed: true },
  { key: "seedGrassP", label: "Seed grass %", kind: "percent", min: 0, max: 100, step: 0.5, appliesOnReseed: true },
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
