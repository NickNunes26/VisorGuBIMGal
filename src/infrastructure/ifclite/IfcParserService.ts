import {
  IfcParser,
  extractGeoreferencing,
  type IfcDataStore
} from '@ifc-lite/parser';
import type {
  AttributeValue,
  CoordinateReference,
  ElementRef,
  ModelId,
  PropertyScalar,
  PropertySetItem
} from '../../domain/models/ModelRecord';
import type {
  ElementDetails,
  ModelSummary,
  ProgressHandler,
  SchemaId,
  SpatialElement,
  SpatialNode,
  SpatialTree
} from '../../domain/kernel/KernelPort';
import { detectCrsFromText, utm29NToLatLng, DEFAULT_GALICIA_CENTER } from '../../domain/coordinates/Coordinates';

function toSchemaId(raw: string): SchemaId {
  const upper = raw.toUpperCase();
  if (upper.includes('IFC4X3') || upper.includes('IFC4X')) return 'IFC4X3';
  if (upper.includes('IFC4')) return 'IFC4';
  return 'IFC2X3';
}

function toPropertyScalar(val: unknown): PropertyScalar {
  if (val === null || val === undefined) return null;
  if (typeof val === 'string' || typeof val === 'number' || typeof val === 'boolean') {
    return val;
  }
  return String(val);
}

export class IfcParserService {
  private parser: IfcParser;
  private stores: Map<ModelId, IfcDataStore> = new Map();

  constructor() {
    this.parser = new IfcParser();
  }

  async parseModelBuffer(
    id: ModelId,
    buffer: ArrayBuffer,
    onProgress?: ProgressHandler
  ): Promise<ModelSummary> {
    onProgress?.('Analizando IFC', 10);

    const store = await this.parser.parseColumnar(buffer, {
      onProgress: (p) => {
        onProgress?.(p.phase, Math.min(95, p.percent));
      }
    });

    this.stores.set(id, store);

    const schemaIdentifier = store.schemaVersion || 'IFC2X3';
    const schema = toSchemaId(schemaIdentifier);
    const lengthUnitScale = store.lengthUnitScale ?? 1.0;
    const lengthUnitSource: 'ifc' | 'default' =
      store.lengthUnitScale !== undefined ? 'ifc' : 'default';

    let coordinateRef: CoordinateReference = {
      source: 'unknown',
      confidence: 'unknown',
      latLng: DEFAULT_GALICIA_CENTER
    };

    try {
      if (store.entities) {
        const georef = extractGeoreferencing(new Map(), new Map());
        if (georef.hasGeoreference && georef.projectedCRS) {
          const detected = detectCrsFromText(
            georef.projectedCRS.name,
            georef.projectedCRS.description
          );
          coordinateRef = {
            crsId: detected || georef.projectedCRS.name,
            source: 'ifc-map-conversion',
            confidence: detected ? 'verified' : 'unknown',
            localToMap: georef.transformMatrix,
            verticalDatum: georef.projectedCRS.verticalDatum
          };

          if (georef.mapConversion) {
            const pos = utm29NToLatLng(
              georef.mapConversion.eastings,
              georef.mapConversion.northings
            );
            coordinateRef.latLng = [pos.lat, pos.lng];
          }
        }
      }
    } catch {
      // Fallback georeferencing
    }

    const entityCount = store.entityCount || 0;

    onProgress?.('Completado', 100);

    return {
      schema,
      schemaIdentifier,
      entityCount,
      elementCount: entityCount,
      lengthUnitScale,
      lengthUnitSource,
      coordinateReference: coordinateRef,
      diagnostics: []
    };
  }

  removeStore(modelId: ModelId): void {
    this.stores.delete(modelId);
  }

  async getElementDetails(ref: ElementRef): Promise<ElementDetails | null> {
    const store = this.stores.get(ref.modelId);
    if (!store) return null;

    try {
      const entity = store.getEntity ? store.getEntity(ref.expressId) : null;
      const type = entity?.type || 'Elemento IFC';
      const attributes: Record<string, AttributeValue> = {};

      if (entity?.attributes && Array.isArray(entity.attributes)) {
        entity.attributes.forEach((attr, idx) => {
          if (attr !== null && attr !== undefined) {
            attributes[`Atributo #${idx + 1}`] = String(attr);
          }
        });
      }

      const rawPsets = store.getProperties ? await store.getProperties(ref.expressId) : [];
      const propertySets: PropertySetItem[] = [];

      if (Array.isArray(rawPsets)) {
        for (const pset of rawPsets) {
          const props = Array.isArray(pset.properties)
            ? pset.properties.map((p) => {
                const rawVal =
                  p.value !== undefined
                    ? p.value
                    : (p as { nominalValue?: unknown }).nominalValue ?? '';
                return {
                  name: p.name || 'Propiedad',
                  value: toPropertyScalar(rawVal),
                  type: p.type !== undefined ? String(p.type) : undefined
                };
              })
            : [];

          propertySets.push({
            name: pset.name || 'Conjunto de Propiedades',
            globalId: pset.globalId,
            properties: props
          });
        }
      }

      const globalId =
        typeof entity?.attributes?.[0] === 'string' ? entity.attributes[0] : undefined;
      const name =
        typeof entity?.attributes?.[2] === 'string' ? entity.attributes[2] : undefined;

      return {
        modelId: ref.modelId,
        expressId: ref.expressId,
        globalId,
        type,
        name,
        attributes,
        propertySets,
        materials: []
      };
    } catch (err) {
      console.error('Error fetching element details:', err);
      return null;
    }
  }

  async getSpatialTree(modelId: ModelId): Promise<SpatialTree | null> {
    const store = this.stores.get(modelId);
    if (!store) return null;

    const hierarchy = store.spatialHierarchy;
    const project = hierarchy?.project;
    const rawStoreys = hierarchy?.byStorey ? Array.from(hierarchy.byStorey.entries()) : [];

    const storeyNodes: SpatialNode[] = rawStoreys
      .map(([storeyId, elementIds]) => {
        const numStoreyId = Number(storeyId);
        const sortedIds = [...elementIds].map(Number).sort((a, b) => a - b);
        const elements: SpatialElement[] = sortedIds.map((elemId) => ({
          expressId: elemId,
          type: 'IfcProduct',
          via: 'contains' as const
        }));

        return {
          expressId: numStoreyId,
          type: 'IfcBuildingStorey',
          name: `Planta #${numStoreyId}`,
          children: [],
          elements
        };
      })
      .sort((a, b) => a.expressId - b.expressId);

    return {
      root: {
        expressId: Number(project?.expressId || 1),
        type: 'IfcProject',
        name: project?.name || 'Proyecto',
        children: storeyNodes,
        elements: []
      },
      unassigned: []
    };
  }
}

export const ifcParserService = new IfcParserService();
