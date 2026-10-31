# 05 — Contrato del Kernel y del Renderizador

**Estado:** Borrador v1, pendiente de aceptación de Gemini y DeepSeek (T-010).  
**Propósito:** Fuente única de los límites entre kernel, renderizador y aplicación.  
**Código normativo:** [`src/domain/kernel/`](../src/domain/kernel/) (`KernelPort.ts`, `RendererPort.ts`, `GeometryBatch.ts`). Si este documento y los tipos discrepan, es un error que debe corregirse.

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

## 2. Sistemas de coordenadas

| Sistema | Unidad | Precisión | Definición |
|---|---|---|---|
| **Modelo** | metros | `f64` | Coordenadas IFC tras aplicar unidades y `IfcObjectPlacement`. **Z arriba.** |
| **Local de lote** | metros | `f32` | `modelo − originShift`. Lo único que llega a los vértices. |
| **Mapa** | metros | `f64` | `localToMap × modelo`, matriz 4×4 por columnas, derivada de `IfcMapConversion`. |

**`originShift`:** lo calcula el kernel antes de emitir geometría y lo devuelve `openModel`. Es el centro de la caja de las ubicaciones de los `IfcProduct`, redondeado al metro. Debe ser determinista.

**Reglas (RSK-01):**
- Ninguna coordenada absoluta llega a la GPU.
- El renderizador calcula en la CPU, con `f64` y en cada fotograma, la traslación de cada modelo **relativa a la cámara** (`originShift − cámara`), y solo entonces pasa a `f32`. Un desplazamiento fijo entre modelos no basta: dos modelos federados pueden estar a cientos de kilómetros y `f32` perdería precisión.
- El kernel conserva Z arriba, porque corte y GIS trabajan en ese espacio. La conversión a la convención de la cámara es interna del renderizador.
- `DrawingOptions` y los límites (`ModelBounds`) usan coordenadas de modelo.

---

## 3. Lotes de geometría

Definidos en `GeometryBatch.ts`.

| Campo | Tipo | Contenido |
|---|---|---|
| `positions` | `Float32Array` | xyz por vértice, coordenadas locales de lote |
| `normals` | `Float32Array` | xyz unitario por vértice. Las aristas vivas duplican vértices |
| `indices` | `Uint32Array` | Triángulos, base cero dentro del lote |
| `meshTable` | `Uint32Array` | 7 palabras por registro: `expressId, firstVertex, vertexCount, firstIndex, indexCount, rgba, flags` |

- `rgba` es `0xRRGGBBAA`, sRGB sin premultiplicar.
- Un elemento puede tener varios registros. Cada índice de un registro apunta dentro de su rango de vértices.
- Banderas: `Transparent`, `Degraded` (hay un diagnóstico asociado), `HiddenByDefault` (`IfcSpace`).
- `findBatchError` valida este formato en pruebas y con datos simulados.
- **Los lotes se transfieren, nunca se clonan** (RSK-06). Tras `addBatch`, el renderizador sube los datos a la GPU y no conserva copia en JS (RSK-03).

---

## 4. Identidad y errores

- **Identidad:** siempre `ElementRef` = `(modelId, expressId)`. Selección, visibilidad y resaltado no pueden usar un `expressId` suelto (TC-002).
- **`KernelError`:** fallo de toda una operación, por ejemplo un archivo ilegible.
- **`KernelDiagnostic`:** problema de un elemento, con su `expressId`. Un elemento nunca hace fallar el modelo (R8): se omite o se degrada, y se informa.
- Cerrar un modelo durante `streamGeometry` termina el flujo sin error.

---

## 5. Planos 2D

- `generateDrawing` se ejecuta dentro del kernel y devuelve un `DrawingId`. La UI ya no maneja mallas.
- `DrawingCut` (dentro de `DrawingOptions`) sustituye a `DrawingCutType`: `{kind:'plan', elevation}` o `{kind:'section', axis:'x'|'y', position}`. La antigua `section-z` del espacio Y arriba corresponde ahora al eje `y`. El adaptador temporal de IFClite traduce esa diferencia, y T-011 elimina los tipos antiguos.
- **Seguridad del SVG:** `SectionDxfPanel.tsx:188` lo inserta con `dangerouslySetInnerHTML` y los nombres vienen del archivo IFC, que no es de confianza. El exportador debe escapar todo texto y no emitir `<script>`, `<foreignObject>`, atributos `on*` ni referencias externas.
- **DXF R12:** textos saneados para CP1252 (RSK-04).

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
```

**Corrección a `docs/03` sección 4:** una vista `Float32Array` sobre `wasmMemory.buffer` queda inválida en cuanto la memoria WASM crece, y la memoria de WASM no se puede transferir entre hilos. Por tanto, el recorrido real es:

1. `next_batch` deja el lote en memoria WASM.
2. El Worker lo copia a `ArrayBuffer` propios **antes de cualquier otra llamada a WASM**.
3. Los transfiere al hilo principal, sin más copias.

Hay exactamente una copia y ninguna serialización. Los datos no geométricos, poco frecuentes (detalles, árbol, resumen), pueden viajar como JSON.

---

## 7. Migración desde el código actual

| Hoy | Después |
|---|---|
| `ModelRecord.originalBytes` | Se elimina: `openModel` toma posesión del búfer |
| `bimEngine.modelMeshes`, `getModelMeshes` | Se eliminan: nadie retiene mallas en JS |
| `getSpatialHierarchy` (`byStorey`) | `getSpatialTree`, árbol real con nombres |
| `Atributo #n` posicional | `attributes` con nombres EXPRESS del esquema |
| `BimEngine.isWebGpuSupported` en `Header` | `initialize` informa del fallo; la cabecera deja de depender del motor |
