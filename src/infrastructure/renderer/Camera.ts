import type { ModelBounds } from '../../domain/models/ModelRecord';
import type { ViewPreset } from '../../domain/kernel/RendererPort';
import type { Ray, Vec3 } from '../../domain/measure/geometry';
import { WHOLE_VIEW, type Frustum, type ViewWindow } from './frustum';

const ISO_AZIMUTH = -Math.PI / 4; // +X, -Y (front-right in IFC Z-up)
const ISO_ELEVATION = Math.atan(1 / Math.SQRT2); // ~35.264 deg above horizontal

/**
 * Orbit camera operating in IFC Z-up f64 world coordinates.
 * Produces a rotation-only view matrix at the camera eye (for RTC rendering)
 * and a Reverse-Z perspective projection matrix (depth 1 at near, 0 at far).
 */
export class Camera {
  /** Target point in IFC Z-up f64 metres. */
  private target: [number, number, number] = [0, 0, 0];
  /** Distance from target to eye in f64 metres. */
  private distance = 12;
  /** Horizontal angle in the X-Y plane from +X towards +Y (radians). */
  private azimuth = ISO_AZIMUTH;
  /** Vertical angle from the X-Y plane towards +Z (radians, in [-pi/2, +pi/2]). */
  private elevation = ISO_ELEVATION;
  /** Viewport aspect ratio (width / height). */
  private aspect = 16 / 9;
  /** Vertical field of view in radians (45 deg). */
  private readonly fovY = Math.PI / 4;

  setAspect(aspect: number): void {
    if (Number.isFinite(aspect) && aspect > 0) {
      this.aspect = aspect;
    }
  }

  /** Returns camera eye position in IFC Z-up f64 coordinates. */
  getEye(): [number, number, number] {
    const cosPhi = Math.cos(this.elevation);
    const sinPhi = Math.sin(this.elevation);
    const cosTheta = Math.cos(this.azimuth);
    const sinTheta = Math.sin(this.azimuth);

    return [
      this.target[0] + this.distance * cosPhi * cosTheta,
      this.target[1] + this.distance * cosPhi * sinTheta,
      this.target[2] + this.distance * sinPhi
    ];
  }

  getTarget(): [number, number, number] {
    return [this.target[0], this.target[1], this.target[2]];
  }

  getDistance(): number {
    return this.distance;
  }

  orbit(dx: number, dy: number): void {
    this.azimuth -= dx * 0.007;
    const nextElev = this.elevation + dy * 0.007;
    const maxElev = Math.PI / 2;
    this.elevation = Math.max(-maxElev, Math.min(maxElev, nextElev));
  }

  pan(dx: number, dy: number): void {
    const cosPhi = Math.cos(this.elevation);
    const sinPhi = Math.sin(this.elevation);
    const cosTheta = Math.cos(this.azimuth);
    const sinTheta = Math.sin(this.azimuth);

    // Orthonormal right (r) and up (u) vectors in IFC Z-up
    const rx = -sinTheta;
    const ry = cosTheta;
    const rz = 0;

    const ux = -sinPhi * cosTheta;
    const uy = -sinPhi * sinTheta;
    const uz = cosPhi;

    const panScale = this.distance * 0.0014;
    const moveRight = -dx * panScale;
    const moveUp = dy * panScale;

    this.target[0] += rx * moveRight + ux * moveUp;
    this.target[1] += ry * moveRight + uy * moveUp;
    this.target[2] += rz * moveRight + uz * moveUp;
  }

  zoom(deltaY: number): void {
    this.zoomBy(deltaY > 0 ? 1.12 : 1 / 1.12);
  }

  /** Multiplies the distance to the target: below 1 moves closer. */
  zoomBy(factor: number): void {
    if (!Number.isFinite(factor) || factor <= 0) return;
    this.distance = Math.max(0.05, Math.min(500000, this.distance * factor));
  }

  fitToBounds(bounds: ModelBounds): void {
    const cx = (bounds.min.x + bounds.max.x) * 0.5;
    const cy = (bounds.min.y + bounds.max.y) * 0.5;
    const cz = (bounds.min.z + bounds.max.z) * 0.5;

    const dx = Math.max(0.1, bounds.max.x - bounds.min.x);
    const dy = Math.max(0.1, bounds.max.y - bounds.min.y);
    const dz = Math.max(0.1, bounds.max.z - bounds.min.z);
    const radius = 0.5 * Math.hypot(dx, dy, dz);

    this.target = [cx, cy, cz];
    const halfFov = this.fovY * 0.5;
    const fitScale = this.aspect < 1 ? 1 / Math.max(0.25, this.aspect) : 1;
    this.distance = Math.max(1.0, (radius / Math.tan(halfFov)) * 1.25 * fitScale);
    this.azimuth = ISO_AZIMUTH;
    this.elevation = ISO_ELEVATION;
  }

  setPreset(preset: ViewPreset, bounds: ModelBounds | null): void {
    if (bounds) {
      this.target = [
        (bounds.min.x + bounds.max.x) * 0.5,
        (bounds.min.y + bounds.max.y) * 0.5,
        (bounds.min.z + bounds.max.z) * 0.5
      ];
      const dx = Math.max(0.1, bounds.max.x - bounds.min.x);
      const dy = Math.max(0.1, bounds.max.y - bounds.min.y);
      const dz = Math.max(0.1, bounds.max.z - bounds.min.z);
      const maxDim = Math.max(dx, dy, dz);
      this.distance = Math.max(1.5, maxDim * 2.1);
    }

    switch (preset) {
      case 'iso':
        this.azimuth = ISO_AZIMUTH;
        this.elevation = ISO_ELEVATION;
        break;
      case 'top':
        // Looking straight down (-Z_ifc) with +Y_ifc (North) up and +X_ifc (East) right
        this.azimuth = -Math.PI / 2;
        this.elevation = Math.PI / 2;
        break;
      case 'front':
        // Looking from -Y_ifc towards +Y_ifc with +Z_ifc up and +X_ifc right
        this.azimuth = -Math.PI / 2;
        this.elevation = 0;
        break;
      case 'side':
        // Looking from +X_ifc towards -X_ifc with +Z_ifc up and +Y_ifc right
        this.azimuth = 0;
        this.elevation = 0;
        break;
    }
  }

  private near(): number {
    return Math.max(0.01, Math.min(2.0, this.distance * 0.002));
  }

  private far(): number {
    return Math.max(50000.0, this.distance * 250.0);
  }

  /** Forward, right and up unit vectors in IFC Z-up; see writeUniforms. */
  private axes(): { forward: Vec3; right: Vec3; up: Vec3 } {
    const cosPhi = Math.cos(this.elevation);
    const sinPhi = Math.sin(this.elevation);
    const cosTheta = Math.cos(this.azimuth);
    const sinTheta = Math.sin(this.azimuth);
    return {
      forward: [-cosPhi * cosTheta, -cosPhi * sinTheta, -sinPhi],
      right: [-sinTheta, cosTheta, 0],
      up: [-sinPhi * cosTheta, -sinPhi * sinTheta, cosPhi]
    };
  }

  /**
   * Where a point shows, in normalised device coordinates (x right, y up, -1 to 1 across the
   * view), in f64. Null in front of the near plane or behind the eye.
   */
  project(point: Vec3): { x: number; y: number } | null {
    const eye = this.getEye();
    const { forward, right, up } = this.axes();
    const d: Vec3 = [point[0] - eye[0], point[1] - eye[1], point[2] - eye[2]];
    const along = (axis: Vec3) => d[0] * axis[0] + d[1] * axis[1] + d[2] * axis[2];
    const depth = along(forward);
    if (!(depth >= this.near())) return null;
    const h = 1.0 / Math.tan(this.fovY * 0.5);
    return { x: ((h / this.aspect) * along(right)) / depth, y: (h * along(up)) / depth };
  }

  /** The ray from the eye through a point in normalised device coordinates. */
  rayAt(ndcX: number, ndcY: number): Ray {
    const { forward, right, up } = this.axes();
    const h = 1.0 / Math.tan(this.fovY * 0.5);
    const a = (ndcX * this.aspect) / h;
    const b = ndcY / h;
    const v: Vec3 = [
      forward[0] + right[0] * a + up[0] * b,
      forward[1] + right[1] * a + up[1] * b,
      forward[2] + right[2] * a + up[2] * b
    ];
    const l = Math.hypot(v[0], v[1], v[2]);
    return { origin: this.getEye(), direction: [v[0] / l, v[1] / l, v[2] / l] };
  }

  /**
   * What `writeUniforms` lets through a window of the view, as planes in absolute coordinates:
   * depth between near and far, and each side of the window, where a camera coordinate times
   * its projection scale equals the depth times the window's edge.
   */
  frustum(window: ViewWindow = WHOLE_VIEW): Frustum {
    const eye = this.getEye();
    const { forward, right, up } = this.axes();
    const h = 1.0 / Math.tan(this.fovY * 0.5);
    const w = h / this.aspect;
    const planes = new Float64Array(24);
    const put = (at: number, n: Vec3, offset: number) => {
      planes.set(n, at);
      planes[at + 3] = offset - (n[0] * eye[0] + n[1] * eye[1] + n[2] * eye[2]);
    };
    // edge·forward − scale·side: inside when the side coordinate shows at or before the edge.
    const tilt = (edge: number, scale: number, side: Vec3): Vec3 => [
      edge * forward[0] - scale * side[0],
      edge * forward[1] - scale * side[1],
      edge * forward[2] - scale * side[2]
    ];
    put(0, forward, -this.near());
    put(4, [-forward[0], -forward[1], -forward[2]], this.far());
    put(8, tilt(window.right, w, right), 0);
    put(12, tilt(-window.left, -w, right), 0);
    put(16, tilt(window.top, h, up), 0);
    put(20, tilt(-window.bottom, -h, up), 0);
    return planes;
  }

  /**
   * Writes column-major Reverse-Z projection (floats 0..15) and
   * rotation-only view matrix at the camera eye (floats 16..31).
   */
  writeUniforms(out: Float32Array): void {
    // 1. Reverse-Z perspective projection matrix (clip z in [0, w], near -> 1, far -> 0)
    const h = 1.0 / Math.tan(this.fovY * 0.5);
    const w = h / this.aspect;
    const near = this.near();
    const far = this.far();
    const a = near / (far - near);
    const b = (far * near) / (far - near);

    out[0] = w;
    out[1] = 0;
    out[2] = 0;
    out[3] = 0;

    out[4] = 0;
    out[5] = h;
    out[6] = 0;
    out[7] = 0;

    out[8] = 0;
    out[9] = 0;
    out[10] = a;
    out[11] = -1;

    out[12] = 0;
    out[13] = 0;
    out[14] = b;
    out[15] = 0;

    // 2. Rotation-only view matrix at camera eye (maps IFC Z-up into camera space: +X right, +Y up, -Z forward)
    const cosPhi = Math.cos(this.elevation);
    const sinPhi = Math.sin(this.elevation);
    const cosTheta = Math.cos(this.azimuth);
    const sinTheta = Math.sin(this.azimuth);

    // Forward f = (-cosPhi*cosTheta, -cosPhi*sinTheta, -sinPhi)
    // Right   r = (-sinTheta, cosTheta, 0)
    // Up      u = r x f = (-sinPhi*cosTheta, -sinPhi*sinTheta, cosPhi)
    const rx = -sinTheta;
    const ry = cosTheta;
    const rz = 0;

    const ux = -sinPhi * cosTheta;
    const uy = -sinPhi * sinTheta;
    const uz = cosPhi;

    const negFx = cosPhi * cosTheta;
    const negFy = cosPhi * sinTheta;
    const negFz = sinPhi;

    // Column 0 (X_ifc basis in camera space)
    out[16] = rx;
    out[17] = ux;
    out[18] = negFx;
    out[19] = 0;

    // Column 1 (Y_ifc basis in camera space)
    out[20] = ry;
    out[21] = uy;
    out[22] = negFy;
    out[23] = 0;

    // Column 2 (Z_ifc basis in camera space)
    out[24] = rz;
    out[25] = uz;
    out[26] = negFz;
    out[27] = 0;

    // Column 3 (zero translation — RTC offset is applied per batch in f64 on the CPU)
    out[28] = 0;
    out[29] = 0;
    out[30] = 0;
    out[31] = 1;
  }
}
