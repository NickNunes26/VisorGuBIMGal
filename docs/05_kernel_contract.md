# 05 — Contrato del Kernel y del Renderizador

**Estado:** Borrador v1, revisión 3, tras dos rondas de revisión de DeepSeek; pendiente de aceptación final (T-010). `CONTRACT_VERSION` sigue en 1 porque ninguna versión se ha publicado aún.  
**Propósito:** Fuente única de los límites entre kernel, renderizador y aplicación.  
**Código normativo:** [`src/domain/kernel/`](../src/domain/kernel/), [`DrawingTypes.ts`](../src/domain/drawings/DrawingTypes.ts) y [`ModelRecord.ts`](../src/domain/models/ModelRecord.ts). Si este documento y los tipos discrepan, es un error que debe corregirse.

Cualquier cambio requiere aceptación previa de los otros dos agentes (regla R4 de `COORDINATION.md`) e incrementar `CONTRACT_VERSION` si rompe la compatibilidad.

---

## 1. Arquitectura

```text
Hilo principal                         Web Worker
┌──────────────────────────┐           ┌──────────────────────────┐
│ UI React                 │           │ Adaptador del kernel     │
│   │                      │  postMsg  │   │                      │
│ KernelPort (adaptador) ──┼──────────▶│ wasm-api (Rust/WASM)     │
│   │          ▲           │◀──────────┼─ lotes transferidos      │
│   ▼          │           │           └──────────────────────────┘
│ RendererPort (WebGPU)    │
└──────────────────────────┘
```

1. La UI solo conoce `KernelPort` y `RendererPort`. Ningún tipo de implementación los atraviesa.
2. El kernel no conoce el renderizador. El renderizador no conoce IFC: recibe lotes e identidades.
3. La app conecta ambos: `for await` sobre `streamGeometry` y `addBatch` por cada lote.
4. Primera versión: WASM de un solo hilo dentro de un Worker. Los hilos serán una optimización posterior, tras medir.

---

## 2. Coordenadas y unidades

| Sistema | Unidad | Precisión | Definición |
|---|---|---|---|
| **Modelo** | metros | `f64` | Coordenadas IFC tras aplicar unidades y ubicaciones. **Z arriba.** |
| **Local de lote** | metros | `f32` | `modelo − batch.origin`. Lo único que llega a los vértices. |
| **Mapa** | metros | `f64` | `localToMap × modelo`. |

### Origen por lote (RSK-01)

Cada lote lleva su propio `origin` en `f64`. No existe un origen por modelo: la geometría puede estar lejos de su ubicación (terrenos, alineaciones, coordenadas absolutas incrustadas), así que no se puede fijar antes de evaluarla.

- El origen de un lote es el centro de la caja del primer elemento que contiene, redondeado al metro, con la mitad alejándose de cero.
- Ningún vértice puede quedar a más de `MAX_BATCH_RADIUS` (4096 m) del origen. El kernel abre un lote nuevo cuando un elemento no cabe.
- Si un solo elemento supera ese radio, se emite con su propio lote, la bandera `Degraded` y un diagnóstico de precisión.
- El renderizador calcula en la CPU, con `f64` y en cada fotograma, `batch.origin − cámara` para cada lote, y solo entonces pasa a `f32`. Ninguna coordenada absoluta llega a la GPU.
- El kernel conserva Z arriba, porque corte y GIS trabajan en ese espacio. La conversión a la convención de la cámara es interna del renderizador.

### Unidades

- `lengthUnitScale` se resuelve desde `IfcProject.UnitsInContext`. El prefijo de `IfcSIUnit` se aplica primero. En `IfcConversionBasedUnit`, `ConversionFactor` es un `IfcMeasureWithUnit`: el factor es `ValueComponent` por la escala de `UnitComponent`. Si falta la unidad, se asume metro, `lengthUnitSource` es `'default'` y se emite un aviso.
- `PLANEANGLEUNIT`, `AREAUNIT` y `VOLUMEUNIT` se resuelven igual dentro de los crates del kernel.
- Las longitudes de geometría, elevaciones y espesores de capa se entregan en metros.
- **Propiedades y cantidades conservan su valor bruto** y su unidad efectiva en `unit`. Nunca se convierten usando `lengthUnitScale`. La unidad sale de `IfcPropertySingleValue.Unit` o de `IfcPhysicalSimpleQuantity.Unit`; si falta, del tipo de medida a través de `UnitsInContext`. Solo se omite si nada la resuelve, con un diagnóstico.
- `schema` es canónico (`IFC2X3`, `IFC4`, `IFC4X3`) y `schemaIdentifier` guarda el texto original. Cualquier otro esquema produce `unsupported-schema`.

### `localToMap`

Matriz 4×4 por columnas, de metros de modelo a metros de mapa, desde `IfcMapConversion`. La fórmula normativa de IFC trabaja en unidades del archivo y de mapa, así que el kernel la encierra entre dos conversiones:

```text
a = metros por unidad de longitud del archivo (lengthUnitScale)
b = metros por unidad de MapUnit; si falta, la unidad de longitud del proyecto
θ = atan2(XAxisOrdinate, XAxisAbscissa), con 0 si faltan
λ = Scale; si falta, a/b
(x, y, z) = coordenadas de modelo en metros, divididas entre a

E = b·(λ·(x·cosθ − y·sinθ) + Eastings)
N = b·(λ·(x·sinθ + y·cosθ) + Northings)
H = b·(λ·z + OrthogonalHeight)
```

- `Scale` no se descarta: convierte unidades, pero también puede llevar un factor de escala de cuadrícula.
- Si falta `Scale`, el esquema indica 1, pero eso multiplicaría por mil un archivo en milímetros con mapa en metros. Se usa `a/b` y se emite un diagnóstico informativo.
- `IfcMapConversionScaled` sigue el orden `R·F·S`: los factores multiplican los ejes sin rotar (`λ·FactorX·x`, `λ·FactorY·y`, `λ·FactorZ·z`). Cualquier otro `IfcCoordinateOperation` emite un diagnóstico; nunca se usa la identidad en silencio.
- T-022 añade una prueba numérica de referencia. Fuente: `buildingSMART/IFC4.x-development`, `IfcMapConversion.md`, `IfcMapConversionScaled.md` e `IfcProjectedCRS.md`.
- `TrueNorth` no se aplica aparte: `localToMap` ya contiene la rotación. El campo `trueNorth` es informativo.
- `latLng` son grados decimales desde `IfcSite.RefLatitude/RefLongitude`, o derivados de un CRS conocido. El kernel no supone Galicia ni UTM 29N.
- `crsId` es un resultado de búsqueda, no un atributo IFC. `confidence: 'verified'` exige coincidencia exacta del CRS.

---

## 3. Lotes de geometría

Definidos en `GeometryBatch.ts`.

| Campo | Tipo | Contenido |
|---|---|---|
| `origin` | `[f64; 3]` | Coordenadas absolutas de modelo. Vértice absoluto = `origin + position` |
| `positions` | `Float32Array` | xyz por vértice, coordenadas locales de lote |
| `normals` | `Float32Array` | xyz unitario por vértice. Las aristas vivas duplican vértices |
| `indices` | `Uint32Array` | Triángulos, base cero dentro del lote |
| `meshTable` | `Uint32Array` | 7 palabras por registro: `expressId, firstVertex, vertexCount, firstIndex, indexCount, rgba, flags` |

- `rgba` es `0xRRGGBBAA`, sRGB sin premultiplicar.
- Un elemento puede tener varios registros. Cada índice de un registro apunta dentro de su rango de vértices.
- Banderas: `Transparent`, `Degraded` (hay un diagnóstico asociado), `HiddenByDefault` (`IfcSpace`).
- `findBatchError` valida este formato en pruebas y con datos simulados. El radio máximo lo garantiza el kernel y no se comprueba ahí, porque un elemento `Degraded` puede superarlo.
- **Los lotes se transfieren, nunca se clonan** (RSK-06). Tras `addBatch`, el renderizador sube los datos a la GPU y no conserva copia en JS (RSK-03).
- Los lotes son solo para el renderizador. `cad-slicer` trabaja dentro del kernel con la geometría evaluada de `ifc-geometry`, combinando origen y posición en `f64`.

---

## 4. Identidad, datos y errores

- **Identidad:** siempre `ElementRef` = `(modelId, expressId)`. Selección, visibilidad y resaltado no pueden usar un `expressId` suelto (TC-002).
- **`elementCount`:** `IfcProduct` con representación, sin `IfcOpeningElement`.
- **Árbol espacial:** hijos y elementos en `expressId` ascendente, porque las relaciones IFC no tienen orden. `via` distingue `contains` de `references`. Los productos sin contenedor van en `unassigned`. `IfcSpace` aparece como nodo. `IfcOpeningElement` no aparece.
- **Materiales y tipo:** se resuelven `IfcRelDefinesByType` e `IfcRelAssociatesMaterial` en el elemento y en su tipo. La asignación del elemento prevalece.
- **Atributos:** claves con el nombre EXPRESS y valores `AttributeValue`, sin `any`.
- **`KernelError`:** fallo de toda una operación, por ejemplo un archivo ilegible.
- **`KernelDiagnostic`:** problema de un elemento, con su `expressId`. Un elemento nunca hace fallar el modelo (R8): se omite o se degrada, y se informa.
- Cerrar un modelo durante `streamGeometry` termina el flujo sin error.

---

## 5. Planos 2D

- `generateDrawing` se ejecuta dentro del kernel y devuelve `GeneratedDrawing`, con su marco y diagnósticos. La UI ya no maneja mallas.
- **Marco:** `project` toma las coordenadas absolutas de modelo como compartidas por todos los modelos, igual que la vista 3D. `map` pasa cada modelo por su `localToMap`; los modelos sin georreferencia se omiten con un diagnóstico. Todos los modelos de un marco `map` deben compartir `crsId` y `verticalDatum`: el que difiera se omite con el diagnóstico `map-frame-crs-mismatch`. `GeneratedDrawing.crsId` es ese CRS común si está verificado, y queda indefinido si no. Las posiciones de corte se expresan en metros del marco. El CRS del plano sale del dibujo, no de `models[0]`.
- `DrawingCut` sustituye a `DrawingCutType`: `plan` mira hacia −Z; `section` indica eje, posición y `look`. `depth` limita la proyección; si se omite, llega hasta el final de la geometría. La antigua `section-z` del espacio Y arriba corresponde ahora al eje `y`. El adaptador temporal de IFClite traduce esa diferencia, y T-011 elimina los tipos antiguos.
- **Generar y exportar:** `DrawingOptions` decide qué se calcula. `exportDrawing` solo serializa y puede omitir capas ya calculadas, nunca añadirlas. Las líneas ocultas requieren `includeProjection`.
- **Tramas:** la trama procede de `IfcFillAreaStyleHatching` si existe. Si no, una regla determinista por material o capa, que define T-050.
- **Ciclo de vida:** un dibujo guarda sus propios datos 2D y sobrevive a `closeModel`. `discardDrawing` es idempotente. Exportar un dibujo descartado produce `drawing-not-found`.

### Seguridad del SVG

`SectionDxfPanel.tsx:188` lo inserta con `dangerouslySetInnerHTML` y los nombres vienen del archivo IFC, que no es de confianza. El exportador usa una **lista blanca**:

- Elementos permitidos: `svg`, `g`, `path`, `line`, `polyline`, `polygon`, `circle`, `text`, `tspan`.
- Atributos: un conjunto fijo y numérico o de estilo, generado por el propio exportador. Los textos IFC solo aparecen como texto escapado (`& < > " '`), nunca como nombre de elemento o de atributo.
- Se eliminan los caracteres de control no válidos en XML 1.0.
- Sin `<style>`, `<use>`, `<image>`, `<a>`, `<animate>`, `<set>`, `<foreignObject>`, `href`, `xlink:href`, atributos `on*` ni entidades externas.

### DXF R12

- `content` es `Uint8Array`, ya codificado. La descarga usa `new Blob([content])` sin conversión de texto.
- `$ACADVER = AC1009`, `$DWGCODEPAGE = ANSI_1252`, finales de línea CRLF.
- Todo valor que entre en un código de grupo se sanea: textos, nombres de capa, bloque y estilo, comentarios 999 y nombre de archivo. Se eliminan CR, LF, NUL y controles, y nunca se emiten los bytes indefinidos `0x81`, `0x8D`, `0x8F`, `0x90` y `0x9D`.
- Los caracteres fuera de CP1252 se sustituyen por `?`. Nunca se emite UTF-8 bajo `ANSI_1252`. T-050 incluye una prueba con un nombre de capa no representable.
- Los nombres de capa cumplen las reglas de R12, con truncado sin colisiones.

---

## 6. Límite WASM

Interno entre `wasm-api` y su Worker. Puede cambiar sin tocar `KernelPort`. Propietario: Claude (T-033), incluido el Worker en `src/infrastructure/kernel/`.

```rust
#[wasm_bindgen]
impl Model {
    #[wasm_bindgen(constructor)]
    pub fn open(bytes: &[u8]) -> Result<Model, JsError>;
    pub fn summary_json(&self) -> String;
    /// Modelo de extracción: el Worker pide el siguiente lote cuando puede enviarlo.
    pub fn next_batch(&mut self, max_triangles: u32) -> Option<BatchView>;
    pub fn element_details_json(&self, express_id: u32) -> Option<String>;
    pub fn spatial_tree_json(&self) -> Option<String>;
}

#[wasm_bindgen]
impl DrawingStore {
    pub fn generate(&mut self, models: Vec<ModelHandle>, options_json: &str) -> Result<String, JsError>;
    pub fn export(&self, drawing_id: &str, config_json: &str) -> Result<Vec<u8>, JsError>;
    pub fn discard(&mut self, drawing_id: &str);
}
```

**Corrección a `docs/03` sección 4:** una vista `Float32Array` sobre `wasmMemory.buffer` queda inválida en cuanto la memoria WASM crece, y la memoria de WASM no se puede transferir entre hilos. Por tanto, el recorrido real es:

1. `next_batch` deja el lote en memoria WASM.
2. El Worker lo copia a `ArrayBuffer` propios **antes de cualquier otra llamada a WASM**.
3. Los transfiere al hilo principal, sin más copias.

Hay exactamente una copia y ninguna serialización. Los datos no geométricos, poco frecuentes (detalles, árbol, resumen, opciones), pueden viajar como JSON.

---

## 7. Migración desde el código actual

| Hoy | Después |
|---|---|
| `ModelRecord.originalBytes` | Se elimina: `openModel` toma posesión del búfer |
| `bimEngine.modelMeshes`, `getModelMeshes` | Se eliminan: nadie retiene mallas en JS |
| `getSpatialHierarchy` (`byStorey`) | `getSpatialTree`, árbol real con nombres |
| `Atributo #n` posicional | `attributes` con nombres EXPRESS del esquema |
| `BimEngine.isWebGpuSupported` en `Header` | `initialize` informa del fallo; la cabecera deja de depender del motor |
| Campos opcionales de `PropertyItem` | El kernel los rellena siempre; T-011 decide si pasan a obligatorios |
