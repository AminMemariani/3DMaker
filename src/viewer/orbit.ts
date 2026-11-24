// Pure orbit-camera math (no PlayCanvas), so it can be unit tested.
//
// Convention: yaw rotates about +Y (yaw 0 → camera on +Z looking toward -Z),
// pitch < 0 means the camera is above the target (looking down).

export type Vec3T = [number, number, number];

export interface OrbitPose {
  yaw: number; // degrees
  pitch: number; // degrees, clamped to ±89
  distance: number;
  target: Vec3T;
}

export const DEFAULT_YAW = 35;
export const DEFAULT_PITCH = -15;
export const FRAME_MARGIN = 1.15;
const PITCH_LIMIT = 89;

const rad = (d: number) => (d * Math.PI) / 180;
const deg = (r: number) => (r * 180) / Math.PI;

export function clampPitch(p: number): number {
  return Math.max(-PITCH_LIMIT, Math.min(PITCH_LIMIT, p));
}

export function clampDistance(d: number, radius: number): number {
  const r = radius > 0 && Number.isFinite(radius) ? radius : 1;
  return Math.max(r * 0.25, Math.min(r * 25, d));
}

export function poseToPosition(p: OrbitPose): Vec3T {
  const y = rad(p.yaw);
  const x = rad(p.pitch);
  return [
    p.target[0] + p.distance * Math.sin(y) * Math.cos(x),
    p.target[1] - p.distance * Math.sin(x),
    p.target[2] + p.distance * Math.cos(y) * Math.cos(x),
  ];
}

export function poseFromView(position: Vec3T, target: Vec3T): OrbitPose {
  const v: Vec3T = [position[0] - target[0], position[1] - target[1], position[2] - target[2]];
  const d = Math.hypot(v[0], v[1], v[2]) || 1;
  return {
    yaw: deg(Math.atan2(v[0], v[2])),
    pitch: clampPitch(-deg(Math.asin(Math.max(-1, Math.min(1, v[1] / d))))),
    distance: d,
    target: [...target] as Vec3T,
  };
}

/** Pose that fits a sphere of `radius` into the view with a margin, from a three-quarter angle. */
export function framePose(center: Vec3T, radius: number, vfovDeg: number, aspect: number): OrbitPose {
  const r = radius > 0 && Number.isFinite(radius) ? radius : 1;
  const halfV = rad(vfovDeg) / 2;
  const halfH = Math.atan(Math.tan(halfV) * (aspect > 0 ? aspect : 1));
  const half = Math.min(halfV, halfH);
  return {
    yaw: DEFAULT_YAW,
    pitch: DEFAULT_PITCH,
    distance: (r * FRAME_MARGIN) / Math.sin(half),
    target: [...center] as Vec3T,
  };
}

/** Camera right/up basis for the pose. */
export function basis(p: OrbitPose): { right: Vec3T; up: Vec3T } {
  const y = rad(p.yaw);
  const x = rad(p.pitch);
  const right: Vec3T = [Math.cos(y), 0, -Math.sin(y)];
  const fwd: Vec3T = [-Math.sin(y) * Math.cos(x), Math.sin(x), -Math.cos(y) * Math.cos(x)];
  const up: Vec3T = [
    right[1] * fwd[2] - right[2] * fwd[1],
    right[2] * fwd[0] - right[0] * fwd[2],
    right[0] * fwd[1] - right[1] * fwd[0],
  ];
  return { right, up };
}

/** Pan by screen pixels; content follows the pointer. */
export function panPose(p: OrbitPose, dxPx: number, dyPx: number, vfovDeg: number, viewportH: number): OrbitPose {
  const wpp = (2 * p.distance * Math.tan(rad(vfovDeg) / 2)) / Math.max(1, viewportH);
  const { right, up } = basis(p);
  const t = p.target;
  return {
    ...p,
    target: [
      t[0] - right[0] * dxPx * wpp + up[0] * dyPx * wpp,
      t[1] - right[1] * dxPx * wpp + up[1] * dyPx * wpp,
      t[2] - right[2] * dxPx * wpp + up[2] * dyPx * wpp,
    ],
  };
}

/** Rotate a point by XYZ Euler degrees (R = Rz · Ry · Rx, matching PlayCanvas setEulerAngles). */
export function rotateEuler(v: Vec3T, eDeg: Vec3T): Vec3T {
  const [ax, ay, az] = eDeg.map(rad);
  let [x, y, z] = v;
  // X
  [y, z] = [y * Math.cos(ax) - z * Math.sin(ax), y * Math.sin(ax) + z * Math.cos(ax)];
  // Y
  [x, z] = [x * Math.cos(ay) + z * Math.sin(ay), -x * Math.sin(ay) + z * Math.cos(ay)];
  // Z
  [x, y] = [x * Math.cos(az) - y * Math.sin(az), x * Math.sin(az) + y * Math.cos(az)];
  return [x, y, z];
}
