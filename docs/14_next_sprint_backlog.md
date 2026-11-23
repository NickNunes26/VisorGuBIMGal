# 14 — Planificación y backlog del siguiente sprint

**Repositorio de trabajo:** `NickNunes26/VisorGuBIMGal`  
**Organización:** pistas independientes en paralelo para evitar conflictos entre escritorio nativo, kernel/DXF y frontend.

---

## Pista 0 (completada): Choques LandXML → DXF y round-trip STEP
*Especificación completa: [`12_clash_alignment_dxf.md`](./12_clash_alignment_dxf.md)*

| ID | Elemento | Tamaño | Estado |
|---|---|---|---|
| **F1** | **Parser LandXML de alineación horizontal** (`Line`, `Curve`, `Spiral` clotoide, `grads`/grados/radianes, `staStart` negativo, `Northing Easting`, `localToMap⁻¹`, proyección `PK` + tangente) | M | **Hecho** (`t170-landxml-test`: 7/7) |
| **F2** | **Máxima penetración y motor de banda por Elemento A** (punto de máx. penetración CSG/fallback en `clash-detector`, banda `[s_min, s_max]` vs `depth = 0` longitudinal, agrupación por `PK` a `20 mm` por defecto) | M | **Hecho** (`clash-detector`: 13/13) |
| **F3** | **Corte rápido sin oclusión + DXF maestro sin solapes** (poda 3D banda-AABB en `wasm-api`, modo sin oclusión en `cad-slicer`, normalización `(0,0)` en metros, capas `CLASH_*`, `GUBIM_META`/`GUBIM_ORIG` y `BLOCK`s round-trip, rejilla `clashes_all.dxf`) | L | **Hecho** (`clash_dxf`: 3/3, `test:drawings`: 31/31) |
| **F4** | **Worker por lotes e interfaz en `ClashPanel`** (Worker único secuencial con progreso/cancelación, `.zip` con `individual/` + `clashes_all.dxf` + `summary.csv`, vista previa SVG instantánea por choque) | M | **Hecho** (`t171-clash-dxf-export-test`: 2/2, `test:unit`: 18/18) |
| **F5** | **Reimportación DXF y parcheador quirúrgico STEP IFC** (lectura `GUBIM_META`/`GUBIM_ORIG`/`INSERT`/`POLYLINE`/`LWPOLYLINE`, Nivel 1 `IfcLocalPlacement` + Nivel 2 `IfcSweptDiskSolid`, re-comprobación automática de choques + log CSV) | L | **Hecho** (`roundtrip_dxf_step`: 2/2) |

---

## Siguiente sprint — Pista A: Carcasa de escritorio soberana (sin MPL-2.0)
*Sin solape con frontend ni crates del kernel (`FilePort`). Ver fila E9 en [`06_mejoras.md`](./06_mejoras.md).*

| ID | Elemento | Tamaño | Dependencias |
|---|---|---|---|
| **E9** | **Sustituir Tauri v2 por carcasa propia en Rust puro (Win32 + WebView2)** (`windows` MIT/Apache-2.0 + `webview2-com` MIT, instancia única `CreateMutexW`/`WM_COPYDATA`, diálogos nativos, `WebResourceRequested` para `.ifc` grandes, instalador NSIS, pasa `cargo deny check`) | M | Ninguna |

---

## Siguiente sprint — Pista B: Rediseño UI/UX y migración a SolidJS
*Especificación completa: [`13_ui_redesign_solidjs.md`](./13_ui_redesign_solidjs.md)*

| ID | Elemento | Tamaño | Dependencias |
|---|---|---|---|
| **U0** | **Extracción de almacenes `src/ui/stores/*` y contrato `data-testid` E2E** (descomponer `App.tsx`, corregir las 3 fugas hexagonales, mantener 16 unit + 24 E2E en verde) | M | Ninguna |
| **U1** | **Carcasa SolidJS y nuevo diseño CAD/BIM** (React 18 → SolidJS + CSS Modules + `tokens.css`, barra superior ligera, raíl 44 px + paneles plegables, dock único inferior, `BimViewer` y paneles base) | L | U0 |
| **U2** | **Migración de paneles de coordinación, 2D y GIS a SolidJS** (`SectionDxfPanel`, `SearchPanel`, `ComparePanel`, `ClashPanel` incl. F4/F5, `IdsPanel`, `BcfPanel`, `GisMapPanel`, `MeasureOverlay`, `Minimap`, `ViewCube`) | L | U1 |

---

## Siguiente sprint — Pista C: Persistencia, accesibilidad e idiomas
*Se apoya directamente en los almacenes de U0 y el nuevo sistema de diseño de U1.*

| ID | Elemento | Tamaño | Dependencias |
|---|---|---|---|
| **P1** | **Persistencia de sesión en IndexedDB** (vistas guardadas, incidencias BCF, reglas de choques y preferencias de interfaz) | M | U0 |
| **A1** | **Auditoría de accesibilidad y atajos CAD** (navegación por teclado `F`/`H`/`I`/`M`/`Esc`/`1–6`, `:focus-visible`, `aria-live`, contraste WCAG AA) | M | U1 |
| **C18** | **Idiomas adicionales (`es`, `gl`, `en`)** (catálogos en gallego e inglés, selector conectado a `setLocale`, eliminar textos fijos en español en `AutoRenderer`/`WasmKernelAdapter`) | M | U1 |

---

## Orden de ejecución recomendado
1. ~~**Pista 0:** **F1 → F2 → F3 → F4 → F5**~~ (**Completada** — pendiente solo revisión en sesión dedicada y `test:e2e` en navegador).
2. **Ola 1 (en paralelo):** **E9** (escritorio soberano) + **U0** (extracción de almacenes) + **P1** (IndexedDB).
3. **Ola 2:** **U1 → U2** (rediseño CAD + SolidJS) integrando directamente **A1** y **C18** sobre los nuevos componentes.
