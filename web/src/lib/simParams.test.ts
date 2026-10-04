import test from "node:test";
import assert from "node:assert/strict";

import {
  clampParams,
  displayValue,
  applyPreset,
  matchingPreset,
  withFieldValue,
  DEFAULT_PARAMS,
  PARAM_FIELDS,
  PRESETS,
} from "./simParams.ts";

const field = (key: string) => {
  const f = PARAM_FIELDS.find((f) => f.key === key);
  assert.ok(f, `no field spec for ${key}`);
  return f;
};

test("defaults display as the spec percentages", () => {
  assert.equal(displayValue(field("grassSeedP"), DEFAULT_PARAMS), 0, "no spontaneous seed: grass only creeps");
  assert.equal(displayValue(field("grassClonalP"), DEFAULT_PARAMS), 8);
  assert.equal(displayValue(field("treeGrowthP"), DEFAULT_PARAMS), 0.2);
  assert.equal(displayValue(field("shadeStrength"), DEFAULT_PARAMS), 100);
  assert.equal(displayValue(field("sodFactor"), DEFAULT_PARAMS), 40);
  assert.equal(displayValue(field("fireIgnitionP"), DEFAULT_PARAMS), 0);
  assert.equal(displayValue(field("treeRange"), DEFAULT_PARAMS), 3);
  assert.equal(displayValue(field("treeMaturityAge"), DEFAULT_PARAMS), 40);
});

test("percent edits round-trip through the stored fraction", () => {
  const p = withFieldValue(field("grassSeedP"), DEFAULT_PARAMS, 7.5);
  assert.equal(p.grassSeedP, 0.075);
  assert.equal(displayValue(field("grassSeedP"), p), 7.5);
  // Tiny fire percentages survive the round-trip too.
  const f = withFieldValue(field("fireIgnitionP"), DEFAULT_PARAMS, 0.002);
  assert.equal(displayValue(field("fireIgnitionP"), f), 0.002);
});

test("edits clamp to the field bounds", () => {
  assert.equal(withFieldValue(field("treeGrowthP"), DEFAULT_PARAMS, 250).treeGrowthP, 1);
  assert.equal(withFieldValue(field("treeRange"), DEFAULT_PARAMS, 0).treeRange, 1);
  assert.equal(withFieldValue(field("treeRange"), DEFAULT_PARAMS, 5.7).treeRange, 6);
  assert.equal(withFieldValue(field("sodFactor"), DEFAULT_PARAMS, 900).sodFactor, 5);
  assert.equal(withFieldValue(field("grassMeanLife"), DEFAULT_PARAMS, -3).grassMeanLife, 1);
});

test("non-finite input falls back to the default", () => {
  assert.equal(withFieldValue(field("treeMeanLife"), DEFAULT_PARAMS, NaN).treeMeanLife, 250);
});

test("clampParams fills gaps and normalizes every field", () => {
  const p = clampParams({ ...DEFAULT_PARAMS, treeRange: 99, grassSeedP: 9 });
  assert.equal(p.treeRange, 8);
  assert.equal(p.grassSeedP, 1);
  assert.equal(p.treeMeanLife, 250);
});

test("presets are clamp-stable, uniquely keyed, and detectable", () => {
  const keys = PRESETS.map((p) => p.key);
  assert.equal(new Set(keys).size, keys.length, "duplicate preset keys");
  assert.equal(PRESETS[0].key, "defaults");
  assert.deepEqual(PRESETS[0].params, DEFAULT_PARAMS);
  for (const p of PRESETS) {
    assert.deepEqual(clampParams(p.params), p.params, `preset ${p.key} not clamp-stable`);
    assert.equal(matchingPreset(p.params), p.key);
  }
  assert.equal(matchingPreset({ ...DEFAULT_PARAMS, treeRange: 5 }), null, "custom mix must not match");
});

test("the world starts in the player's hands", () => {
  assert.equal(DEFAULT_PARAMS.seedTreeP, 0, "no initial trees — the user plants them");
  assert.equal(DEFAULT_PARAMS.seedGrassP, 0, "no initial grass — the user plants it");
  for (const p of PRESETS) {
    assert.equal(p.params.seedTreeP, 0, `${p.key} must not auto-seed trees`);
    assert.equal(p.params.seedGrassP, 0, `${p.key} must not auto-seed grass`);
    assert.equal(p.params.grassSeedP, 0, `${p.key} grass spreads only by creeping`);
    assert.equal(p.params.autoPlant, 0, `${p.key} must not auto-plant`);
  }
  assert.equal(DEFAULT_PARAMS.autoPlant, 0, "auto-planting starts off");
});

test("the regime presets differ along the measured axes", () => {
  const by = Object.fromEntries(PRESETS.map((p) => [p.key, p.params]));
  assert.equal(by["defaults"].fireIgnitionP, 0, "default savanna has no lightning");
  assert.ok(by["moist-forest"].treeGrowthP > by["defaults"].treeGrowthP, "forest grows trees faster");
  assert.equal(by["moist-forest"].fireIgnitionP, 0);
  assert.ok(by["fire-grassland"].treeGrowthP < by["fire-savanna"].treeGrowthP, "grassland trees can't outgrow the flames");
  assert.ok(by["fire-grassland"].fireIgnitionP > 0);
  assert.ok(by["fire-savanna"].fireIgnitionP > 0);
  assert.equal(by["defaults"].terrain, 1, "the default world has relief");
  assert.equal(by["flat-plain"].terrain, 0, "the flat plain is the no-niche baseline");
  assert.equal(by["flat-plain"].grassNiches, 0);
  assert.equal(by["flat-plain"].rivers, 0);
  assert.equal(by["flat-plain"].climateZones, 0);
});

test("the oak presets relax what the sweep found limits oak", () => {
  const by = Object.fromEntries(PRESETS.map((p) => [p.key, p.params]));
  for (const key of ["oak-woodland", "oak-mosaic"]) {
    const p = by[key];
    assert.ok(p.treeGrowthP > by["defaults"].treeGrowthP, `${key}: oak is recruitment-limited`);
    assert.ok(p.pestStrength < 1 && p.browse < 1, `${key}: fewer oak-wilt outbreaks and deer`);
    assert.ok(p.grazing > 0, `${key}: grazers keep the sod open (wood pasture)`);
  }
  assert.ok(by["oak-woodland"].treeGrowthP > by["oak-mosaic"].treeGrowthP, "the woodland recruits hardest");
});

test("presets keep the map size and still match on a custom-sized map", () => {
  const big = { ...DEFAULT_PARAMS, width: 384, height: 128 };
  const forest = PRESETS.find((p) => p.key === "moist-forest")!;
  const applied = applyPreset(forest, big);
  assert.equal(applied.width, 384);
  assert.equal(applied.height, 128);
  assert.equal(applied.treeGrowthP, forest.params.treeGrowthP);
  assert.equal(matchingPreset(applied), "moist-forest");
  assert.equal(matchingPreset(big), "defaults");
  // Auto-planting is a player tool: it survives presets and doesn't break
  // preset matching.
  const planting = { ...DEFAULT_PARAMS, autoPlant: 2.5 };
  assert.equal(applyPreset(forest, planting).autoPlant, 2.5);
  assert.equal(matchingPreset(planting), "defaults");
});

test("the auto-plant rate keeps its decimals and clamps", () => {
  const f = field("autoPlant");
  assert.equal(withFieldValue(f, DEFAULT_PARAMS, 0.25).autoPlant, 0.25);
  assert.equal(withFieldValue(f, DEFAULT_PARAMS, -3).autoPlant, 0);
  assert.equal(withFieldValue(f, DEFAULT_PARAMS, 500).autoPlant, 100);
});

test("the regional presets lean on the landscape features they showcase", () => {
  const by = Object.fromEntries(PRESETS.map((p) => [p.key, p.params]));
  assert.ok(by["coast-desert"].biomes > by["coast-desert"].climateZones, "gradients over mountains");
  assert.ok(by["mountain-island"].climateZones > by["mountain-island"].biomes, "mountains over gradients");
  assert.ok(by["river-delta"].terrain < 1 && by["river-delta"].rivers === 1, "flat and wet");
  assert.equal(by["flat-plain"].biomes, 0);
});

test("every SimParams key has exactly one field spec", () => {
  const keys = PARAM_FIELDS.map((f) => f.key).sort();
  assert.deepEqual(keys, Object.keys(DEFAULT_PARAMS).sort());
});
