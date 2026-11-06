import React, { useState } from 'react';
import { BookmarkPlus, X } from 'lucide-react';
import type { SavedView } from '../viewer/savedViews';

interface SavedViewsBarProps {
  views: readonly SavedView[];
  nextIndex: number;
  onSave: (name: string) => void;
  onApply: (view: SavedView) => void;
  onDelete: (id: string) => void;
}

export const SavedViewsBar: React.FC<SavedViewsBarProps> = ({
  views,
  nextIndex,
  onSave,
  onApply,
  onDelete
}) => {
  const [name, setName] = useState('');

  const handleSave = () => {
    onSave(name);
    setName('');
  };

  return (
    <div className="saved-views-bar">
      <div className="display-bar-row">
        <span className="display-bar-label">Vistas</span>
        <input
          type="text"
          className="select saved-view-name"
          placeholder={`Vista ${nextIndex}`}
          value={name}
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault();
              handleSave();
            }
          }}
          title="Nombre de la vista guardada"
        />
        <button
          type="button"
          className="btn btn-ghost btn-icon saved-view-add"
          onClick={handleSave}
          title="Guardar la vista actual (cámara, visibilidad, corte y selección)"
        >
          <BookmarkPlus size={14} />
        </button>
      </div>

      {views.length > 0 && (
        <ul className="saved-views-list">
          {views.map((view) => (
            <li key={view.id} className="saved-view-item" data-id={view.id}>
              <button
                type="button"
                className="saved-view-apply"
                onClick={() => onApply(view)}
                title={`Restaurar ${view.name}`}
              >
                {view.name}
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-icon saved-view-delete"
                onClick={() => onDelete(view.id)}
                title={`Borrar ${view.name}`}
              >
                <X size={12} />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};
