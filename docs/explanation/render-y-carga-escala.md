# Render y carga a escala — plan

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Estado (30-10-2026):** plan redactado tras la [evaluación](../records/archive/RENDER_ASSESSMENT_2026-10-30.md);
la fase R1 no necesita decisiones del propietario; R3–R5 esperan las
[preguntas pendientes](preguntas-propietario.md) P1–P6.

**Objetivo (propietario, 30-10-2026):** cargar nubes de cientos de GB y dibujarlas con vóxeles de
1–8 px en pantalla, con una carga que se perciba continua: sin saltos a grueso al mover la cámara,
sin huecos y sin bloqueos de frame.

**Arquitectura de partida:** un worker de visor lee bloques del `.oct` local, selecciona un corte
LOD por error en pantalla, voxeliza en CPU todo el corte para la vista actual y sube las instancias
a la GPU (cubos instanciados). La evaluación identifica dos bloqueos estructurales (voxelización por
vista en CPU, B1; pirámide LOD sin densidades intermedias, B2) y varios problemas acotados (B3–B7).
Este plan los ataca de menor a mayor compromiso: primero lo que no cambia contratos (R0–R2), después
lo que necesita decisiones del propietario (R3–R5).

## Restricciones globales

- `scripts/check.sh` en verde al cerrar cada tarea; sin cambiar el nightly ni añadir `#![feature]`.
- Sin cambios de formato ni de salida del conversor en R0–R2 (R1 del conversor intacto).
- Cada mejora se mide **antes y después con el mismo escenario y el mismo arnés** (fase R0); si una
  métrica empeora más allá del ruido registrado, la tarea no se cierra.
- Los cambios de selección LOD conservan el contrato visible: ningún vóxel por encima del objetivo
  de píxeles salvo los ya permitidos por la densidad de la fuente (`REGIONAL_VOXEL_LOD.md`).
- Registros nuevos fechados con el día de ejecución, en español; commits convencionales en inglés con
  `Co-Authored-By`; sin push. Mediciones de tiempo sin otros agentes pesados activos.

## Review Focus

1. **Órbita continua durante la carga**: el corte visible nunca vuelve a una etapa más gruesa (R1-T1).
2. **Llegada masiva de bloques**: ningún frame espera a una reconstrucción completa (R1-T2).
3. **Jerarquías grandes**: selección y prioridad con 10⁵–10⁶ nodos sintéticos sin coste cuadrático (R1-T3).
4. **Cámara cerca del suelo**: el plano cercano no recorta primeros planos (R1-T5).
5. **Fallos de lectura y expulsiones**: las optimizaciones conservan la retención del último corte completo (todas).

---

## Fase R0: arnés y línea base

### R0-T1: arnés reproducible de carga y órbita

**Archivos:** `examples/web-host/probe-*.mjs` (extender `probe-cache-camera.mjs` o crear `probe-orbit-load.mjs`), `docs/records/benchmarks/RENDER_BASELINE_<aaaa-mm-dd>.md`.

- [ ] **Paso 1:** escenario fijo por dataset (Majadas 4 px, Belvedere 8 px y 4 px con tope de 300 s): primer frame, tiempo hasta el objetivo, pendientes al tope, p95/máx de la llamada de render durante la carga y durante una órbita de 10 s, `lodStagePixels` a lo largo de la órbita, ms de selección/plan/construcción, nº de reconstrucciones, PSS del árbol del navegador. Firefox headless con WebGPU (como los registros previos) y Chrome con GPU si está disponible.
- [ ] **Paso 2:** 3 rondas por celda; registro con mediana y rango; commit.

## Fase R1: mejoras sin decisiones del propietario

### R1-T1: no reiniciar la etapa LOD al mover la cámara (Q1)

**Archivos:** `crates/viewer_wasm/src/bindings/{camera.rs, lod.rs}`, `crates/viewer_wasm/src/lod_dataset.rs` y sus tests.

- [ ] Test: tras una secuencia de órbitas con el objetivo no residente, la etapa publicada nunca sube; el corte visible se conserva y se refina desde él. Implementar con histéresis simple (refinar al objetivo, no simplificar hasta 2 × el objetivo). Medida R0: `lodStagePixels` monótono en órbita.

### R1-T2: reconstrucción agrupada y fuera de `renderFrame` (Q2)

**Archivos:** `examples/web-host/viewer-worker.js`, `crates/viewer_wasm/src/bindings/render.rs`, `crates/viewer_wasm/src/lod_dataset.rs`.

- [ ] Test: N llegadas seguidas producen una sola reconstrucción por ventana de tiempo; `renderFrame` dibuja la última escena construida sin reconstruir. Medida R0: nº de reconstrucciones y p95 de la llamada de render en carga.

### R1-T3: selección y prioridad sin coste cuadrático (Q3, Q4, Q5)

**Archivos:** `crates/viewer_core/src/spatial_lod_priority.rs`, `crates/viewer_core/src/lod_transition.rs`, `crates/viewer_wasm/src/{lod_dataset.rs, bindings/lod.rs}`.

- [ ] Tests de caracterización primero (mismo corte, mismo orden de pendientes que hoy en fixtures y jerarquías sintéticas); después: frontera indexada (O(F log F)), selección reutilizada entre `target_cut_is_resident` y la actualización, prioridad calculada solo sobre bloques pendientes con la métrica de selección. Bench con 10⁵ y 10⁶ nodos sintéticos.

### R1-T4: métrica de error única y punto más cercano del AABB (Q6, B7)

**Archivos:** `crates/viewer_wasm/src/lod_dataset.rs`, `crates/viewer_render_wgpu/src/camera.rs`, `crates/viewer_core` (si la métrica pasa allí).

- [ ] Una función de error en pantalla compartida por selección y prioridad: espaciado del nodo ÷ distancia al punto más cercano del AABB, con el factor de proyección actual. Tests: con la misma cámara, el máximo de arista proyectada sigue ≤ objetivo y se seleccionan igual o menos puntos que hoy.

### R1-T5: plano cercano adaptativo (Q8)

**Archivos:** `crates/viewer_render_wgpu/src/camera.rs`.

- [ ] Test: con la cámara a 1 m del suelo de una nube de decenas de km, ningún nodo visible queda recortado por el plano cercano; precisión de profundidad suficiente (sin z-fighting en el test de picking existente).

### R1-T6: jerarquía construida en wasm desde el reader (Q7, B6 parcial)

**Archivos:** `crates/viewer_wasm/src/{async_format_reader.rs, lod_metadata.rs, lod_dataset.rs}`, `examples/web-host/viewer-worker.js`.

- [ ] Sin descriptor JS por nodo: el reader entrega la jerarquía a `SpatialHierarchy` dentro de wasm. Test con 1 M nodos sintéticos: ms de apertura y memoria JS/wasm frente a hoy.

### R1-T7: medida después de R1

- [ ] Mismo arnés y escenarios que R0; registro con la comparación; commit.

## Fase R2: decodificación fuera del hilo de render (B4)

### R2-T1: workers de decodificación con transferibles

Patrón de ADR 0015 (sin memoria compartida): el worker de visor reparte lecturas y decodificación a
K workers y recibe columnas listas como `ArrayBuffer` transferidos. Sin decisión del propietario.

- [ ] Diseño breve (arquitecto) y pre-mortem antes de implementar; medida R0 antes y después (tiempo hasta el objetivo y p95 de render en carga).

## Fases que esperan decisiones del propietario

| Fase | Contenido | Pregunta |
| --- | --- | --- |
| R3 | Geometría por nodo independiente de la vista y residente en GPU; tamaño en pantalla en el shader; sustituye a la voxelización CPU por corte (B1, opción a); o rasterización por compute para 1 px (opción c) | P1, P2, P5 |
| R4 | Contrato de densidad de la pirámide LOD (un nivel por ×2 de densidad, hojas ≤ 16–64 k puntos, espaciado por nodo en el índice) en V7: reconversión de datasets (B2) | P3, P4 |
| R5 | Jerarquía paginada (al estilo de las páginas de COPC) para > 2 M nodos y apertura sin índice completo; transporte remoto si lo hay | P3, P6 |

Borradores de ADR para R3 y R4 en la [evaluación](../records/archive/RENDER_ASSESSMENT_2026-10-30.md)
y en el informe del arquitecto; se numeran al aceptarse (0017 queda reservada para las zonas OPFS de la
[etapa 4](conversor-web-paralelo-etapa4.md)).
