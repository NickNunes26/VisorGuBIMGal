import React from 'react';
import { FlipVertical2 } from 'lucide-react';
import type { ModelBounds } from '../../domain/models/ModelRecord';
import {
  SECTION_AXES,
  boxCorners,
  planePosition,
  type SectionAxis,
  type SectionState
} from '../viewer/section';

interface SectionBarProps {
  section: SectionState;
  bounds: ModelBounds | null;
  onChange: (next: SectionState) => void;
}

/** Slider steps per axis: fine enough to place a cut within a storey of a large model. */
const STEPS = 1000;

interface SliderProps {
  className: string;
  label: string;
  min: number;
  max: number;
  value: number;
  onChange: (value: number) => void;
}

const Slider: React.FC<SliderProps> = ({ className, label, min, max, value, onChange }) => (
  <input
    type="range"
    className={`section-slider ${className}`}
    min={min}
    max={max}
    step={max > min ? (max - min) / STEPS : 'any'}
    value={value}
    onChange={(event) => onChange(Number(event.target.value))}
    title={`${label}: ${value.toFixed(2)} m`}
  />
);

export const SectionBar: React.FC<SectionBarProps> = ({ section, bounds, onChange }) => {
  const update = (patch: Partial<SectionState>) => onChange({ ...section, ...patch });

  return (
    <div className="section-bar">
      <div className="display-bar-row">
        <span className="display-bar-label">Corte 3D</span>
        <select
          className="select section-mode"
          value={section.mode}
          disabled={!bounds}
          onChange={(event) => update({ mode: event.target.value as SectionState['mode'] })}
          title="Corte 3D"
        >
          <option value="off">Sin corte</option>
          <option value="plane">Plano</option>
          <option value="box">Caja</option>
        </select>
      </div>

      {bounds && section.mode === 'plane' && (
        <>
          <div className="display-bar-row">
            <select
              className="select section-axis"
              value={section.axis}
              onChange={(event) => update({ axis: event.target.value as SectionAxis, position: null })}
              title="Eje del plano"
            >
              {SECTION_AXES.map((axis) => (
                <option key={axis} value={axis}>
                  Eje {axis.toUpperCase()}
                </option>
              ))}
            </select>
            <button
              type="button"
              className={`btn btn-ghost btn-icon section-flip ${section.flip ? 'active' : ''}`}
              aria-pressed={section.flip}
              onClick={() => update({ flip: !section.flip })}
              title="Invertir: conservar el otro lado del plano"
            >
              <FlipVertical2 size={14} />
            </button>
          </div>
          <div className="display-bar-row">
            <Slider
              className="section-position"
              label={`Plano ${section.axis.toUpperCase()}`}
              min={bounds.min[section.axis]}
              max={bounds.max[section.axis]}
              value={planePosition(section, bounds)}
              onChange={(position) => update({ position })}
            />
            <span className="section-value">{planePosition(section, bounds).toFixed(2)} m</span>
          </div>
        </>
      )}

      {bounds && section.mode === 'box' && (() => {
        const { min, max } = boxCorners(section, bounds);
        return SECTION_AXES.map((axis) => (
          <div className="display-bar-row" key={axis}>
            <span className="section-axis-label">{axis.toUpperCase()}</span>
            <Slider
              className={`section-box-min-${axis}`}
              label={`Mínimo ${axis.toUpperCase()}`}
              min={bounds.min[axis]}
              max={bounds.max[axis]}
              value={min[axis]}
              onChange={(value) => update({ boxMin: { ...min, [axis]: Math.min(value, max[axis]) }, boxMax: max })}
            />
            <Slider
              className={`section-box-max-${axis}`}
              label={`Máximo ${axis.toUpperCase()}`}
              min={bounds.min[axis]}
              max={bounds.max[axis]}
              value={max[axis]}
              onChange={(value) => update({ boxMin: min, boxMax: { ...max, [axis]: Math.max(value, min[axis]) } })}
            />
          </div>
        ));
      })()}
    </div>
  );
};
