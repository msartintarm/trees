import test from "node:test";
import assert from "node:assert/strict";

import { BRUSH_CODES, CONTROL_TYPES, DEATH_CAUSES, KEY_BITS, isControl, type Control } from "./protocol.ts";
import { DEFAULT_PARAMS } from "./simParams.ts";

test("every control variant passes the guard", () => {
  const samples: Control[] = [
    { type: "play" },
    { type: "pause" },
    { type: "step" },
    { type: "speed", value: 8 },
    { type: "reseed", seed: 42 },
    { type: "params", params: DEFAULT_PARAMS },
    { type: "paint", bx: 10, by: 20, brush: "tree" },
    { type: "orbit", dyaw: 0.1, dpitch: -0.05 },
    { type: "panBy", dx: 3, dy: -7 },
    { type: "zoom", factor: 1.1 },
    { type: "resetCamera" },
    { type: "inspect", bx: 10, by: 20 },
    { type: "rootsView", on: true },
    { type: "biomeView", on: true },
    { type: "flashes", on: false },
    { type: "walk", bx: 400, by: 300 },
    { type: "exitWalk" },
    { type: "keys", bits: 1 | 16 },
    { type: "look", dyaw: 0.01, dpitch: -0.02 },
    { type: "act", action: 1, species: 1, grass: 0 },
    { type: "inspectTarget" },
    { type: "thirdPerson" },
    { type: "hexColumns", on: true },
    { type: "landforms", on: false },
    { type: "lightMode", mode: 1 },
    { type: "bloom", on: false },
    { type: "detail", value: 0.5 },
    { type: "hexOverlay", on: true },
    { type: "resize", w: 800, h: 600 },
  ];
  assert.equal(samples.length, CONTROL_TYPES.size, "one sample per registered type");
  for (const c of samples) assert.ok(isControl(c), `rejected ${c.type}`);
});

test("junk is rejected", () => {
  assert.ok(!isControl(undefined));
  assert.ok(!isControl(null));
  assert.ok(!isControl("play"));
  assert.ok(!isControl({}));
  assert.ok(!isControl({ type: "init" }));
  assert.ok(!isControl({ type: "warp" }));
});

test("brush codes match the engine's paint() contract", () => {
  assert.equal(BRUSH_CODES.clear, 0);
  assert.equal(BRUSH_CODES.grass, 1);
  assert.equal(BRUSH_CODES.tree, 2);
});

test("walking contracts match the engine", () => {
  assert.equal(DEATH_CAUSES.length, 12, "DeathCause has 12 entries (logging last)");
  assert.equal(DEATH_CAUSES[11], "logging");
  const bits = Object.values(KEY_BITS);
  assert.equal(new Set(bits).size, bits.length);
  assert.ok(bits.every((b) => (b & (b - 1)) === 0), "each key is one bit");
});
