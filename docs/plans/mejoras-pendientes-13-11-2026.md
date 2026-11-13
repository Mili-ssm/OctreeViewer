# Mejoras y pendientes tras la revisión del 13-11-2026: plan de trabajo

Estado: en curso · 13-11-2026, enmendado el mismo día por un pre-mortem

**Objetivo:** ordenar en un solo plan lo que queda de R3, los pendientes menores del README y las
propuestas de la [revisión de técnicas](../records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md),
según el [orden de trabajo acordado](../requirements.md#orden-de-trabajo-acordado): R3 → R5 (completado;
queda la robustez remota) → congelar V7 final → API pública. La velocidad de conversión (RQ-07) es
requisito para promover V7. Las prioridades activas siguen en el [README](../../README.md#prioridades-pendientes).

**Rama:** `feat/r3-v75` (worktree `~/.cache/codex-worktrees/octree-r3-v75`, que guarda los datasets
v7.5 en `target/render-data`). La revisión se escribió sobre `c473c85`, 51 commits atrás; su vigencia
en esta rama se anota en cada tarea. **TW1 ya está corregido aquí** (`let outcome` antes del `if`).

**Reglas:** cada tarea se cierra con su check en verde y `scripts/check.sh --offline`. Las
optimizaciones que pueden rechazarse (fase C) van en una rama `explore/` y se integran solo con el
visto bueno del propietario. Si salta un criterio de parada de la Tarea 11, se para la fase B y se
consulta al propietario.

## Fase A: higiene y defectos baratos

- [x] A1. Cerrar OQ-06 en requisitos (lo adoptó el ADR 0030) y enlazar este plan en el índice. Check: `check.sh` (enlaces, índices).
- [x] A2. TW2: un `load` con otro `cacheBytes` recrea el presupuesto de lectura, en `build-worker.js` y
  también en `viewer-worker.js` (sin worker de construcción, donde el worker no se recrea). Check: test Node rojo → verde.
- [x] A3. Una página que falla al cargar no se vuelve a pedir hasta una refinación explícita o un
  dataset nuevo, en los dos workers (sin worker de construcción se pedía en bucle; con él, en cada
  vista), y un injerto que lanza no deja rechazada la cola de mensajes. Check: tests Node rojo → verde
  y mutaciones que quitan cada guarda hacen fallar su test.
- [x] A4. TA8: `#![forbid(unsafe_code)]` en los cinco crates sin `unsafe` (todos salvo `viewer_convert`). Check: `cargo check` nativo y wasm32.

## Fase B: terminar R3 ([plan](render-r3-cubos-en-gpu.md))

- [ ] B1. Tarea 11, humo en navegador: criterios de «alcanzado», congelación por `DescendantTooFine`
  y medida del olvido bajo presupuesto. El tope de GPU/GTT se mide como incremento sobre el reposo
  (iGPU con memoria compartida). Check: Playwright en Chrome con WebGPU; registro fechado.
  - [x] B1.1. Estadísticas de celdas en JS: estado del último corte (`Applied`/`RetainedPrevious`) y
    contador acumulado de nodos olvidados. Check: test Rust de los bindings.
  - [x] B1.2. Probe `probe-cells-smoke.mjs`: poses de Belvedere y Majadas (incluido Majadas dentro de
    una hoja a 4 px con límites de producción), saltos de pose, criterios de «alcanzado», rachas de
    `RetainedPrevious`, olvidos en reposo, p50/p95 de frame y GTT sobre el reposo. Check: corre en Chrome con WebGPU.
  - [x] B1.3. Ejecutar el probe y registrar `R3_T11_SMOKE_<fecha>` con los criterios de la Tarea 11.
- [ ] B1b. Casillas abiertas de la Tarea 16b: cota del margen de profundidad del Hi-Z, escala de la
  pirámide de `hzb.rs` (test rojo primero) y ≤ 16,7 ms en wasm con límites de producción, que se mide en B1.
- [ ] B1c. Corte ajustado al presupuesto de CPU (R3-Q27, [ADR 0037](../adr/0037-corte-ajustado-al-presupuesto-de-cpu.md)): el driver no olvida nodos del
  corte vigente y el corte se engrosa en lo lejano hasta caber. Check: test rojo → verde con un
  presupuesto menor que el conjunto de trabajo, y Majadas a 1 px dentro de una hoja se asienta en el probe.
- [ ] B2. Tarea 12, modos de color, con ≤ 1 LSB en intensidad y elevación (R3-Q25).
- [ ] B3. Tarea 13, continuidad entre nubes con paginación.
- [ ] B4. Tarea 14, modos de px.
- [ ] B5. TW6, pérdida de dispositivo: recuperar sin recargar (R3-Q26). Base existente: `poolReset` (`build-worker.js`); la superficie perdida sigue siendo
  fatal (`gpu.rs`). Check: caída del proceso GPU de Chrome, no `device.destroy()` (motivo `destroyed`, que no se recupera).
- [ ] B6. Tarea 17, medida y ruta por defecto. Depende de B2 y del equipo de referencia con RED-REF;
  al cerrarla, actualizar el estado del ADR 0027.

## Fase C: velocidad de conversión, RQ-07 (rama `explore/convert-speed`)

Puede avanzar en paralelo a la fase B. Cada variante: mediana de 3 en Belvedere,
SHA-256 de salida idéntico y V5 medido de nuevo en la misma sesión para la razón v7.5/V5.

- [ ] C1. TC3: `i128` fuera del bucle por punto en conversor y lector. Check adicional: paridad del lector y `decodeMs` en nativo y wasm.
- [ ] C2. TC2: acumulador de muestreo con datos calientes separados o agregación por lotes.
- [ ] C3. TC7: cola sin barrera por lote y `par_iter` efectivo. Check: curva 1–8 workers con RSS y orden de salida igual.
- Parada (umbral propuesto en este plan, no medido): si tras C1 y C2 Belvedere sigue en v7.5/V5 ≥ 1,7,
  se replantea RQ-07 alrededor de TC5.

## Fase D: fuente remota y V7 final

- [ ] D1. TW3 cancelación real con `AbortController` y TW5 reintentos con tiempo límite.
- [ ] D2. TW4 unión con huecos y TW10 concurrencia por fuente, medidas en RED-REF.
- [ ] D3. Congelar V7 final (ADR 0022), tras decidir OQ-09 (orden de filas, TF1).
- [ ] D4. TW8 `{url}` en la API pública; pertenece a la etapa de API pública, después de D3.

## Decisiones del propietario pendientes

| Tema | Bloquea |
| --- | --- |
| OQ-09 orden de filas en bloque (TF1) | D3 |
| OQ-10 visor nativo o sin cabeza (TA4) | Planificar la API pública |
| ADR 0008 (`release` optimiza tamaño; usa `opt-level = 3` desde el 02-11-2026) y ADR 0025 (aceptado; el codificador propio no existe) | Nada; requieren ADR nuevo o decisión |
| PR de `feat/r3-v75` a `master` | Integración |
