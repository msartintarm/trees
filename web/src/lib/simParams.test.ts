import test from "node:test";
import assert from "node:assert/strict";

import {
  clampParams,
  displayValue,
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
  }
});

test("the regime presets differ along the measured axes", () => {
  const by = Object.fromEntries(PRESETS.map((p) => [p.key, p.params]));
  assert.equal(by["defaults"].fireIgnitionP, 0, "default savanna has no lightning");
  assert.ok(by["moist-forest"].treeGrowthP > by["defaults"].treeGrowthP, "forest grows trees faster");
  assert.equal(by["moist-forest"].fireIgnitionP, 0);
  assert.ok(by["fire-grassland"].treeGrowthP < by["fire-lottery"].treeGrowthP, "grassland trees can't outgrow the flames");
  assert.ok(by["fire-grassland"].fireIgnitionP > 0);
  assert.ok(by["fire-lottery"].fireIgnitionP > 0);
});

test("every SimParams key has exactly one field spec", () => {
  const keys = PARAM_FIELDS.map((f) => f.key).sort();
  assert.deepEqual(keys, Object.keys(DEFAULT_PARAMS).sort());
});
