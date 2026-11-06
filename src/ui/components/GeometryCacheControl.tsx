import React, { useEffect, useState } from 'react';
import { Trash2 } from 'lucide-react';
import { cacheUsage, clearCache } from '../../infrastructure/kernel/geometryCache';

const megabytes = (bytes: number) => `${(bytes / 1048576).toLocaleString('es-ES', { maximumFractionDigits: 1 })} MB`;

/**
 * What the geometry cache holds in this browser, and a way to empty it. Reopening a cached model
 * replays its geometry instead of evaluating it; the files never leave the browser.
 */
export const GeometryCacheControl: React.FC = () => {
  const [usage, setUsage] = useState<{ models: number; bytes: number } | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = () => {
    cacheUsage()
      .then(setUsage)
      .catch(() => setUsage(null));
  };
  useEffect(refresh, []);

  const empty = async () => {
    setBusy(true);
    try {
      await clearCache();
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const label =
    usage === null
      ? 'Caché de geometría no disponible en este navegador.'
      : `Caché de geometría: ${usage.models} ${usage.models === 1 ? 'modelo' : 'modelos'}, ${megabytes(usage.bytes)}.`;

  return (
    <div className="geometry-cache" style={{ display: 'flex', alignItems: 'center', gap: '8px', fontSize: '12px', color: '#94a3b8' }}>
      <span title="Al reabrir un archivo ya cargado, su geometría se lee de aquí en vez de calcularse. Se guarda solo en este navegador.">
        {label}
      </span>
      {usage !== null && usage.models > 0 && (
        <button className="btn btn-ghost" onClick={empty} disabled={busy} title="Vaciar la caché de geometría">
          <Trash2 size={14} />
          <span>Vaciar</span>
        </button>
      )}
    </div>
  );
};
