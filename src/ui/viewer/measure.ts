import { angleAt, distanceBetween, polygonArea, type Vec3 } from '../../domain/measure/geometry';

export type MeasureKind = 'distance' | 'height' | 'area' | 'angle';

export const MEASURE_KINDS: readonly { kind: MeasureKind; label: string; hint: string }[] = [
  { kind: 'distance', label: 'Distancia', hint: 'Marca dos puntos.' },
  { kind: 'height', label: 'Desnivel', hint: 'Marca dos puntos: el segundo se compara con el primero.' },
  { kind: 'area', label: 'Área', hint: 'Marca el contorno y ciérralo en el primer punto o con Intro.' },
  { kind: 'angle', label: 'Ángulo', hint: 'Marca un punto, el vértice y otro punto.' }
];

/** Points in absolute model coordinates, in the order they were marked. */
export interface Measurement {
  id: number;
  kind: MeasureKind;
  points: Vec3[];
}

/** Points that complete a measurement; an area takes as many as the user marks. */
const POINTS: Record<MeasureKind, number | null> = { distance: 2, height: 2, area: null, angle: 3 };

export function startMeasurement(kind: MeasureKind, id: number): Measurement {
  return { id, kind, points: [] };
}

/** Adds a point: the measurement is `done` once it has its points, else still the `draft`. */
export function addPoint(
  draft: Measurement,
  point: Vec3
): { draft: Measurement | null; done: Measurement | null } {
  const next = { ...draft, points: [...draft.points, point] };
  return next.points.length === POINTS[draft.kind] ? { draft: null, done: next } : { draft: next, done: null };
}

export function canClose(draft: Measurement): boolean {
  return draft.kind === 'area' && draft.points.length >= 3;
}

export function closeArea(draft: Measurement): Measurement | null {
  return canClose(draft) ? { ...draft, points: [...draft.points] } : null;
}

const metres = (value: number) => `${value.toFixed(3)} m`;
const signed = (value: number) => `${value >= 0 ? '+' : '-'}${metres(Math.abs(value))}`;

/** What the measurement reads, and the figures behind it. Empty while points are missing. */
export function measurementText(measurement: Measurement): { value: string; detail: string } {
  const [a, b, c] = measurement.points;
  switch (measurement.kind) {
    case 'distance': {
      if (!b) return { value: '', detail: '' };
      const d = distanceBetween(a, b);
      return {
        value: metres(d.length),
        detail: `ΔX ${signed(d.dx)} · ΔY ${signed(d.dy)} · ΔZ ${signed(d.dz)} · horizontal ${metres(d.horizontal)}`
      };
    }
    case 'height': {
      if (!b) return { value: '', detail: '' };
      const d = distanceBetween(a, b);
      return { value: signed(d.dz), detail: `horizontal ${metres(d.horizontal)} · directa ${metres(d.length)}` };
    }
    case 'area': {
      const area = polygonArea(measurement.points);
      if (!area) return { value: '', detail: '' };
      return { value: `${area.area.toFixed(3)} m²`, detail: `perímetro ${metres(area.perimeter)}` };
    }
    case 'angle': {
      const angle = c ? angleAt(a, b, c) : null;
      if (angle === null) return { value: '', detail: '' };
      return { value: `${angle.toFixed(1)}°`, detail: `suplementario ${(180 - angle).toFixed(1)}°` };
    }
  }
}
