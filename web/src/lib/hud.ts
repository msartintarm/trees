// Pure snapshot→string formatting for the HUD, kept out of the component so
// the exact readout is unit-tested.

import type { StatsSnapshot } from "./protocol.ts";

function group(n: number): string {
  return Math.floor(n).toLocaleString("en-US");
}

export function speedLabel(s: StatsSnapshot): string {
  const speed = s.selectedSpeed === Math.floor(s.selectedSpeed)
    ? String(s.selectedSpeed)
    : s.selectedSpeed.toFixed(1);
  return s.playing ? `▶ ${speed}×` : "⏸ paused";
}

export function statsText(s: StatsSnapshot): string {
  const fire = s.burning > 0 ? ` · 🔥 ${group(s.burning)}` : "";
  const storm = s.storms > 0 ? ` · ⛈ ${group(s.storms)}` : "";
  const pct = (v: number) => `${Math.round(v * 100)}%`;
  const climate = ` · ☀ ${pct(s.sun)} 💧 ${pct(s.moisture)}`;
  return `tick ${group(s.tick)} · trees ${group(s.trees)} · grass ${group(s.grass)} · bare ${group(s.bare)}${climate}${storm}${fire} · ${speedLabel(s)}`;
}
