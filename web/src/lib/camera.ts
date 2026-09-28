// Pure input→camera math shared by the component and tests: backing-store
// sizing (DPR-capped), client→backing pixel mapping for picking, and the
// sensitivity mappings from pointer gestures to camera controls. The 3D
// camera itself lives in the engine; only deltas and screen points cross the
// worker boundary.

export const MAX_DPR = 2;

/** Backing-store size for a CSS box, DPR capped so a 3–4× phone doesn't
 * quadruple the fill cost. Always at least 1×1. */
export function backingSize(
  cssW: number,
  cssH: number,
  dpr: number,
  maxDpr = MAX_DPR,
): { w: number; h: number } {
  const d = Math.min(dpr || 1, maxDpr);
  return { w: Math.max(1, Math.round(cssW * d)), h: Math.max(1, Math.round(cssH * d)) };
}

export type Rect = { left: number; top: number; width: number; height: number };

/** Client (CSS) coords → backing-store pixels, the space `pick_tile` expects. */
export function clientToBacking(
  clientX: number,
  clientY: number,
  rect: Rect,
  backing: { w: number; h: number },
): { bx: number; by: number } {
  return {
    bx: ((clientX - rect.left) / Math.max(1, rect.width)) * backing.w,
    by: ((clientY - rect.top) / Math.max(1, rect.height)) * backing.h,
  };
}

/** One wheel notch → zoom factor; scroll up (deltaY < 0) zooms in (<1). */
export function wheelZoomFactor(deltaY: number, step = 1.1): number {
  return deltaY > 0 ? step : 1 / step;
}

/** Radians of orbit per CSS pixel of drag. Dragging right swings the camera
 * around the target; dragging up tilts toward top-down. */
export const ORBIT_SENSITIVITY = 0.006;

export function dragToOrbit(dxCss: number, dyCss: number): { dyaw: number; dpitch: number } {
  return { dyaw: dxCss * ORBIT_SENSITIVITY, dpitch: -dyCss * ORBIT_SENSITIVITY };
}

/** A press that moves less than this many CSS pixels is a click (paint), not
 * a drag (orbit/pan). */
export const CLICK_SLOP_PX = 4;

export function isClick(dxCss: number, dyCss: number, slop = CLICK_SLOP_PX): boolean {
  return Math.hypot(dxCss, dyCss) < slop;
}
