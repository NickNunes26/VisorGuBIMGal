# Plan de Trabajo Paralelo para Equipo de +10 Desarrolladores

**Documento:** Distribución de Tareas, Contratos de Interfaz y Gobernanza Técnica  
**Proyecto:** Visor Web BIM/GIS basado en **IFClite** (`@ifc-lite/*`)  
**Base técnica:** [`SPEC_IFCLITE_BIM_GIS_VIEWER.md`](./SPEC_IFCLITE_BIM_GIS_VIEWER.md) y [`RIESGOS.md`](./RIESGOS.md)  
**Fecha:** 2026-10-17  
**Objetivo:** Coordinar el trabajo concurrente de 10 a 12 desarrolladores sobre el desarrollo del visor con **IFClite** como motor principal, garantizando cero conflictos en Git y manteniendo el aislamiento de arquitectura hexagonal que permitirá en el futuro la transición hacia un kernel propio.

---

## 1. Principio Fundamental: Arquitectura Hexagonal y Trabajo Desacoplado

Para que más de 10 desarrolladores trabajen en paralelo sobre la base de IFClite sin colisiones:
1. **Límites estrictos de Puertos y Adaptadores:** La interfaz de dominio (`src/domain/`) y los casos de uso (`src/application/`) definen los contratos. Nadie importa `@ifc-lite` fuera de `src/infrastructure/ifclite/`.
2. **Propiedad de código por directorios (Code Ownership):** Cada desarrollador tiene asignado un módulo específico y una rama aislada.
3. **Desarrollo guiado por contratos y Mocks:** Los desarrolladores de UI, GIS y exportación pueden trabajar desde el primer día utilizando datos sintéticos o adaptadores simulados (*mocks*) sin esperar a que el adaptador de IFClite esté completado.

---

## 2. Mapa de Squads y Asignación de Tareas (12 Desarrolladores)

```
┌────────────────────────────────────────────────────────────────────────┐
│                        COORDINACIÓN GENERAL                            │
├───────────────────┬───────────────────┬────────────────────────────────┤
│  SQUAD 1: IFCLITE │ SQUAD 2: PLANOS   │   SQUAD 3: GIS 2D/3D           │
│  Y MOTOR 3D       │ Y EXPORTACIÓN DXF │   Y COORDENADAS                │
│  Dev 01, 02, 03   │ Dev 04, 05        │   Dev 06, 07                   │
├───────────────────┴───────────────────┴────────────────────────────────┤
│  SQUAD 4: CASOS DE USO Y FEDERACIÓN   │  SQUAD 5: UI Y PANELES         │
│  Dev 08, 09                           │  Dev 10, 11                    │
├───────────────────────────────────────┴────────────────────────────────┤
│  SQUAD 6: CI/CD, DESPLIEGUE EN VERCEL Y QA (Dev 12)                    │
└────────────────────────────────────────────────────────────────────────┘
```

---

### SQUAD 1: Adaptador IFClite & Renderizado 3D

#### **DEV 01: Adaptador Core de IFClite, Parser y Memoria**
* **Ubicación exclusiva:** `src/infrastructure/ifclite/parser/` y `src/infrastructure/ifclite/store/`
* **Misión:** Encapsular las APIs de `@ifc-lite/core` para lectura de archivos IFC, carga a través de Web Workers y ciclo de vida de memoria.
* **Entregables:**
  - Carga de modelos desde `ArrayBuffer` / `File` sin retener buffers duplicados en RAM (prevención de OOM, ver `RSK-03`).
  - Extracción de metadata básica, esquema IFC (IFC2X3, IFC4) y unidades (`m`, `mm`).
  - Implementación del método de liberación `disposeModel()` para desasignar memoria WASM y recursos.
* **Contrato TypeScript:**
  ```typescript
  export interface IfcParserPort {
    parseModel(modelId: string, file: File): Promise<LoadedModelHandle>;
    disposeModel(modelId: string): Promise<void>;
  }
  ```

---

#### **DEV 02: Adaptador de Renderizado WebGPU & Diagnóstico Fallback**
* **Ubicación exclusiva:** `src/infrastructure/ifclite/renderer/` y `src/ui/viewer/`
* **Misión:** Integrar el renderer WebGPU oficial de IFClite y preparar el fallback a WebGL.
* **Entregables:**
  - Inicialización y montaje del canvas de renderizado de IFClite.
  - Implementación de controles de cámara: órbita, pan, zoom, encuadre de modelo seleccionado y vistas ortogonales (N, S, E, O, planta).
  - Detección de soporte de WebGPU con pantalla de diagnóstico amigable y prototipado del fallback WebGL/Three.js (`NFR-03`).
* **Contrato TypeScript:**
  ```typescript
  export interface BimRendererPort {
    init(canvas: HTMLCanvasElement): Promise<RendererCapabilities>;
    loadGeometry(modelId: string, handle: LoadedModelHandle): Promise<void>;
    setCameraView(view: "orbit" | "top" | "front" | "fit-all"): void;
    highlightElement(ref: ElementRef | null): void;
  }
  ```

---

#### **DEV 03: Adaptador 2D de IFClite y Planos de Corte**
* **Ubicación exclusiva:** `src/infrastructure/ifclite/drawing-2d/`
* **Misión:** Implementar la generación de secciones y vistas 2D utilizando `@ifc-lite/drawing-2d`.
* **Entregables:**
  - Corte interactivo 3D según plano de sección ($Ax + By + Cz + D = 0$).
  - Invocación de `@ifc-lite/drawing-2d` para generar la geometría de planta o sección a una cota dada.
  - Mapeo de la salida de IFClite al contrato neutral `DrawingDocument` (capas `IFC-CUT`, `IFC-PROJECTION`, `IFC-HATCH`).
* **Contrato TypeScript:**
  ```typescript
  export interface DrawingGeneratorPort {
    generateSection(modelIds: string[], planeParams: SectionPlane): Promise<DrawingDocument>;
  }
  ```

---

### SQUAD 2: Exportación DXF/SVG & Persistencia Local

#### **DEV 04: Motor de Exportación DXF & SVG**
* **Ubicación exclusiva:** `src/infrastructure/export/`
* **Misión:** Gestionar la exportación del dibujo vectorial a formatos descargables compatibles con CAD.
* **Entregables:**
  - Adaptador para el exportador DXF R12 de IFClite con codificación Windows-1252.
  - Generador SVG complementario para visualización directa en navegador con estilos y grosores (`RSK-04`).
  - Evaluación técnica de `@tarikjabiri/dxf` como alternativa para soportar capas y entidades avanzadas.
  - Mecanismo de descarga local de archivos con nombres parametrizados (`<modelo>_seccion_<fecha>.dxf`).
* **Contrato TypeScript:**
  ```typescript
  export interface ExportPort {
    exportToDxf(doc: DrawingDocument): Blob;
    exportToSvg(doc: DrawingDocument): string;
  }
  ```

---

#### **DEV 05: Persistencia Local y Sesiones (IndexedDB)**
* **Ubicación exclusiva:** `src/infrastructure/storage/` y `src/domain/session/`
* **Misión:** Almacenar el estado de trabajo del usuario en el navegador sin backend.
* **Entregables:**
  - Servicio IndexedDB para persistir la lista de modelos cargados, visibilidad, colores asignados y transformaciones CRS.
  - Serialización y deserialización de la sesión a formato JSON para importar/exportar proyectos locales.
  - Guardado de la última posición de cámara y configuración de capas GIS.
* **Contrato TypeScript:**
  ```typescript
  export interface SessionStoragePort {
    saveSession(session: SessionState): Promise<void>;
    loadSession(): Promise<SessionState | null>;
    exportSessionJson(session: SessionState): string;
  }
  ```

---

### SQUAD 3: GIS 2D/3D & Georreferenciación

#### **DEV 06: Módulo GIS 2D (Leaflet) & Servicios OGC**
* **Ubicación exclusiva:** `src/infrastructure/gis-leaflet/` y `src/ui/map/`
* **Misión:** Integrar el mapa 2D ligero para contextualizar los modelos IFC georreferenciados.
* **Entregables:**
  - Componente Leaflet 2D con selector de mapas base (OpenStreetMap, ortofotos).
  - Soporte para añadir URLs de capas WMS y WMTS externas con manejo de fallos por CORS.
  - Vista sincronizada (split screen o minimapa flotante) entre la vista 3D y el mapa 2D.
* **Contrato TypeScript:**
  ```typescript
  export interface GisMapPort {
    initMap(containerId: string): void;
    addWmsLayer(url: string, layerName: string): Promise<void>;
    setMarker(coords: [number, number], label: string): void;
    drawPolygon(rings: [number, number][]): void;
  }
  ```

---

#### **DEV 07: Servicio de Coordenadas, CRS & Proyecciones**
* **Ubicación exclusiva:** `src/domain/coordinates/` y `src/infrastructure/gis-cesium/`
* **Misión:** Realizar la georreferenciación rigurosa de los archivos IFC y preparar la extensión a CesiumJS.
* **Entregables:**
  - Lectura de entidades `IfcProjectedCRS` e `IfcMapConversion` a través del adaptador de IFClite.
  - Cálculo de matriz de transformación `localToMap` con `proj4js` para convertir coordenadas locales a EPSG:4326/UTM.
  - Definición del marcador y caja envolvente orientada (OBB) en el mapa Leaflet (`RSK-05`).
  - Módulo opcional base de CesiumJS para visualización en globo 3D (Fase 2).

---

### SQUAD 4: Casos de Uso, Dominio y Federación

#### **DEV 08: Gestor de Federación de Múltiples Modelos**
* **Ubicación exclusiva:** `src/domain/models/` y `src/application/use-cases/federation/`
* **Misión:** Coordinar la apertura simultánea de múltiples modelos sin colisiones de identificadores.
* **Entregables:**
  - Registro de modelos `ModelRecord` con claves compuestas `(modelId, expressId)`.
  - Casos de uso: `LoadIfcUseCase`, `RemoveModelUseCase`, `ToggleVisibilityUseCase`, `IsolateElementsUseCase`.
  - Gestión de transformaciones visuales manuales para alinear modelos que carezcan de origen compartido.

---

#### **DEV 09: Extracción de Propiedades, Filtros y Árbol Espacial**
* **Ubicación exclusiva:** `src/domain/models/` y `src/application/use-cases/properties/`
* **Misión:** Consultar la información BIM no geométrica de los modelos cargados.
* **Entregables:**
  - Caso de uso `GetSpatialTreeUseCase`: jerarquía `IfcProject -> IfcSite -> IfcBuilding -> IfcBuildingStorey`.
  - Caso de uso `GetElementPropertiesUseCase`: extracción de atributos y Property Sets (`IfcPropertySet`).
  - Filtro por clases IFC (`IfcWall`, `IfcDoor`, `IfcWindow`, `IfcSlab`).

---

### SQUAD 5: Interfaz de Usuario, Paneles y Herramientas

#### **DEV 10: Paneles UI, Árbol Espacial e Inspector de Propiedades**
* **Ubicación exclusiva:** `src/ui/panels/` y `src/ui/components/`
* **Misión:** Construir la interfaz de control, accesible y reactiva.
* **Entregables:**
  - Lista de modelos cargados con controles de visibilidad, color y botón de retirar.
  - Componente de Árbol Espacial virtualizado para navegar por pisos y elementos sin congelar el navegador.
  - Tabla inspectora de propiedades seleccionadas (GlobalId, tipo, dimensiones, materiales).
  - Indicadores visuales de progreso de carga y gestión de errores amigables.

---

#### **DEV 11: Herramientas de Navegación, Gizmo de Sección y Vista 2D**
* **Ubicación exclusiva:** `src/ui/viewer/tools/` y `src/ui/drawings/`
* **Misión:** Desarrollar los controles interactivos sobre la escena y la previsualización de planos.
* **Entregables:**
  - Gizmo o slider interactivo para ajustar la posición y altura de los planos de corte 3D.
  - Modal o panel de previsualización 2D para ver los planos generados por IFClite antes de exportar.
  - Controles de zoom/pan en la vista previa 2D y botones de descarga directa en DXF y SVG.

---

### SQUAD 6: Calidad, CI/CD, Despliegue en Vercel y QA

#### **DEV 12: CI/CD, Despliegue en Vercel, Pruebas y Fixtures**
* **Ubicación exclusiva:** `.github/workflows/`, `vercel.json`, `tests/`
* **Misión:** Asegurar la infraestructura de despliegue y la calidad técnica continua.
* **Entregables:**
  - Configuración de despliegue en Vercel con cabeceras `Cross-Origin-Opener-Policy: same-origin` y `Cross-Origin-Embedder-Policy: credentialless` (`RSK-02`).
  - Pipeline de GitHub Actions ejecutando pruebas unitarias con Vitest y typecheck en cada PR.
  - Pruebas E2E automatizadas con Playwright (verificación de carga de IFC y ausencia de fugas de memoria).
  - Suite de modelos IFC de prueba clasificados por tamaño y complejidad geométrica.
  - Validación de los archivos DXF generados en lectores CAD reales (LibreCAD/QCAD).

---

## 3. Matriz de Dependencias y Mocks para Trabajo Autónomo

| Desarrollador | Módulo asignado | Puede avanzar en paralelo usando |
|---|---|---|
| **Dev 01** | `ifclite/parser` | Archivos `.ifc` de prueba locales. |
| **Dev 02** | `ifclite/renderer` | Canvas HTML y el visor de ejemplo de `@ifc-lite`. |
| **Dev 03** | `ifclite/drawing-2d` | Modelos de prueba oficiales del paquete de IFClite. |
| **Dev 04** | `export/dxf-svg` | Objetos `DrawingDocument` sintéticos en memoria. |
| **Dev 05** | `storage/indexeddb` | Objetos `SessionState` simulados en TypeScript. |
| **Dev 06** | `gis-leaflet` | Coordenadas fijas y servicios WMS públicos (IGN/Catastro). |
| **Dev 07** | `domain/coordinates` | Matrices de prueba y cálculos con `proj4js`. |
| **Dev 08** | `application/federation` | Mocks tipados de `LoadedModelHandle`. |
| **Dev 09** | `application/properties` | Datos JSON simulados de árboles espaciales IFC. |
| **Dev 10** | `ui/panels` | Store reactivo simulado con Zustand o similar. |
| **Dev 11** | `ui/viewer/tools` | Canvas de prueba con caja 3D básica. |
| **Dev 12** | `ci-cd/vercel/qa` | Repositorio base, scripts de build y tests iniciales. |

---

## 4. Convención de Ramas y Flujo Git

1. **Ramas por Desarrollador:**
   - Formato: `feature/<squad>-dev<numero>-<nombre-tarea>`
   - Ejemplos:
     - `feature/squad1-dev01-ifclite-parser`
     - `feature/squad2-dev04-dxf-export`
     - `feature/squad3-dev06-leaflet-wms`
     - `feature/squad5-dev10-spatial-tree-ui`
2. **Políticas de Integración Continua:**
   - Ninguna PR puede tocar archivos de otro módulo sin aprobación previa.
   - Todo commit debe pasar `npm run lint` y `npm run typecheck`.
   - Vercel generará una URL de vista previa automática (*Preview Deployment*) para cada Pull Request.
