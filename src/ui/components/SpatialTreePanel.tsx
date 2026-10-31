import React, { useState } from 'react';
import { ChevronRight, ChevronDown, Folder, Building, Layers, Box } from 'lucide-react';
import type { ElementRef, ModelId, ModelRecord } from '../../domain/models/ModelRecord';
import type { SpatialElement, SpatialNode, SpatialTree } from '../../domain/kernel/KernelPort';

interface SpatialTreePanelProps {
  models: ModelRecord[];
  spatialTrees: Record<ModelId, SpatialTree>;
  onSelectElement: (ref: ElementRef) => void;
  selectedRef: ElementRef | null;
}

export const SpatialTreePanel: React.FC<SpatialTreePanelProps> = ({
  models,
  spatialTrees,
  onSelectElement,
  selectedRef
}) => {
  const [expandedNodes, setExpandedNodes] = useState<Record<string, boolean>>({ root: true });

  const toggleNode = (nodeKey: string) => {
    setExpandedNodes((prev) => ({ ...prev, [nodeKey]: !prev[nodeKey] }));
  };

  const renderElementRow = (modelId: ModelId, elem: SpatialElement) => {
    const isSelected =
      selectedRef?.modelId === modelId && selectedRef?.expressId === elem.expressId;
    return (
      <div
        key={`${modelId}-elem-${elem.expressId}-${elem.via}`}
        style={{
          padding: '2px 6px',
          borderRadius: '4px',
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          gap: '6px',
          background: isSelected ? '#0284c7' : 'transparent',
          color: isSelected ? 'white' : '#cbd5e1'
        }}
        onClick={() => onSelectElement({ modelId, expressId: elem.expressId })}
      >
        <Box size={11} />
        <span>
          Elemento #{elem.expressId}
          {elem.name ? ` · ${elem.name}` : ''}
        </span>
      </div>
    );
  };

  const renderSpatialNode = (modelId: ModelId, node: SpatialNode) => {
    const nodeKey = `${modelId}-node-${node.expressId}`;
    const isExpanded = !!expandedNodes[nodeKey];
    const label = node.name || `Planta #${node.expressId}`;

    return (
      <div key={nodeKey} style={{ marginTop: '4px' }}>
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: '6px',
            cursor: 'pointer',
            color: '#94a3b8'
          }}
          onClick={() => toggleNode(nodeKey)}
        >
          {isExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          <Layers size={13} color="#10b981" />
          <span>
            {label} ({node.elements.length} elem.)
          </span>
        </div>

        {isExpanded && (
          <div style={{ paddingLeft: '18px', marginTop: '2px' }}>
            {node.children.map((child) => renderSpatialNode(modelId, child))}
            {node.elements.slice(0, 50).map((elem) => renderElementRow(modelId, elem))}
            {node.elements.length > 50 && (
              <div style={{ color: '#64748b', fontSize: '11px', padding: '2px 6px' }}>
                ... y {node.elements.length - 50} elementos más
              </div>
            )}
          </div>
        )}
      </div>
    );
  };

  if (models.length === 0) {
    return (
      <div style={{ padding: '24px 12px', textAlign: 'center', color: '#64748b', fontSize: '12px' }}>
        No hay modelos para mostrar el árbol espacial.
      </div>
    );
  }

  return (
    <div style={{ fontSize: '12px' }}>
      {models.map((model) => {
        const tree = spatialTrees[model.id];
        const rootNode = tree?.root;
        const storeys = rootNode?.children ?? [];
        const unassigned = tree?.unassigned ?? [];

        return (
          <div key={model.id} style={{ marginBottom: '12px' }}>
            {/* Model root */}
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: '6px',
                padding: '4px 0',
                fontWeight: 600,
                color: '#38bdf8',
                cursor: 'pointer'
              }}
              onClick={() => toggleNode(`model-${model.id}`)}
            >
              {expandedNodes[`model-${model.id}`] ? (
                <ChevronDown size={14} />
              ) : (
                <ChevronRight size={14} />
              )}
              <Building size={14} />
              <span title={model.fileName}>{model.fileName}</span>
            </div>

            {expandedNodes[`model-${model.id}`] && (
              <div style={{ paddingLeft: '16px' }}>
                {rootNode && (
                  <div style={{ padding: '2px 0', color: '#cbd5e1' }}>
                    <div
                      style={{ display: 'flex', alignItems: 'center', gap: '6px', cursor: 'pointer' }}
                      onClick={() => toggleNode(`${model.id}-proj-${rootNode.expressId}`)}
                    >
                      {expandedNodes[`${model.id}-proj-${rootNode.expressId}`] ? (
                        <ChevronDown size={12} />
                      ) : (
                        <ChevronRight size={12} />
                      )}
                      <Folder size={13} color="#f59e0b" />
                      <span>{rootNode.name || 'Proyecto'}</span>
                    </div>
                  </div>
                )}

                {/* Child spatial nodes (storeys, sites, buildings, spaces) */}
                {storeys.map((node) => renderSpatialNode(model.id, node))}

                {/* Direct root elements or unassigned elements */}
                {(rootNode?.elements.length ?? 0) > 0 &&
                  rootNode!.elements.slice(0, 50).map((elem) => renderElementRow(model.id, elem))}
                {unassigned.length > 0 &&
                  unassigned.slice(0, 50).map((elem) => renderElementRow(model.id, elem))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
};
