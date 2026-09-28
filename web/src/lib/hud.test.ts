import test from "node:test";
import assert from "node:assert/strict";

import { speedLabel, statsText } from "./hud.ts";
import type { StatsSnapshot } from "./protocol.ts";

const base: StatsSnapshot = {
  tick: 1234,
  bare: 2784,
  grass: 800,
  trees: 512,
  burning: 0,
  storms: 0,
  sun: 0.64,
  moisture: 0.41,
  playing: true,
  selectedSpeed: 2,
  seed: 7,
};

test("statsText formats the full readout", () => {
  assert.equal(statsText(base), "tick 1,234 · trees 512 · grass 800 · bare 2,784 · ☀ 64% 💧 41% · ▶ 2×");
});

test("active fires appear in the readout", () => {
  assert.equal(
    statsText({ ...base, burning: 37 }),
    "tick 1,234 · trees 512 · grass 800 · bare 2,784 · ☀ 64% 💧 41% · 🔥 37 · ▶ 2×",
  );
});

test("paused runs show the pause glyph", () => {
  assert.equal(speedLabel({ ...base, playing: false }), "⏸ paused");
});

test("active storms appear in the readout", () => {
  assert.equal(
    statsText({ ...base, storms: 2, burning: 5 }),
    "tick 1,234 · trees 512 · grass 800 · bare 2,784 · ☀ 64% 💧 41% · ⛈ 2 · 🔥 5 · ▶ 2×",
  );
});

test("fractional speeds keep one decimal", () => {
  assert.equal(speedLabel({ ...base, selectedSpeed: 0.5 }), "▶ 0.5×");
});
