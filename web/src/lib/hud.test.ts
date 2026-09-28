import test from "node:test";
import assert from "node:assert/strict";

import { diversityLabel, speedLabel, statsText } from "./hud.ts";
import type { StatsSnapshot } from "./protocol.ts";

const base: StatsSnapshot = {
  tick: 1234,
  bare: 2784,
  grass: 800,
  trees: 512,
  burning: 0,
  storms: 0,
  infested: 0,
  sun: 0.64,
  moisture: 0.41,
  mast: false,
  diversity: 4.83,
  playing: true,
  selectedSpeed: 2,
  actualSpeed: 2,
  gridWidth: 256,
  gridHeight: 256,
  seed: 7,
};

test("statsText formats the full readout", () => {
  assert.equal(statsText(base), "256×256 · tick 1,234 · trees 512 · grass 800 · bare 2,784 · 🌿 4.8 types · ☀ 64% 💧 41% · ▶ 2×");
});

test("active fires appear in the readout", () => {
  assert.equal(
    statsText({ ...base, burning: 37 }),
    "256×256 · tick 1,234 · trees 512 · grass 800 · bare 2,784 · 🌿 4.8 types · ☀ 64% 💧 41% · 🔥 37 · ▶ 2×",
  );
});

test("paused runs show the pause glyph", () => {
  assert.equal(speedLabel({ ...base, playing: false }), "⏸ paused");
});

test("active storms appear in the readout", () => {
  assert.equal(
    statsText({ ...base, storms: 2, burning: 5 }),
    "256×256 · tick 1,234 · trees 512 · grass 800 · bare 2,784 · 🌿 4.8 types · ☀ 64% 💧 41% · ⛈ 2 · 🔥 5 · ▶ 2×",
  );
});

test("pest outbreaks appear in the readout", () => {
  assert.equal(
    statsText({ ...base, infested: 42, burning: 3 }),
    "256×256 · tick 1,234 · trees 512 · grass 800 · bare 2,784 · 🌿 4.8 types · ☀ 64% 💧 41% · 🔥 3 · 🐛 42 · ▶ 2×",
  );
});

test("mast years show an acorn badge", () => {
  assert.equal(
    statsText({ ...base, mast: true }),
    "256×256 · tick 1,234 · trees 512 · grass 800 · bare 2,784 · 🌿 4.8 types · ☀ 64% 💧 41% 🌰 · ▶ 2×",
  );
});

test("biodiversity reads as effective types with one decimal", () => {
  assert.equal(diversityLabel({ ...base, diversity: 1 }), "🌿 1.0 types");
  assert.equal(diversityLabel({ ...base, diversity: 5.26 }), "🌿 5.3 types");
});

test("a map that can't keep up shows its achieved speed", () => {
  assert.equal(speedLabel({ ...base, selectedSpeed: 32, actualSpeed: 6.4 }), "▶ 32× (≈6.4×)");
  assert.equal(speedLabel({ ...base, selectedSpeed: 32, actualSpeed: 12.6 }), "▶ 32× (≈13×)");
  assert.equal(speedLabel({ ...base, selectedSpeed: 8, actualSpeed: 7.9 }), "▶ 8×");
});

test("fractional speeds keep one decimal", () => {
  assert.equal(speedLabel({ ...base, selectedSpeed: 0.5 }), "▶ 0.5×");
});
