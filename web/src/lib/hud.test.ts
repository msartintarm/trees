import test from "node:test";
import assert from "node:assert/strict";

import {
  clockLabel,
  cloudsText,
  deathsText,
  diversityLabel,
  newestCloudEvent,
  paceLabel,
  seasonLabel,
  speedLabel,
  statsText,
  walkText,
} from "./hud.ts";
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
  localDiversity: 3.07,
  deathsRecent: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  heat: 0.5,
  clouds: [0, 0, 0, 0],
  cloudEvents: [0, 0, 0, 0, 0, 0, 0, 0],
  biomeShares: [0, 0, 0, 0, 0, 0, 0],
  biomeSamples: [],
  flooding: false,
  houses: 0,
  walk: null,
  playing: true,
  selectedSpeed: 2,
  actualSpeed: 2,
  gridWidth: 256,
  gridHeight: 256,
  seed: 7,
};

const HEAD = "256×256 · tick 1,234 · trees 512 · grass 800 · bare 2,784 · 🌿 4.8 types (local 3.1)";

test("statsText formats the full readout", () => {
  assert.equal(statsText(base), `${HEAD} · ☀ 64% 💧 41% · ▶ 2×`);
});

test("active fires appear in the readout", () => {
  assert.equal(statsText({ ...base, burning: 37 }), `${HEAD} · ☀ 64% 💧 41% · 🔥 37 · ▶ 2×`);
});

test("paused runs show the pause glyph", () => {
  assert.equal(speedLabel({ ...base, playing: false }), "⏸ paused");
});

test("active storms appear in the readout", () => {
  assert.equal(statsText({ ...base, storms: 2, burning: 5 }), `${HEAD} · ☀ 64% 💧 41% · ⛈ 2 · 🔥 5 · ▶ 2×`);
});

test("river floods show a badge", () => {
  assert.equal(statsText({ ...base, flooding: true, storms: 1 }), `${HEAD} · ☀ 64% 💧 41% · ⛈ 1 · 🌊 flood · ▶ 2×`);
});

test("pest outbreaks appear in the readout", () => {
  assert.equal(statsText({ ...base, infested: 42, burning: 3 }), `${HEAD} · ☀ 64% 💧 41% · 🔥 3 · 🐛 42 · ▶ 2×`);
});

test("mast years show an acorn badge", () => {
  assert.equal(statsText({ ...base, mast: true }), `${HEAD} · ☀ 64% 💧 41% 🌰 · ▶ 2×`);
});

test("houses show once built", () => {
  assert.equal(statsText({ ...base, houses: 2 }), `${HEAD} · ☀ 64% 💧 41% · 🏠 2 · ▶ 2×`);
});

test("biodiversity reads whole-map and local effective types", () => {
  assert.equal(diversityLabel({ ...base, diversity: 1, localDiversity: 1 }), "🌿 1.0 types (local 1.0)");
  assert.equal(diversityLabel({ ...base, diversity: 5.26, localDiversity: 2.96 }), "🌿 5.3 types (local 3.0)");
});

test("a map that can't keep up shows its achieved speed", () => {
  assert.equal(speedLabel({ ...base, selectedSpeed: 32, actualSpeed: 6.4 }), "▶ 32× (≈6.4×)");
  assert.equal(speedLabel({ ...base, selectedSpeed: 32, actualSpeed: 12.6 }), "▶ 32× (≈13×)");
  assert.equal(speedLabel({ ...base, selectedSpeed: 8, actualSpeed: 7.9 }), "▶ 8×");
});

test("fractional speeds keep one decimal", () => {
  assert.equal(speedLabel({ ...base, selectedSpeed: 0.5 }), "▶ 0.5×");
  assert.equal(speedLabel({ ...base, selectedSpeed: 0.05, actualSpeed: 0.05 }), "▶ 0.05×");
});

test("recent deaths list the top three causes", () => {
  assert.equal(deathsText(base), "");
  const deathsRecent = [4.2, 1.0, 9.6, 0, 0, 12.4, 0, 0.3, 0, 0, 0, 0];
  assert.equal(deathsText({ ...base, deathsRecent }), "☠ pests 12 · starvation 10 · old age 4");
});

test("the sky reads by genus", () => {
  assert.equal(cloudsText(base), "");
  assert.equal(cloudsText({ ...base, clouds: [6, 1, 0, 2] }), "☁ 6 cumulus · 1 thunderhead · 2 cirrus");
});

test("the weather ticker announces new transitions, not evaporation", () => {
  const a = [3, 1, 0, 0, 0, 4, 0];
  assert.equal(newestCloudEvent(null, a), null);
  assert.equal(newestCloudEvent(a, a), null);
  assert.equal(newestCloudEvent(a, [3, 2, 0, 0, 0, 4, 0]), "⛈ a cumulus towered into a thunderhead");
  assert.equal(newestCloudEvent(a, [3, 1, 0, 0, 0, 9, 0]), null);
});

test("seasons read early/mid/late", () => {
  assert.equal(seasonLabel(0), "early spring");
  assert.equal(seasonLabel(0.2), "late spring");
  assert.equal(seasonLabel(0.3), "early summer");
  assert.equal(seasonLabel(0.62), "mid autumn");
  assert.equal(seasonLabel(0.99), "late winter");
  assert.equal(seasonLabel(3.3), "early summer");
});

test("time reads as a clock when slow and a blur when fast", () => {
  assert.equal(clockLabel(0.5, 0.04), "12:00");
  assert.equal(clockLabel(0.25 + 5 / 1440, 0.04), "06:05");
  assert.equal(clockLabel(0.5, 3), "days blur past");
  assert.equal(paceLabel(1 / 24), "⏱ 1.0 h/s");
  assert.equal(paceLabel(12.2), "⏩ 12 days/s");
  assert.equal(paceLabel(4.24), "⏩ 4.2 days/s");
  assert.equal(paceLabel(548), "⏩ 1.5 yr/s");
});

test("the wanderer's readout", () => {
  const w = {
    years: 2.4,
    dayFrac: 0.5,
    daysPerSec: 1 / 24,
    lapse: 0,
    wood: 4.25,
    wading: true,
    thirdPerson: false,
    yearFrac: 0.4,
    message: "",
    target: "",
  };
  assert.equal(walkText(w), "🧭 year 3 · mid summer · 12:00 · ⏱ 1.0 h/s · 🪵 4.3 timber · 🌊 wading");
});
