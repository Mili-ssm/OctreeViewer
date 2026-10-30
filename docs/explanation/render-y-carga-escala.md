# Render y carga a escala — plan

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Estado (30-10-2026):** plan redactado tras la [evaluación](../records/archive/RENDER_ASSESSMENT_2026-10-30.md)
y enmendado tras su pre-mortem (abajo). R0 (arnés con Chrome y GPU hardware, datasets reconvertidos), R1-T0, R1-T3a y R1-T4 hechas; línea base tomada (regresión de B en diagnóstico; R2 apunta al worker de construcción). R0–R2 avanzan sin decisiones del propietario; R1-T1b y R3–R5
esperan las [preguntas pendientes](preguntas-propietario.md).

**Objetivo (propietario, 30-10-2026):** cargar nubes de cientos de GB y dibujarlas con vóxeles de
1–8 px en pantalla, con una carga que se perciba continua: sin saltos a grueso al mover la cámara,
sin huecos y sin bloqueos de frame.

**Criterios de éxito medibles** (fijados por el coordinador; se revisan con la línea base): intervalo
entre respuestas de frame del worker al hilo principal con **p95 < 33 ms y máximo < 100 ms** durante
la carga y durante una órbita de 10 s; la etapa LOD publicada nunca sube durante una órbita; sin
aumento de nodos sin cubrir ni de píxeles vacíos frente a la línea base.

**Arquitectura de partida:** un worker de visor lee bloques del `.oct` local, selecciona un corte
LOD por error en pantalla, voxeliza en CPU todo el corte para la vista actual y sube las instancias
a la GPU (cubos instanciados). Todos los mensajes pasan por una cola serie del worker
(`viewer-worker.js:530-533`), así que un rebuild de 1 s retrasa la respuesta del frame siguiente
aunque no ocurra dentro de `renderFrame`. Bloqueos estructurales: voxelización por vista en CPU (B1)
y pirámide LOD sin densidades intermedias (B2); problemas acotados: B3–B7.

## Restricciones globales

- `scripts/check.sh` en verde al cerrar cada tarea; sin cambiar el nightly ni añadir `#![feature]`.
- Sin cambios de formato ni de salida del conversor en R0–R2.
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
- [x] **Paso 2 (`7a6746a`, [registro](../records/benchmarks/RENDER_BASELINE_2026-10-30.md)):** Chrome con adaptador hardware, 18 ejecuciones A (`ede7e0a`) frente a B (`87ef649`). Ninguna rama cumple los criterios de latencia (p95 de respuestas ≈ 1 s en Majadas y 5–7 s en Belvedere durante la carga; en órbita tras el objetivo cada frame reconstruye: 480 ms y 2,6 s); la etapa nunca sube en órbita; sin aumento de nodos sin cubrir ni píxeles vacíos en B. La voxelización ocupa el 98–99,5 % del worker y la decodificación el 0,3–0,9 % (E13 → worker de construcción). **Regresión en B:** más lento hasta el objetivo (Majadas +10 %, Belvedere 8 px sin completar en 300 s) con más puntos por reconstrucción; diagnóstico en curso. Línea base: Majadas 4 px, Belvedere 8 px y 4 px (tope 300 s), 3 rondas; registro con mediana, rango y SHA de `.wasm` y datasets; commit.

## Fase R1: mejoras sin decisiones del propietario (orden E12)

### R1-T0: detector de thrash en la ruta async (E11)

**Archivos:** `crates/viewer_wasm/src/lod_dataset.rs` y tests.

- [x] **`1981f87`:** el detector observa también las llegadas async (sin reiniciarse en cada inserción); contadores `pendingTargetEvictions` y `cacheCycleDetected`. Test: una secuencia de cargas async que expulsa bloques pendientes del objetivo se detecta igual que en la ruta síncrona (pausa del refinado o contador expuesto en `lod`).

### R1-T3a: selección sin coste cuadrático, mismo corte (Q3, Q5)

**Archivos:** `crates/viewer_core/src/spatial_lod_priority.rs`, `crates/viewer_core/src/lod_transition.rs`, `crates/viewer_wasm/src/{lod_dataset.rs, bindings/lod.rs}`.

- [x] **Parte de `viewer_core` (`4ce23e8`, `dac3a38`):** tests dorados (2k, 10⁵ en `check.sh`, 10⁶ ignorado; Replacement y Additive; cuatro cámaras; vía de prioridad y alternativa forzada) sin cambios tras el refactor; frontera enlazada O(hijos) por expansión y comprobaciones de descendientes sin anidamiento; selección por prioridad a 10⁶ nodos de 1,4–1,7 s a 0,4–0,6 s (Replacement) y de 45–96 ms a 12–28 ms (Additive). Residual: liberar candidatos bloqueados sigue siendo lineal (para T3b).
- [x] **Parte de `viewer_wasm` (`4321f17`, `48e7524`):** selección y plan una vez por actualización (antes 2 selecciones y 3 `prepare`); decisiones fijadas con un test dorado; contadores `selectionsPerUpdate` y `preparesPerUpdate`.
- [ ] Test dorado primero: orden exacto de `node_ids` del corte y de pendientes en fixtures y jerarquías sintéticas de 10⁵ y 10⁶ nodos, por la vía de prioridad y por la alternativa; después frontera indexada y selección reutilizada entre `target_cut_is_resident` y la actualización. Benches `#[ignore]` fuera de `check.sh`.

### R1-T4: métrica de error única (Q6, B7, E9)

**Archivos:** `crates/viewer_wasm/src/lod_dataset.rs`, `crates/viewer_render_wgpu/src/camera.rs`.

- [x] **`87ef649`:** `OrbitCamera::projected_error_pixels` única para selección y prioridad (profundidad mínima de las 8 esquinas del AABB; sin `geometric_error`); en 120 cámaras aleatorias nunca selecciona más puntos (menos en 47); arista ≤ objetivo en celdas dentro del frustum (fuera de él el factor fuera de eje puede exceder ~2 %). Una función de error en pantalla para selección y prioridad: espaciado del nodo ÷ profundidad mínima en vista de las 8 esquinas del AABB, con el factor de proyección actual. Tests: arista proyectada ≤ objetivo también fuera de eje en las esquinas del viewport; igual o menos puntos seleccionados que hoy con la misma cámara.

### R1-T5: plano cercano adaptativo (Q8, E10)

**Archivos:** `crates/viewer_render_wgpu/src/camera.rs`.

- [ ] Test: cámara a 1 m del suelo de una nube de decenas de km sin recorte de nodos visibles; puntos y bloques seleccionados antes y después comparados; presupuesto de la fuente respetado; picking existente sin regresión.

### R1-T1a: no reiniciar la etapa al mover (Q1, E6)

**Archivos:** `crates/viewer_wasm/src/bindings/{camera.rs, lod.rs}`, `crates/viewer_wasm/src/lod_dataset.rs`.

- [ ] Función pura (p. ej. `next_lod_stage`) con tests: tras órbitas con el objetivo no residente, la etapa publicada nunca sube; las regiones sin cubrir tienen prioridad de carga. Sin histéresis (eso es T1b).

### R1-T2: menos reconstrucciones y bordes cubiertos (Q2, E7)

**Archivos:** `examples/web-host/viewer-worker.js`, `crates/viewer_wasm/src/{bindings/render.rs, lod_dataset.rs}`.

- [ ] Llegadas agrupadas: una reconstrucción por ventana de tiempo; en un cambio de cámara se dibuja la escena anterior hasta la nueva; el frustum de construcción se amplía con un margen para que el giro no deje bordes vacíos. Medida: `sceneBuilds` y p95 de respuestas de frame (si no baja, criterio de parada → E13).

### R1-T3b: prioridad solo sobre pendientes (Q4)

- [ ] Reordenación intencional (métrica de selección, cálculo solo sobre bloques pendientes) medida por separado: tiempo hasta el objetivo y expulsiones de pendientes.

### R1-T6: jerarquía construida en wasm desde el reader (Q7, B6 parcial)

**Archivos:** `crates/viewer_wasm/src/{async_format_reader.rs, lod_metadata.rs, lod_dataset.rs}`, `examples/web-host/viewer-worker.js`.

- [ ] Sin descriptor JS por nodo; test con 1 M nodos sintéticos: ms de apertura y memoria JS/wasm frente a hoy.

### R1-T1b (espera a P5): histéresis

- [ ] Solo tras la respuesta del propietario a P5 (rango de píxeles dinámico o fijo).

### R1-T7: medida después de R1

- [ ] Matriz completa de R0 con el mismo arnés; registro con la comparación y los criterios de éxito; commit.

## Fase R2: puerta de decisión sobre el hilo de render (B4, E13)

- [ ] Con los datos de R0 (E5): si la decodificación ≥ 20 % del tiempo ocupado del worker, workers de decodificación con transferibles (patrón de ADR 0015); si no, spike de un **worker de construcción** que posee la caché decodificada y transfiere buffers de instancias al worker de render (ataca B1 sin cambiar la estética). Diseño breve (arquitecto) y pre-mortem antes de implementar.

## Fases que esperan decisiones del propietario

| Fase | Contenido | Pregunta |
| --- | --- | --- |
| R3 | Geometría por nodo independiente de la vista y residente en GPU; tamaño en pantalla en el shader; sustituye a la voxelización CPU por corte (B1, opción a); o rasterización por compute para 1 px (opción c) | P1, P2, P5 |
| R4 | Contrato de densidad de la pirámide LOD (un nivel por ×2 de densidad, hojas ≤ 16–64 k puntos, espaciado por nodo en el índice) en V7: reconversión de datasets (B2) | P3, P4 |
| R5 | Jerarquía paginada (al estilo de las páginas de COPC) para > 2 M nodos y apertura sin índice completo; transporte remoto si lo hay | P3, P6 |

Borradores de ADR para R3 y R4 en la evaluación; se numeran al aceptarse (0017 queda reservada para
las zonas OPFS de la [etapa 4](conversor-web-paralelo-etapa4.md)).
