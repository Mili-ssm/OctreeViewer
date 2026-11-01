# Render y carga a escala — plan

Estado: activo · 30-10-2026

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Progreso (30-10-2026):** plan redactado tras la [evaluación](../records/archive/RENDER_ASSESSMENT_2026-10-30.md)
y enmendado tras su pre-mortem (abajo). R0 hecha; R1 parcial (T0, T3a, T4 y T5 hechas); R2 hecha, con el worker de
construcción encendido por defecto desde esta rama (`?buildWorker=0` / `buildWorker: false` lo apagan, [ADR 0018](../adr/0018-worker-de-construccion-por-defecto.md)).
Preguntas P1–P8 resueltas el 30-10-2026 ([requisitos](../requirements.md)). Orden acordado: resto de R1 → R4 → R3 ([ADR 0028](../adr/0028-r4-antes-que-r3.md)) → R5 → V7 final.
Nada de R3–R5 está implementado: las decisiones están registradas, el código no ha cambiado.

**Objetivo (propietario, 30-10-2026):** cargar nubes de cientos de GB y dibujarlas con vóxeles de
1–8 px en pantalla, con una carga que se perciba continua: sin saltos a grueso al mover la cámara,
sin huecos y sin bloqueos de frame.

**Criterios de éxito medibles** (fijados por el coordinador; se revisan con la línea base): intervalo
entre respuestas de frame del worker al hilo principal con **p95 < 33 ms y máximo < 100 ms** durante
la carga y durante una órbita de 10 s; la etapa LOD publicada nunca sube durante una órbita; sin
aumento de nodos sin cubrir ni de píxeles vacíos frente a la línea base.
Las puertas de escala RQ-03, RQ-04, RQ-05 y RQ-11 (tiempos al primer frame y al objetivo, PSS ≤ 8 GiB,
demostración a 500 GB) están en [requisitos](../requirements.md#requisitos-de-calidad).

**Arquitectura de partida:** un worker de visor lee bloques del `.oct` local, selecciona un corte
LOD por error en pantalla, voxeliza en CPU todo el corte para la vista actual y sube las instancias
a la GPU (cubos instanciados). Todos los mensajes pasan por una cola serie del worker
(`viewer-worker.js:530-533`), así que un rebuild de 1 s retrasa la respuesta del frame siguiente
aunque no ocurra dentro de `renderFrame`. Bloqueos estructurales: voxelización por vista en CPU (B1)
y pirámide LOD sin densidades intermedias (B2); problemas acotados: B3–B7.

## Restricciones globales

- `scripts/check.sh` en verde al cerrar cada tarea; sin cambiar el nightly ni añadir `#![feature]`.
- Sin cambios de formato ni de salida del conversor en R0–R2.
- R4 y R5 cambian el formato dentro de V7 ([ADR 0022](../adr/0022-alcance-y-congelacion-de-v7-final.md)).
- Contrato visible: ningún vóxel por encima del objetivo de píxeles salvo lo permitido por la densidad de la fuente (`REGIONAL_VOXEL_LOD.md`); la retención del último corte completo se conserva.
- **Puertas por contadores deterministas** (`sceneBuilds`, `voxelSourcePoints`, bloques cargados, nodos sin cubrir, expulsiones de pendientes, ms de selección en bench nativo); el tiempo de pared es orientativo con n = 3. Matriz completa solo en R0 y en la medida final; por tarea, una ejecución de humo en Majadas.
- **Serie sobre `lod_dataset.rs`/`bindings/lod.rs`**: una tarea cada vez en ese código (orden abajo). El host sirve la raíz del worktree que se mide; cada registro anota el SHA del `.wasm` y del dataset.
- Antes de cronometrar: ningún `cargo`, `rustc` ni otro navegador activo (`pgrep`) y `loadavg` anotado.
- Registros nuevos fechados con el día de ejecución, en español; commits convencionales en inglés con `Co-Authored-By`; sin push.

## Review Focus

1. **Órbita continua durante la carga**: el corte visible nunca vuelve a una etapa más gruesa (R1-T1a).
2. **Regiones recién expuestas al girar**: no quedan vacías mientras se reconstruye (R1-T2, margen de frustum).
3. **Jerarquías grandes**: selección y prioridad con 10⁵–10⁶ nodos sin coste cuadrático, con el mismo corte (R1-T3a).
4. **Cámara cerca del suelo**: el plano cercano no recorta y la selección no explota (R1-T5).
5. **Thrash en la ruta async**: las expulsiones de bloques pendientes se miden y no aumentan (R0, R1-T0).

## Enmiendas del pre-mortem (30-10-2026)

| # | Enmienda | Tareas |
| --- | --- | --- |
| E1 | Paso 0 de R0: demostrar adaptador hardware (librerías Vulkan mapeadas, `adapter.info`, `gpu_busy_percent`); Chrome solo si da adaptador hardware; driver BiDi existente | R0 |
| E2 | Métrica principal: intervalo entre respuestas de frame (p95/máx) y espera en cola; `renderCallMs` secundaria | R0, R1-T2 |
| E3 | Registro con marca de tiempo de cada mensaje LOD (etapa, vóxeles sobredimensionados, nodos sin cubrir) y captura de píxeles vacíos; órbita guionizada de 10 s | R0 |
| E4 | Puertas por contadores deterministas; humo en Majadas por tarea | todas |
| E5 | Tiempo de decodificación en la ruta async y ocupación del worker por mensaje | R0 |
| E6 | R1-T1 se divide: T1a sin reinicio de etapa al mover (corte retenido y prioridad a regiones sin cubrir); T1b histéresis tras P5 | R1-T1 |
| E7 | R1-T2 = agrupar llegadas y menos reconstrucciones; mantener la escena anterior y ampliar el frustum de construcción con margen | R1-T2 |
| E8 | R1-T3 se divide: T3a refactor con test dorado del orden exacto de `node_ids` (vía prioridad y vía alternativa) a 10⁵ y 10⁶; T3b reordenación intencional medida aparte | R1-T3 |
| E9 | Q6 con la profundidad mínima en vista sobre las 8 esquinas del AABB; test fuera de eje en las esquinas del viewport | R1-T4 |
| E10 | R1-T5 medido aparte tras T4; compara puntos y bloques seleccionados a 1 m y respeta el presupuesto | R1-T5 |
| E11 | Detector de thrash en la ruta async (o métrica de expulsiones de pendientes por minuto) antes de T3b y T1a | R1-T0 |
| E12 | Orden serie: T0 → T3a → T4 → T5 → T1a → T2 → T3b → T6 | proceso |
| E13 | R2 es una puerta de decisión: si la decodificación es < 20 % del tiempo ocupado del worker, se sustituye por un spike de worker de construcción que posee la caché y transfiere buffers de instancias (ataca B1 sin decisión del propietario) | R2 |
| E14 | Tests de integración en navegador (`lod-reuse`, `lod-session`) con `pkg` recién construido en la lista de cada tarea R1 | R1 |

**Criterios de parada:** adaptador llvmpipe, swiftshader o desconocido → sin tiempos, solo
resultados funcionales; rango entre rondas > 30 % en humo → más rondas o solo contadores; tras T2 el
p95 de respuestas no cambia aunque baje `renderCallMs` → no se declara mejora de latencia y se pasa a
E13; tras T1a/T4/T5 suben nodos sin cubrir, píxeles vacíos, expulsiones de pendientes o aparece
`SourceDetailBudgetExceeded` → revertir esa tarea; el test dorado de T3a difiere → parar; R0 > 2 días
de máquina → matriz reducida a Majadas y Belvedere 8 px.

---

## Fase R0: arnés y línea base

### R0-T1: arnés reproducible de carga y órbita

**Archivos:** `examples/web-host/probe-*.mjs` (extender `probe-cache-camera.mjs` o crear `probe-orbit-load.mjs`), `examples/web-host/viewer-worker.js` (contadores e instrumentación), `docs/records/benchmarks/RENDER_BASELINE_<aaaa-mm-dd>.md`.

- [x] **Paso 0 (E1, 30-10-2026):** Firefox 159 headless obtiene un adaptador **software** (`adapter.info` vacío, `gpu_busy` ≤ 3 %): sin tiempos válidos. Chrome 155 con `--use-angle=vulkan` da `{vendor: "amd", architecture: "rdna-3"}` (hardware); sin chromedriver, se conduce con `playwright-core` (paso 1b).
- [x] **Paso 1 (E2, E3, E5; `46b76dd`, `4cb3ef7`, `ede7e0a`):** instrumentación: intervalo de respuestas de frame y espera en cola, ocupación del worker por tipo de mensaje, tiempo de decodificación async, registro con marca de tiempo de cada mensaje LOD, nodos sin cubrir, píxeles vacíos, expulsiones de pendientes; órbita guionizada de 10 s.
- [x] **Paso 1b (`bae9a21`, `671f2da`):** backend de Chrome en `probe-orbit-load.mjs` con adaptador hardware; reconversión de Belvedere y Majadas a `target/render-data/` (los `.oct` de `data/datasets` son de un formato provisional que el visor ya no abre).
- [x] **Paso 2 (`7a6746a`, [registro](../records/benchmarks/RENDER_BASELINE_2026-10-30.md)):** Chrome con adaptador hardware, 18 ejecuciones A (`ede7e0a`) frente a B (`87ef649`). Ninguna rama cumple los criterios de latencia (p95 de respuestas ≈ 1 s en Majadas y 5–7 s en Belvedere durante la carga; en órbita tras el objetivo cada frame reconstruye: 480 ms y 2,6 s); la etapa nunca sube en órbita; sin aumento de nodos sin cubrir ni píxeles vacíos en B. La voxelización ocupa el 98–99,5 % del worker y la decodificación el 0,3–0,9 % (E13 → worker de construcción). **Regresión en B** (más lento hasta el objetivo) diagnosticada ([registro](../records/benchmarks/RENDER_T4_DIAGNOSIS_2026-10-30.md)): el corte era idéntico; cambió el orden de carga de pendientes (las hojas densas se intercalaban pronto). Corregido en `d4604b7` (prioridad = error en pantalla ÷ puntos fuente) y verificado en vivo: Majadas 4 px 56,8 → 50,8 s y Belvedere 8 px 253 → 159 s frente a A, sin empeorar nodos sin cubrir, píxeles vacíos ni expulsiones. Línea base: Majadas 4 px, Belvedere 8 px y 4 px (tope 300 s), 3 rondas; registro con mediana, rango y SHA de `.wasm` y datasets; commit.

## Fase R1: mejoras sin decisiones del propietario (orden E12)

### R1-T0: detector de thrash en la ruta async (E11)

**Archivos:** `crates/viewer_wasm/src/lod_dataset.rs` y tests.

- [x] **`1981f87`:** el detector observa también las llegadas async (sin reiniciarse en cada inserción); contadores `pendingTargetEvictions` y `cacheCycleDetected`. Test: una secuencia de cargas async que expulsa bloques pendientes del objetivo se detecta igual que en la ruta síncrona (pausa del refinado o contador expuesto en `lod`).

### R1-T3a: selección sin coste cuadrático, mismo corte (Q3, Q5)

**Archivos:** `crates/viewer_core/src/spatial_lod_priority.rs`, `crates/viewer_core/src/lod_transition.rs`, `crates/viewer_wasm/src/{lod_dataset.rs, bindings/lod.rs}`.

- [x] **Parte de `viewer_core` (`4ce23e8`, `dac3a38`):** tests dorados (2k, 10⁵ en `check.sh`, 10⁶ ignorado; Replacement y Additive; cuatro cámaras; vía de prioridad y alternativa forzada) sin cambios tras el refactor; frontera enlazada O(hijos) por expansión y comprobaciones de descendientes sin anidamiento; selección por prioridad a 10⁶ nodos de 1,4–1,7 s a 0,4–0,6 s (Replacement) y de 45–96 ms a 12–28 ms (Additive). Residual: liberar candidatos bloqueados sigue siendo lineal (para T3b).
- [x] **Parte de `viewer_wasm` (`4321f17`, `48e7524`):** selección y plan una vez por actualización (antes 2 selecciones y 3 `prepare`); decisiones fijadas con un test dorado; contadores `selectionsPerUpdate` y `preparesPerUpdate`.

### R1-T4: métrica de error única (Q6, B7, E9)

**Archivos:** `crates/viewer_wasm/src/lod_dataset.rs`, `crates/viewer_render_wgpu/src/camera.rs`.

- [x] **`87ef649`:** `OrbitCamera::projected_error_pixels` única para selección y prioridad (profundidad mínima de las 8 esquinas del AABB; sin `geometric_error`); en 120 cámaras aleatorias nunca selecciona más puntos (menos en 47); arista ≤ objetivo en celdas dentro del frustum (fuera de él el factor fuera de eje puede exceder ~2 %). Una función de error en pantalla para selección y prioridad: espaciado del nodo ÷ profundidad mínima en vista de las 8 esquinas del AABB, con el factor de proyección actual. Tests: arista proyectada ≤ objetivo también fuera de eje en las esquinas del viewport; igual o menos puntos seleccionados que hoy con la misma cámara.

### R1-T5: plano cercano adaptativo (Q8, E10)

**Archivos:** `crates/viewer_render_wgpu/src/camera.rs`.

- [x] **`29dfdeb`:** plano cercano = max(profundidad del centro − 1,2·radio, far/10⁵) (Depth32Float sin Z invertida); la métrica LOD usa `lod_min_depth()` (la fórmula antigua) y no cambia: a 1 m del suelo de una nube de 41 km el plano baja de 35,5 m a 0,71 m con la misma selección (0 %). Test: cámara a 1 m del suelo de una nube de decenas de km sin recorte de nodos visibles; puntos y bloques seleccionados antes y después comparados; presupuesto de la fuente respetado; picking existente sin regresión.

### R1-T1a: no reiniciar la etapa al mover (Q1, E6)

**Archivos:** `crates/viewer_wasm/src/bindings/camera.rs`, `crates/viewer_wasm/src/{lod_dataset.rs, scene_builder.rs}` y tests.

- [x] **01-11-2026:** viewer y worker de construcción conservan la etapa al mover; regiones sin cubrir primero, sin histéresis. Regresiones nativas antes/después, probes Chrome y humo Majadas en ambas rutas; `check.sh` 24/24. [Evidencia y límites](../records/benchmarks/RENDER_R1_T1A_01-11-2026.md).

### R1-T2: menos reconstrucciones y bordes cubiertos (Q2, E7) — cancelada por el propietario (30-10-2026)

Con el worker de construcción, la coalescencia «gana la última» ya agrupa llegadas y vistas (reconstrucciones −50 %); la tarea solo afectaba a la ruta sin worker, que el worker por defecto deja de usar ([ADR 0018](../adr/0018-worker-de-construccion-por-defecto.md)). No se implementa.

**Archivos:** `examples/web-host/viewer-worker.js`, `crates/viewer_wasm/src/{bindings/render.rs, lod_dataset.rs}`.

- [ ] Llegadas agrupadas: una reconstrucción por ventana de tiempo; en un cambio de cámara se dibuja la escena anterior hasta la nueva; el frustum de construcción se amplía con un margen para que el giro no deje bordes vacíos. Medida: `sceneBuilds` y p95 de respuestas de frame (si no baja, criterio de parada → E13).

### R1-T3b: prioridad solo sobre pendientes (Q4)

- [x] **Reordenación hecha en `d4604b7`** (error por punto fuente; medida aparte en vivo).
- [x] **01-11-2026:** índice de propietarios por bloque; puntuación solo sobre propietarios pendientes, con el mismo orden. Regresión de 10.004 a 2 proyecciones; humo Majadas antes/después sin empeorar cobertura, expulsiones ni digest, sin mejora temporal demostrada. [Costes, verificación y límites](../records/benchmarks/RENDER_R1_T3B_01-11-2026.md).

### R1-T6: jerarquía construida en wasm desde el reader (Q7, B6 parcial) — movida a R5 (jerarquía paginada)

**Archivos:** `crates/viewer_wasm/src/{async_format_reader.rs, lod_metadata.rs, lod_dataset.rs}`, `examples/web-host/viewer-worker.js`.

- [ ] Sin descriptor JS por nodo; test con 1 M nodos sintéticos: ms de apertura y memoria JS/wasm frente a hoy.

### R1-T1b: modos de px (RF-06, RF-07)

- [ ] Pasa a ser la [Tarea 14 del plan de R3](render-r3-cubos-en-gpu.md#tarea-14-modos-de-px-antes-r1-t1b). Ya no espera a P5 (resuelta): modo fijo progresivo (predeterminado) y dinámico que apunta a 16,7 ms por frame con histéresis ([ADR 0019](../adr/0019-cubos-por-nodo-residentes-en-gpu.md), [requisitos](../requirements.md)).

### R1-T7: medida después de R1

- [x] **01-11-2026:** matriz R0 en `135c147`: respuestas p95 ≤ 17,6 ms y máx ≤ 19,2 ms; objetivo Majadas 18,8 → 10,8 s y Belvedere 8 px 66,8 → 48,8 s; nodos sin cubrir finales 0; expulsiones menores que en las revisiones de control anteriores a T1a/T3b. Edad de escena p95 en Belvedere 8 px 120 → 2932 ms: coste aceptado de T1a (cada vista exige una construcción en la etapa fina); lo resuelve R3 ([diagnóstico](../records/benchmarks/RENDER_R1_T7_SCENE_AGE_01-11-2026.md)). [Registro](../records/benchmarks/RENDER_R1_T7_01-11-2026.md).

## Fase R2: worker de construcción (B1, B4; decidido por E13)

La línea base fija la puerta E13: la decodificación es el 0,3–0,9 % del tiempo ocupado del worker y
la voxelización el 98–99,5 %, así que R2 es el **worker de construcción**. Diseño (arquitecto,
30-10-2026): un segundo worker posee el lector async, la caché decodificada, el planner, la etapa y
la voxelización; recibe vistas versionadas (`gen` por dataset/color, `epoch` por cámara/viewport/px)
con «gana la última»; transfiere el `ArrayBuffer` de instancias (40 B/vóxel) al worker de render, que
dibuja la última escena (los cubos están en coordenadas de mundo: girar sin reconstruir es válido
hasta que cambie la profundidad terminal). Mismos bytes por construcción: mismas funciones sin GPU
(`voxel_grid.rs`, `camera.rs`, `viewer_core`) y cámara transferida como `Float64Array`. Coste:
escena desfasada entre B y 2B (B = tiempo de construcción: 0,48 s en Majadas, 2,6 s en Belvedere
8 px). La opción pasa a **encendida por defecto** (P7/P8 resueltas; [ADR 0018](../adr/0018-worker-de-construccion-por-defecto.md)).

- [x] **R2-T1, costura y digest (`fb18073`–`638eb63`; 48 digests dorados sin cambios; actualización conducible en nativo):** tests dorados de digest FNV-64 de las instancias (fixtures, Replacement/Additive, 4 cámaras, 3 px); `ViewProjection` en `camera.rs` (sin GPU) a la que delega `SurfaceRenderer`; `update_view_progressive` devuelve la escena y quien llama la sube. Aceptación: digests sin cambios; humo Majadas con `sceneBuilds` y `voxelSourcePoints` ±5 %.
- [x] **R2-T2, `WasmSceneBuilder` sin GPU (`014bd20`, `a546941`; digests idénticos entre visor y constructor en cada paso):** binding nuevo; colorización y etapa en funciones compartidas; test de navegador que da la misma secuencia de bloques y vistas a la ruta actual y a la nueva y exige digests idénticos.
- [x] **R2-T3, `build-worker.js` y protocolo (`8c6d0ee`, `763f379`; opción `buildWorker`, encendida por defecto desde esta rama y `?buildWorker=0` la apaga; humo en Majadas con Chrome: órbita p50/p95 16,7/17,5 ms y edad de escena p95 484 ms; límites: modos de color por atributo no cubiertos en este modo y paquete `file://` sin probar en navegador):** opción `buildWorker` (encendida por defecto); empaquetado `file://`; métricas nuevas (`sceneAgeMs`, `epochLag`, `viewsCoalesced`, `scenesDropped`, memoria por worker). Aceptación: Majadas 4 px y Belvedere 8 px con p95 de respuestas < 33 ms y máx < 100 ms en carga y órbita; sin subir nodos sin cubrir, expulsiones de pendientes ni `oversized`; digest final en reposo igual a la ruta actual.
- [x] **R2-T4, subida en trozos:** no hace falta (`sceneApplyMs` máx 1,6 ms en R2-T5).
- [x] **R2-T5 (`165be08`, [registro](../records/benchmarks/RENDER_BUILD_WORKER_2026-10-30.md)):** con el worker de construcción, intervalo de respuestas p95 ≤ 17,6 ms y máx ≤ 33,9 ms en carga y órbita (criterio cumplido); tiempo hasta el objetivo Majadas 48,8 → 18,8 s y Belvedere 157 → 66,8 s; cobertura, píxeles vacíos y expulsiones iguales; edad de escena p95 0,1–0,5 s (máx ~2,6 s en Belvedere); PSS +40–60 MiB. Cerrado después (`18463cd`–`e6b8665`): color por atributo en este modo con paridad bit a bit, digest de escena expuesto y registrado en reposo por el arnés, y respaldo al modo actual cuando el navegador no permite workers anidados (Chrome `file://` los bloquea; Firefox `file://` funciona). La diferencia de digest en reposo observada en Majadas resultó ser del arnés, no del worker ([diagnóstico](../records/benchmarks/RENDER_BUILD_WORKER_DIGEST_2026-10-30.md)): sin worker, la órbita encolada en el hilo principal se aplicaba después de fijar la cámara de reposo; con el arnés corregido (`2a52a1f`), OFF y ON dan el mismo digest (`1f1652bb…`). P8 queda desbloqueada. **R2-T5, registro:** matriz R0 con `sceneAgeMs` y margen de frustum 0/5/10° (parámetro, 0 por defecto); ya no decide R2b (absorbida por R4); queda pendiente.
- [ ] **R2b (absorbida por R4, [ADR 0020](../adr/0020-piramide-lod-densa.md)):** caché por nodo de celdas agregadas para descender por celdas en lugar de por puntos (hoy 1 100–2 700 puntos leídos por vóxel); exactitud: fusión en el orden de `node_ids`, sumas exactas, exclusiones alineadas a celdas.

## Fases decididas (pendientes de plan detallado)

| Fase | Contenido decidido | ADR | Puertas |
| --- | --- | --- | --- |
| R3 | Cubos 3D por nodo, independientes de la vista y residentes en GPU; modos literal (predeterminado) y continuidad por nube; modos de px fijo y dinámico; sustituye a la voxelización CPU por corte (B1) | [0019](../adr/0019-cubos-por-nodo-residentes-en-gpu.md), [0027](../adr/0027-celdas-por-nodo-en-pool-paginado.md); [plan](render-r3-cubos-en-gpu.md) | RQ-01, RQ-02, RQ-05 |
| R4 | Pirámide LOD densa (un nivel por ×2 de densidad, hojas ≤ 16–64 k puntos, espaciado por nodo en el índice; absorbe R2b) + rejilla global determinista + CRS: reconversión de datasets (B2) | [0020](../adr/0020-piramide-lod-densa.md), [0021](../adr/0021-rejilla-global-determinista-y-crs.md), [0022](../adr/0022-alcance-y-congelacion-de-v7-final.md) | RQ-02, RQ-07, RQ-09 |
| R5 | Jerarquía paginada para > 2 M nodos y apertura sin índice completo, local y remota; incluye la antigua R1-T6 | [0023](../adr/0023-fuentes-locales-y-remotas.md) | RQ-04 |
| V7 final | Congelación del formato cuando se cumpla RQ-07 | [0022](../adr/0022-alcance-y-congelacion-de-v7-final.md) | RQ-07, RQ-11 |

Cada fase necesita su propio plan detallado (`writing-plans` + pre-mortem) antes de implementarse.
