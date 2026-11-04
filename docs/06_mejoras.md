# Mejoras del visor: optimizaciones y nuevas funciones

**Estado:** lista de trabajo, revisión 1 (2026-11-02). Elaborada por Claude y Gemini a partir del código en `83b8ccb`; Claude comprobó cada afirmación contra el código.
**Cuándo:** se empieza cuando el visor básico esté terminado, es decir, cuando la cola de `COORDINATION.md` (T-010 a T-062 y sus pendientes) quede cerrada o bloqueada solo por el propietario del proyecto.

## Reglas para cada mejora

- Cada mejora se convierte en una tarea `T-1xx` del tablero antes de empezar, con su criterio de aceptación.
- **Rendimiento:** se mide antes y después con los mismos modelos y el mismo navegador, y se anotan ambas cifras en el registro. Sin medición no se declara ninguna mejora.
- **Funciones:** primero las pruebas, con respuestas independientes del código. Las pruebas deben poder fallar (mutación desde una base limpia).
- Cambios que toquen el contrato (`docs/05_kernel_contract.md`) necesitan los dos acks antes de fusionar (R4).
- Sala limpia: ideas de lo que ofrecen otros visores, sí; su código, nunca.
- Ninguna mejora puede romper TC-001..008 en WebGPU, WebGL2 o sin adaptador.

Columnas: **Tam.** S, M o L. **Origen:** C Claude, G Gemini, C+G ambos. **Comprobado:** lo que se verificó en el código.

## A. Rendimiento y fluidez

Objetivo medible: ningún bloqueo del hilo principal por encima de 200 ms (hoy 222 ms en WebGPU y 279 ms en WebGL2 con 169 a 184 MB), y bajar la carga del modelo público de 184 MB (unos 30 s).

| ID | Mejora | Evidencia en el código | Tam. | Origen | Comprobado |
|---|---|---|---|---|---|
| A1 | Quitar el bucle por vértice que calcula los límites del modelo en JavaScript; usar los límites del kernel | `WebGpuRenderer.ts:661-667`, equivalente en `WebGl2Renderer.ts` | S | C+G | Sí. Medido el 2026-11-03: no es la causa de los bloqueos; el bucle no aparece en ningún tramo de más de 50 ms del hilo principal. Ganancia pequeña. **Hecho en T-101** (2026-11-04): los límites vienen del evento `complete` del kernel. Tiempo propio de `uploadBatchInternal` por carga, modelos de 184 y 108 MB en WebGPU y WebGL2: de 97 a 159 ms a 23 a 42 ms. El bloqueo más largo y el tiempo de carga no cambian de forma medible |
| A2 | No reconstruir toda la tabla de registros cada vez que llega un lote; añadir en su sitio | `WebGpuRenderer.ts:1114-1120` copia palabra a palabra todos los registros | S | C | Sí. Medido el 2026-11-03: tampoco aparece en ningún tramo de más de 50 ms. **Hecho en T-102** (2026-11-04): los registros del último modelo se añaden al final y solo se suben ellos; la textura de WebGL2 duplica su altura al crecer. `updateGpuSceneTables` por carga: de 73 a 138 ms a 7 a 18 ms (184 MB) y de 20 a 36 ms a 2 a 11 ms (108 MB), WebGPU y WebGL2 |
| A3 | Repartir la subida a GPU entre fotogramas con un presupuesto de tiempo por fotograma | Ruta de `addBatch` en ambos renderizadores | M | C | **Medido en T-103** (2026-11-04), no se construye: las subidas no son la causa. Modelo de 184 MB: las 476 llamadas a `bufferData` de una carga suman de 59 a 72 ms y ninguna pasa de 1,5 MB; en una carga WebGPU medida igual, ninguna escritura llega a 10 ms. En WebGL2, 2 de 10 cargas tuvieron llamadas sueltas a `bufferData` de un solo lote (la que se midió, 1,1 MB) que esperaron de 120 a 234 ms al proceso de la GPU. No se reprodujo con una ventana de 2560 × 1440 a escala 1,5, y un presupuesto de tiempo por fotograma no puede partir esa espera. La tarea más larga que queda, en todas las cargas de ese modelo y en los dos renderizadores, es la que atiende un mensaje del Worker al final de la carga: de 85 a 133 ms. **Reabierto por T-104:** cuando los lotes llegan sin pausa, esas esperas aumentan, así que el ritmo de subida sí importa. **Hecho en T-105** (2026-11-04), con un presupuesto en bytes, no en tiempo: los lotes se encolan al llegar y se suben al principio del siguiente fotograma, hasta 2 MB por fotograma, sin partir ningún lote. Esperas de 100 ms o más en una sola subida: modelo de 108 MB, 1 de 18 cargas antes (246 ms) y 0 de 18 después; modelo de 184 MB, 3 de 6 antes (101 a 239 ms) y 0 de 6 después. Sin cambio medible en el tiempo de carga (media de 6,3 a 6,5 s con 108 MB; de 14,2 a 15,3 s por carga con 184 MB, antes y después). El Worker pasa de esperar de 146 a 565 ms por carga a unos 30 ms, porque `addBatch` ya no retiene la siguiente petición. Sigue la tarea del final de la carga, de 90 a 140 ms, que no es una subida. Pendiente: los lotes que llegan antes de que la GPU esté lista se suben todos juntos al iniciarse |
| A4 | Separar índices opacos y transparentes en el kernel, no en el hilo principal | `WebGpuRenderer.ts:765-782` copia con `subarray`/`set` por registro en lotes mixtos | M | G | Sí. Toca el contrato |
| A5 | Canal con 1 o 2 lotes por adelantado entre Worker e hilo principal, en vez de petición y respuesta por lote | `WasmKernelAdapter.ts:133-145` espera cada `next-batch` | M | G | **Probado en T-104** (2026-11-04), no se fusiona. Antes, el Worker esperaba entre lotes de 154 a 580 ms por carga; pedir un lote por adelantado lo baja a unos 20 ms y acorta el flujo de 3,0 a 3,6 s a 2,8 a 3,4 s (108 MB), sin cambio medible en el tiempo de carga. Pero en WebGPU aparecen esperas de una sola escritura de 140 a 397 ms en 4 de 9 cargas, frente a 0 de 9 sin el cambio; en WebGL2, 2 de 9 frente a 3 de 9. La rama queda en local para probarla de nuevo junto con A3. **Probado de nuevo con A3 en T-105**, no se fusiona: los dos juntos tampoco dan esperas (0 de 18 y 0 de 6), pero con A3 solo el Worker ya espera unos 30 ms por carga, y pedir por adelantado acorta el flujo de 1 a 2 % más sin cambio en la carga |
| A6 | Descarte por frustum por lote; el kernel envía la caja de cada lote | `WebGpuRenderer.ts:1239-1257` dibuja todos los lotes visibles; los lotes no llevan caja, solo `MAX_BATCH_RADIUS` | M | C+G | Sí. Toca el contrato |
| A7 | Podar lotes con un rayo contra cajas antes del pase de selección | `WebGpuRenderer.ts:1341` en adelante dibuja toda la escena con scissor 1x1 | S | G | Sí. Depende de A6 |
| A8 | Descarte de caras traseras en mallas que el kernel marque cerradas | `cullMode: 'none'` en `WebGpuRenderer.ts:292,332,356`; `WebGl2Renderer.ts:1146` | M | C | Sí. Las mallas abiertas deben seguir sin descarte |
| A9 | Perfilar la carga del kernel con el modelo público de 184 MB (el 2026-11-03, en el mismo navegador: 11,6 a 13,3 s el kernel frente a 11,0 a 11,3 s IFClite) | Registro de T-061 | M | C | Cifras en el registro. Los demás modelos grandes ya cargan entre 1,8 y 3,7 veces más rápido que IFClite |
| A10 | Caché de geometría en OPFS o IndexedDB por hash del archivo: reabrir un modelo sin analizar ni evaluar | Ninguna | M | C | Función nueva |
| A11 | Compresión de vértices: posiciones de 16 bits respecto a la caja del lote, normales octaédricas | `GeometryBatch.ts` usa f32 | M | C | Sí. Toca el contrato |
| A12 | Omitir objetos diminutos mientras la cámara se mueve; detalle completo al parar | Ninguna | M | C | Función nueva |
| A13 | Instanciación en GPU de representaciones mapeadas repetidas | `batches.rs:37` expande cada instancia | L | C+G | Sí. Toca el contrato |
| A14 | Niveles de detalle para mallas lejanas y densas | Una sola resolución de evaluación | L | G | Sí |
| A15 | WebGL2: lectura de selección asíncrona (PBO y `fenceSync`) | `WebGl2Renderer.ts:1328`, `readPixels` síncrono de 1 píxel | S | G | Sí. El coste real está sin medir; prioridad baja |
| A16 | WebGL2: bloque uniforme para los desplazamientos por lote | `WebGl2Renderer.ts:1173,1211`: un `uniform3f` por lote; las matrices van una vez por fotograma | S | G | Corregido: el informe decía matrices por lote. Ganancia pequeña |

Paralelismo: GitHub Pages no envía COOP/COEP y el `coi-serviceworker` se quitó porque bloqueaba los mosaicos del IGN, así que no hay `SharedArrayBuffer` ni hilos WASM. Repartir un modelo entre varios Workers cuesta una copia del modelo por Worker.

## B. Modelos grandes: carreteras y ferrocarril

| ID | Mejora | Evidencia en el código | Tam. | Origen | Comprobado |
|---|---|---|---|---|---|
| B1 | Precisión de profundidad en WebGL2: búfer de 24 bits y reverse-Z deshecho por `z*2-w`. Con el plano cercano en 2 m, la resolución es de unos 3 cm a 1 km, 0,75 m a 5 km y 12 m a 20 km: capas y marcas viales parpadean en vistas generales. WebGPU usa `depth32float` con reverse-Z y no lo sufre | `WebGl2Renderer.ts:112,193` y el búfer por defecto pedido con `depth: true` (24 bits medidos en Chrome); `Camera.ts:153` | M | C | Sí, calculado; no visto en pantalla |
| B2 | Partir en varios lotes los elementos de más de unos 8 km (radio 4096 m). Hoy se dibujan con aviso `batch-radius-exceeded` y pierden precisión poco a poco, alrededor de 1 mm a 20 km | `batches.rs:8,270-276` | S | C | Sí |
| B3 | Prueba a escala de carretera con un modelo IFC4X3 público y largo: navegación, encuadre, plano lejano (`Camera.ts:154`, máximo de 50 km y 250 veces la distancia), estacas de alineación y planos frente al presupuesto de oclusión | Ningún modelo de carretera real probado | S | C | Pendiente |

## C. Nuevas funciones

| ID | Mejora | Evidencia en el código | Tam. | Origen | Comprobado |
|---|---|---|---|---|---|
| C1 | Panel de diagnósticos del modelo: los avisos de carga nunca llegan a la interfaz; solo el panel de planos muestra los suyos | Solo `SectionDxfPanel.tsx` lee `diagnostics` en `src/ui` | S | C+G | Sí |
| C2 | Planos de corte y caja de sección en 3D, por descarte en el shader | `shaders.ts` sin plano de corte; solo hay cortes 2D | M | C+G | Sí |
| C3 | Aislar, ocultar y modo fantasma o rayos X por selección, clase IFC o planta | Solo visibilidad binaria por elemento | S | C+G | Sí |
| C4 | Búsqueda y filtro por nombre, clase, GlobalId y valor de propiedad | Nada en `src/ui` | M | C+G | Sí |
| C5 | Medición: distancia, desnivel, área y ángulo, con ajuste a vértices y aristas | Nada en `src` | M | C+G | Sí |
| C6 | Táctil y lápiz: eventos de puntero, pellizco para zoom, dos dedos para desplazar | `WebGpuRenderer.ts:539-543` y `WebGl2Renderer.ts:570-574` solo escuchan el ratón | S | C+G | Sí |
| C7 | Navegación: órbita sobre el punto señalado, zoom a la selección, cubo de vistas, proyección ortográfica, primera persona | `Camera.ts` solo órbita sobre un objetivo y cuatro vistas fijas | M | C+G | Sí |
| C8 | Color por propiedad con leyenda | El shader solo alterna color base y selección | M | C+G | Sí |
| C9 | Vistas guardadas: cámara, visibilidad y selección | Nada en `src` | S | C+G | Sí |
| C10 | Incidencias BCF 3.0: importar y exportar temas, comentarios y puntos de vista | Nada en `src`; depende de C9 | L | C+G | Sí |
| C11 | Exportar mediciones a CSV por clase o planta | Nada en `src` | S | C | Sí |
| C12 | Comprobación IDS contra un archivo `.ids` | Nada en `src` | M | C+G | Sí |
| C13 | Comparar dos versiones de un modelo por GlobalId: añadidos, quitados, cambiados | Nada en `src` | L | C+G | Sí |
| C14 | Exportar la escena visible a glTF/GLB | Solo DXF y SVG | M | G | Sí |
| C15 | Planos: exportar a PDF, rótulos de espacios con nombre y área, cotas | `cad-slicer` exporta DXF R12 y SVG | M | C | Sí |
| C16 | Minimapa de planta con la posición y el cono de la cámara | El panel GIS es un mapa exterior | M | G | Sí |
| C17 | Aristas o contornos para leer mejor la geometría | No hay pase de aristas | M | C | Sí |
| C18 | Interfaz en gallego e inglés además de castellano | Textos en castellano en el código | M | C | Sí |
| C19 | Abrir un modelo público desde un parámetro de la URL, para compartir enlaces | Solo archivo local y muestra | S | C | Sí |
| C20 | Monitor de rendimiento: fotogramas, memoria GPU, lotes dibujados | `getDebugStats()` existe en ambos renderizadores y ningún componente lo usa | S | G | Sí |

## D. Robustez y calidad

| ID | Mejora | Evidencia en el código | Tam. | Origen | Comprobado |
|---|---|---|---|---|---|
| D1 | Pruebas de navegador en CI: TC-001..008 en WebGPU, WebGL2 y sin adaptador, con capturas comparadas | `package.json` no tiene script `test`; hoy son scripts sueltos | M | C+G | Sí |
| D2 | Banco de rendimiento en CI con modelos públicos; falla si empeora | Ninguno | M | C | Sí |
| D3 | Regresión visual de planos: SVG frente a referencias aprobadas | `check-drawing-request.mjs` solo comprueba el encuadre | M | G | Sí |
| D4 | Reintentar un modelo cuyo Worker cayó. La caída ya queda aislada en su modelo; falta el botón | `kernel.worker.ts:38-49`, `WasmKernelAdapter.ts:47` | S | G | Corregido: el informe lo daba por ausente |
| D5 | Matriz de navegadores: Firefox ya, Safari cuando haya equipo | Solo Chrome probado | S | C | Sí |

## E. Aplicación de escritorio

Último bloque: se empieza cuando todo lo anterior esté hecho. Un solo código y dos compilaciones: la web sigue publicándose desde `main` y la de escritorio se construye desde el mismo commit.

| ID | Mejora | Evidencia en el código | Tam. | Origen | Comprobado |
|---|---|---|---|---|---|
| E1 | Aplicación de escritorio con Tauri (MIT/Apache) para Windows: abrir un `.ifc` con doble clic, una sola instancia que recibe los archivos siguientes. Usa WebView2 (Chromium), así que WebGPU funciona; donde no lo haya, cae a WebGL2 | Ninguna | M | C | Diseño |
| E2 | Puerto de archivos: abrir desde una ruta y guardar con el diálogo nativo, con versión navegador y versión escritorio elegidas en `composition.ts`, sin ramas `if (escritorio)` repartidas por el código | Descarga por enlace en `SectionDxfPanel.tsx:23` | S | C | Sí |
| E3 | Compilación de escritorio: `base` propia, porque `vite.config.ts:5` apunta a la ruta de Pages | `vite.config.ts:5` | S | C | Sí |
| E4 | Archivos grandes por el protocolo de archivos locales de Tauri, no por su canal de mensajes. Probar primero con el modelo público de 184 MB | Ninguna | S | C | Riesgo principal del puerto |
| E5 | Sin conexión: estado claro en el panel GIS (mosaicos de OpenStreetMap e IGN, `LeafletService.ts:27,33`) y ocultar o incluir la descarga de muestras (`App.tsx:136`) | `LeafletService.ts:27,33`, `App.tsx:136` | S | C | Sí |
| E6 | Publicación: una etiqueta de versión hace que CI construya el instalador y lo suba a GitHub Releases; el actualizador de Tauri lo ofrece al abrir la aplicación. La firma de código (certificado de pago) la decide el propietario del proyecto | Ninguna | M | C | Diseño |
| E7 | macOS y Linux, cuando haya equipos donde compilar y probar | Ninguna | M | C | Diseño |
| E8 | Opcional y más adelante: kernel nativo en vez de WASM, sin el límite de 4 GB por modelo y con carga en varios hilos. Necesita un camino nuevo para la geometría hasta el renderizador; semanas de trabajo | Ninguna | L | C | Diseño |

## Orden propuesto

1. **Fluidez primero, medida:** A1, A2, A3, después A5 y A4. Objetivo: bloqueos por debajo de 200 ms.
2. **Lo que un usuario echa de menos al abrir el visor:** C1, C3, C6, C2, C4, C5.
3. **Carreteras:** B1, B2, B3.
4. **Escala:** A6, A7, A8, A9, A10.
5. **Trabajo en equipo:** C9, C10, C12, C13.
6. **Resto,** según lo que pidan los usuarios.
7. **Aplicación de escritorio:** E1 a E6, después E7. E8 solo si la velocidad con modelos grandes pasa a ser la queja principal.

D1 y D2 conviene tenerlos antes de la fase 4, para que cada optimización tenga su medición en CI.
