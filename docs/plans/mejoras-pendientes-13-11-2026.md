# Mejoras y pendientes tras la revisión del 13-11-2026: plan de trabajo

Estado: en curso · 14-11-2026 (fase A y B1 cerradas; B2 reabierta por B2b, en curso)

**Objetivo:** ordenar en un solo plan lo que queda de R3, los pendientes menores del README y las
propuestas de la [revisión de técnicas](../records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md),
según el [orden de trabajo acordado](../requirements.md#orden-de-trabajo-acordado): R3 → R5 (completado;
queda la robustez remota) → congelar V7 final → API pública. La velocidad de conversión (RQ-07) es
requisito para promover V7. Las prioridades activas siguen en el [README](../../README.md#prioridades-pendientes).

**Ramas:** `feat/r3-v75` (worktree `~/.cache/codex-worktrees/octree-r3-v75`, que guarda los datasets
v7.5 en `target/render-data`) recibe lo verificado. El trabajo sin verificar va en una rama `explore/`
y se integra con avance rápido cuando pasa su check. La revisión se escribió sobre `c473c85`; su
vigencia en esta rama se anota en cada tarea.

**Reglas:** cada tarea se cierra con su check en verde y `scripts/check.sh --offline`. Las
optimizaciones que pueden rechazarse (fase C) se integran solo con el visto bueno del propietario. Si
salta un criterio de parada de la Tarea 11, se consulta al propietario. Una subtarea cerrada se
borra al cerrar su tarea; el detalle queda en git y en los registros.

## Fase A: higiene y defectos baratos (cerrada)

- [x] A1. OQ-06 cerrada (ADR 0030) y plan indexado (`b50497f`).
- [x] A2. TW2: un `cacheBytes` distinto recrea el presupuesto de lectura en los dos workers (`43fc0d9`).
- [x] A3. Una página que falla no se pide en bucle y un injerto fallido no rompe la cola (`43fc0d9`).
- [x] A4. TA8: `#![forbid(unsafe_code)]` en los cinco crates sin `unsafe` (`5a1c7d8`).

## Fase B: terminar R3 ([plan](render-r3-cubos-en-gpu.md))

- [x] B1. Tarea 11 medida: estadísticas `cutApply` y `cellsForgottenTotal`, probe
  `probe-cells-smoke.mjs` y [registro](../records/benchmarks/R3_T11_SMOKE_13-11-2026.md) (`97c52f8`,
  `80a44bd`). Se asienta a 4 px; Majadas a 1 px dentro de una hoja entra en bucle de olvido (R3-Q27).
- [x] B1c. Bucle de olvido de Majadas a 1 px: corregido en tres capas, residencia por celdas
  ([ADR 0038](../adr/0038-residencia-por-celdas.md)), hijos en curso protegidos y contados en el ajuste
  del [ADR 0037](../adr/0037-corte-ajustado-al-presupuesto-de-cpu.md) y bloqueo sin oscilación
  ([registro](../records/benchmarks/R3_B1C_CPU_FIT_14-11-2026.md)). Majadas a 1 px dentro de una hoja
  se asienta engrosado; 4 px sin regresión.
- [x] B1d. Pool de celdas de 512 MiB (65.535 páginas en la Radeon 880M) con presupuesto propio y corte
  de nodos ajustado al pool ([ADR 0040](../adr/0040-pool-de-512-mib-y-corte-ajustado-al-pool.md);
  [registro](../records/benchmarks/R3_B1D_GPU_POOL_14-11-2026.md)). Se asientan las 14 poses a 1 px y
  a 4 px; GTT pico ~+750 MiB. El ajuste al pool no actuó en el navegador (basta el pool mayor); lo
  cubre un test en simulación. La Tarea 11 queda abierta solo por el criterio de ≤ 16,7 ms (B1b.3).
- [x] B1b. Casillas de la Tarea 16b ([registro](../records/benchmarks/R3_B1B_HIZ_14-11-2026.md)): fallo de
  escala de `hzb.rs` corregido con test; margen del Hi-Z derivado (`u·(25ρ + 24)`, ≈ +3 % de GPU);
  tiempo de GPU por frame con timestamps (R3-Q31), p95 7,38 ms en Majadas dentro de una hoja a 4 px.
  Con esto pasan la Tarea 11 y la 16b.
- [x] B2. Tarea 12, modos de color (reabierta por B2b) ([registro](../records/benchmarks/R3_B2_COLOR_14-11-2026.md)): todos los
  modos con celdas, función WGSL común (paridad con diferencia 0), modos de shader sin reconstruir y
  modos de atributo con generación de color; sin vuelta a la ruta CPU.
- [ ] B2b. Intensidad y clase en la ruta asíncrona. Las columnas de intensidad y clase de Belvedere y
  Majadas son constantes a 0 en origen (rangos `[0, 0]` correctos: es el dato). Pero en el navegador
  el worker de construcción lee en modo RGB solo la geometría (`build-worker.js:290-301`,
  `insertAsyncLodBlock` con un solo atributo): las celdas quedan sin intensidad ni clase y los modos de
  shader muestran luminancia y RGB, donde la ruta CPU muestra gris plano y rojo. B2.2 solo cubrió la
  ruta síncrona. Reabre la Tarea 12. Rama `explore/b2b-async-attrs`.
  - [ ] B2b.1. Test rojo: las celdas construidas por la ruta asíncrona en modo RGB llevan las palabras
    de intensidad y clase.
  - [ ] B2b.2. La ruta asíncrona de celdas lee siempre intensidad y clase junto a la geometría (y el
    atributo del modo `attribute:N:M`). Check: B2b.1 en verde.
  - [ ] B2b.3. Navegador: en Majadas, intensidad y clasificación con celdas iguales a la ruta CPU (gris
    plano y rojo); registro nuevo fechado que corrige el de B2.
- [ ] B3. Tarea 13: un mismo espaciado de referencia por nube (T0b) no cambia al injertar páginas.
- [ ] B4. Tarea 14, modos de px: fijo progresivo (predeterminado) y dinámico hacia 16,7 ms con histéresis (RF-06, RF-07).
- [ ] B5. TW6, pérdida de dispositivo: recuperar sin recargar (R3-Q26).
  - [ ] B5.1. El worker de render detecta la pérdida (callback de `wgpu`) y la expone en las estadísticas y en la API.
  - [ ] B5.2. Recrear adaptador, dispositivo, superficie y recursos; el pool usa `poolReset` y el
    worker de construcción reenvía las celdas; la cámara se conserva.
  - [ ] B5.3. Ruta CPU: volver a subir la escena.
  - [ ] B5.4. Playwright: caída del proceso GPU de Chrome (no `device.destroy()`, cuyo motivo
    `destroyed` no se recupera); la vista vuelve con el mismo digest y sin errores en consola.
- [ ] B6. Tarea 17, medida y ruta por defecto: matriz de R0 más RED-REF en el equipo de referencia
  (≥ 4 GB de GPU); al cerrarla, actualizar el estado del ADR 0027.

## Fase C: velocidad de conversión, RQ-07 (rama `explore/convert-speed`)

Cada variante: mediana de 3 en Belvedere, SHA-256 de salida idéntico y V5 medido de nuevo en la
misma sesión para la razón v7.5/V5. No se mide a la vez que el navegador.

- [ ] C0. Línea base de la sesión: V5 y v7.5 en Belvedere, mediana de 3, con tiempos por fase.
- [ ] C1. TC3: `i128` fuera del bucle por punto en conversor y lector. Check adicional: paridad del lector y `decodeMs` en nativo y wasm.
- [ ] C2. TC2: acumulador de muestreo con datos calientes separados o agregación por lotes; `perf stat` de fallos de caché y TLB.
- [ ] C3. TC7: cola sin barrera por lote y `par_iter` efectivo. Check: curva 1–8 workers con RSS y orden de salida igual.
- Parada (umbral propuesto en este plan, no medido): si tras C1 y C2 Belvedere sigue en v7.5/V5 ≥ 1,7,
  se replantea RQ-07 alrededor de TC5.

## Fase D: fuente remota y V7 final

- [ ] D1. TW3 cancelación real con `AbortController` y TW5 reintentos con tiempo límite.
- [ ] D2. TW4 unión con huecos y TW10 concurrencia por fuente, medidas en RED-REF.
- [ ] D3. Congelar V7 final (ADR 0022), tras decidir OQ-09 (orden de filas, TF1).
- [ ] D4. TW8 `{url}` en la API pública; pertenece a la etapa de API pública, después de D3.

## Limpieza

- [x] L1. Copias sin commitear de la revisión en el checkout principal (`feat/render-r3-node-cells`),
  ya integradas en `b50497f`: descartadas el 13-11-2026.
- [x] L2. Salidas temporales de los probes de B1c, B1d y B1b (fuera de git): borradas al registrarlas.
- [x] L3. Ramas `explore/b1c-cpu-fit`, `explore/b1d-gpu-pool`, `explore/b1b-hzb` y `explore/b2-color`: integradas en `feat/r3-v75` y borradas.
- [ ] L4. Salidas de tareas R3 anteriores en `target/r3-*` del worktree (~29 GB, fuera de git; las mayores,
  `r3-t6-accept`, `r3-t6-check`, `r3-t16-recheck` y `r3-t16-cases`, de 6 GB cada una): borrar con el visto
  bueno del propietario, conservando lo que aún usan los probes y benches (`r3-t16-cases`, `r3-t7c-pool`).

## Decisiones del propietario pendientes

| Tema | Bloquea |
| --- | --- |
| OQ-09 orden de filas en bloque (TF1) | D3 |
| OQ-10 visor nativo o sin cabeza (TA4) | Planificar la API pública |
| Criterio de ≤ 16,7 ms si no se mide el tiempo de GPU (B1b.3) | B1b |
| ADR 0008 (`release` optimiza tamaño; usa `opt-level = 3` desde el 02-11-2026) y ADR 0025 (aceptado; el codificador propio no existe) | Nada; requieren ADR nuevo o decisión |
| PR de `feat/r3-v75` a `master` | Integración |
