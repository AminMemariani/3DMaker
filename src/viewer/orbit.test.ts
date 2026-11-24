import { framePose, poseFromView, poseToPosition, rotateEuler, clampDistance, panPose } from "./orbit";

const close = (a: number[], b: number[], eps = 1e-6) => a.forEach((v, i) => expect(v).toBeCloseTo(b[i], 5 + 0 * eps));

describe("orbit math", () => {
  it("round-trips view ↔ pose", () => {
    const p = poseFromView([0, 0, -3], [0, 0, 0]);
    expect(Math.abs(p.yaw)).toBeCloseTo(180);
    close(poseToPosition(p), [0, 0, -3]);
    const q = poseFromView([1, 2, 3], [0.5, 0, 0]);
    close(poseToPosition(q), [1, 2, 3]);
  });

  it("frames a sphere with margin from slightly above", () => {
    const p = framePose([1, 2, 3], 1, 45, 1);
    expect(p.distance).toBeCloseTo(1.15 / Math.sin((22.5 * Math.PI) / 180));
    expect(p.pitch).toBeLessThan(0);
    expect(poseToPosition(p)[1]).toBeGreaterThan(2); // above target
    // Narrow viewports use the horizontal FOV.
    expect(framePose([0, 0, 0], 1, 45, 0.5).distance).toBeGreaterThan(p.distance);
  });

  it("rotates XYZ Euler like PlayCanvas", () => {
    close(rotateEuler([0, 1, 0], [90, 0, 0]), [0, 0, 1]);
    close(rotateEuler([1, 0, 0], [0, 0, 90]), [0, 1, 0]);
  });

  it("clamps distance and pans in screen space", () => {
    expect(clampDistance(1000, 1)).toBe(25);
    const p = panPose({ yaw: 0, pitch: 0, distance: 3, target: [0, 0, 0] }, 10, 0, 45, 500);
    expect(p.target[0]).toBeLessThan(0);
  });
});
