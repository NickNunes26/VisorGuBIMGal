import React, { useEffect, useSyncExternalStore } from 'react';
import { Check, Trash2, X } from 'lucide-react';
import { MEASURE_KINDS, canClose, measurementText, type MeasureKind } from '../viewer/measure';
import type { MeasureController } from '../viewer/MeasureController';

interface MeasureBarProps {
  controller: MeasureController;
}

const LABELS = Object.fromEntries(MEASURE_KINDS.map(({ kind, label }) => [kind, label])) as Record<
  MeasureKind,
  string
>;

export const MeasureBar: React.FC<MeasureBarProps> = ({ controller }) => {
  const { kind, draft, measurements } = useSyncExternalStore(controller.subscribe, controller.getState);
  const hint = MEASURE_KINDS.find((item) => item.kind === kind)?.hint;

  // Escape starts the measurement in hand again; Enter closes an area.
  useEffect(() => {
    if (!kind) return;
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return;
      if (event.key === 'Escape') controller.cancelDraft();
      if (event.key === 'Enter') controller.closeDraft();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [controller, kind]);

  return (
    <div className="measure-bar">
      <div className="display-bar-row">
        <span className="display-bar-label">Medir</span>
        <select
          className="select measure-mode"
          value={kind ?? ''}
          onChange={(event) => controller.setKind((event.target.value || null) as MeasureKind | null)}
          title="Medir"
        >
          <option value="">Sin medir</option>
          {MEASURE_KINDS.map((item) => (
            <option key={item.kind} value={item.kind}>
              {item.label}
            </option>
          ))}
        </select>
        {draft && canClose(draft) && (
          <button
            type="button"
            className="btn btn-ghost btn-icon measure-close"
            onClick={() => controller.closeDraft()}
            title="Cerrar el área (Intro)"
          >
            <Check size={14} />
          </button>
        )}
        {measurements.length > 0 && (
          <button
            type="button"
            className="btn btn-ghost btn-icon measure-clear"
            onClick={() => controller.clear()}
            title="Borrar todas las mediciones"
          >
            <Trash2 size={14} />
          </button>
        )}
      </div>

      {hint && (
        <div className="display-bar-status measure-hint">
          {hint}
          {draft && draft.points.length > 0 && ` ${draft.points.length} marcado(s); Esc para empezar de nuevo.`}
        </div>
      )}

      {measurements.length > 0 && (
        <ul className="measure-list">
          {measurements.map((measurement) => {
            const { value, detail } = measurementText(measurement);
            return (
              <li
                key={measurement.id}
                className="measure-result"
                data-kind={measurement.kind}
                data-id={measurement.id}
                title={detail}
              >
                <span className="measure-kind">{LABELS[measurement.kind]}</span>
                <span className="measure-value">{value}</span>
                <button
                  type="button"
                  className="btn btn-ghost btn-icon measure-delete"
                  onClick={() => controller.remove(measurement.id)}
                  title="Borrar esta medición"
                >
                  <X size={12} />
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
};
