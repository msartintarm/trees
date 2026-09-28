import test from "node:test";
import assert from "node:assert/strict";

import {
  backingSize,
  clientToBacking,
  dragToOrbit,
  isClick,
  wheelZoomFactor,
  MAX_DPR,
} from "./camera.ts";

test("backingSize caps the device pixel ratio", () => {
  assert.deepEqual(backingSize(100, 50, 1), { w: 100, h: 50 });
  assert.deepEqual(backingSize(100, 50, 2), { w: 200, h: 100 });
  assert.deepEqual(backingSize(100, 50, 3.5), { w: 100 * MAX_DPR, h: 50 * MAX_DPR });
  assert.deepEqual(backingSize(0, 0, 0), { w: 1, h: 1 });
});

test("clientToBacking maps corners and center", () => {
  const rect = { left: 10, top: 20, width: 200, height: 100 };
  const backing = { w: 400, h: 200 };
  assert.deepEqual(clientToBacking(10, 20, rect, backing), { bx: 0, by: 0 });
  assert.deepEqual(clientToBacking(210, 120, rect, backing), { bx: 400, by: 200 });
  assert.deepEqual(clientToBacking(110, 70, rect, backing), { bx: 200, by: 100 });
});

test("wheel zoom is symmetric around 1", () => {
  const zoomOut = wheelZoomFactor(100);
  const zoomIn = wheelZoomFactor(-100);
  assert.ok(zoomOut > 1);
  assert.ok(zoomIn < 1);
  assert.ok(Math.abs(zoomOut * zoomIn - 1) < 1e-12);
});

test("drag orbit: right drag yaws, up drag pitches upward", () => {
  const o = dragToOrbit(100, -50);
  assert.ok(o.dyaw > 0);
  assert.ok(o.dpitch > 0, "dragging up should raise the camera");
  assert.ok(Math.abs(o.dyaw) > Math.abs(o.dpitch));
});

test("click slop separates paints from drags", () => {
  assert.ok(isClick(1, 2));
  assert.ok(!isClick(10, 0));
  assert.ok(!isClick(3, 3), "diagonal 3,3 exceeds the 4px slop");
});
