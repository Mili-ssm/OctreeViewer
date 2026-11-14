# Mejoras y pendientes tras la revisión del 13-11-2026: plan de trabajo

Estado: en curso · 14-11-2026 (fases A y B cerradas; fase C parada, C4 pendiente del propietario; en curso: fase D, D3b)

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
- [x] B2. Tarea 12, modos de color (reabierta y cerrada con B2b) ([registro](../records/benchmarks/R3_B2_COLOR_14-11-2026.md)): todos los
  modos con celdas, función WGSL común (paridad con diferencia 0), modos de shader sin reconstruir y
  modos de atributo con generación de color; sin vuelta a la ruta CPU.
- [x] B2b. Intensidad y clase en la ruta asíncrona ([registro](../records/benchmarks/R3_B2B_ASYNC_ATTRS_14-11-2026.md),
  que corrige el de B2): el worker de construcción lee intensidad y clase con la geometría siempre que
  hay celdas (`6f37e29`, test rojo → verde); en el navegador, intensidad y clasificación coinciden en
  tipo de color con la ruta CPU. Las columnas valen 0 en origen: los rangos `[0, 0]` son correctos.
- [x] B3. Tarea 13, continuidad por nube ([registro](../records/benchmarks/R3_B3_CONTINUITY_14-11-2026.md)):
  mínimo de cubo = factor × espaciado de referencia (factor 1 por defecto, validado), sin reconstruir
  celdas al alternar y sin contar como engrosamiento. En Belvedere y Majadas el efecto visual es mínimo:
  el espaciado de referencia de Majadas es ~2 mm. Subir el factor queda como decisión del propietario.
- [x] B4. Tarea 14, modos de px ([registro](../records/benchmarks/R3_B4_PX_MODES_14-11-2026.md)): fijo
  progresivo por defecto (con test) y dinámico con escalera 1, 2, 4, 8 px guiado por el tiempo de GPU;
  en el navegador refina y engrosa según la carga.
- [x] B5. TW6, pérdida de dispositivo ([registro](../records/benchmarks/R3_B5_DEVICE_LOST_14-11-2026.md)):
  recuperación sin recargar en 0,61 s con la misma pose y el mismo corte (R3-Q26), y canvas sustituido
  por la API (R3-Q34). Probado con una caída del proceso GPU vía CDP.
- [x] B6. Tarea 17 ([registro](../records/benchmarks/R3_T17_MEASURE_14-11-2026.md)): RQ-01, RQ-02 y RQ-05
  con celdas en este equipo; el propietario aceptó los +2,7 pp de píxeles vacíos de Majadas y la ruta de
  celdas es la predeterminada (R3-Q35, `8aa40fa`; ADR 0027 aceptado). Con esto se cierra la fase B.

## Fase C: velocidad de conversión, RQ-07 (parada)

[Registro](../records/benchmarks/EXPLORE_CONVERT_SPEED_14-11-2026.md). Línea base Belvedere: V5 20,9 s,
v7.5 40,2 s (razón 1,92). La diferencia está en construir la jerarquía (+12,4 s, muestreo). TC3, TC1 y
TC2(b) dan salida idéntica pero ≤ 8 % de mejora: salta el criterio de parada. Nada se integra; las ramas
`explore/convert-speed` y `explore/convert-batch` se conservan como evidencia.

- [ ] C4. **Decisión del propietario, aplazada el 14-11-2026 a después de la fase D:** TC5 (rediseño del
  pipeline con plan propio), revisar RQ-07 o probar la copia diferida del más cercano.

## Fase D: fuente remota y V7 final

- [x] D1. TW3 cancelación real y TW5 reintentos con tiempo límite ([registro](../records/benchmarks/D1_REMOTE_CANCEL_RETRY_14-11-2026.md)):
  implementados y con tests; sin efecto medible en Majadas por RED-REF (no quedan lecturas obsoletas).
- [x] D2. TW4 unión con huecos y TW10 concurrencia por fuente ([registro](../records/benchmarks/D2_GAP_CONCURRENCY_14-11-2026.md)):
  8 lecturas remotas por defecto (−15 a −30 % hasta alcanzar el objetivo en la ruta CPU); `maxGap`
  queda en 0. De paso se corrigió el orden de lectura asíncrono de las celdas (Belvedere remoto no
  llegaba al objetivo).
- [ ] D3. Congelar V7 final (ADR 0022) con el orden de filas libre por bloque ([ADR 0041](../adr/0041-orden-de-filas-libre-por-bloque.md),
  [registro de OQ-09](../records/benchmarks/OQ09_MORTON_ORDER_14-11-2026.md)).
  - [x] D3a. Causa de la construcción más rápida con ficheros Morton ([registro](../records/benchmarks/D3A_CELL_SORT_ORDER_14-11-2026.md)):
    el constructor ordenaba con pdqsort; con un radix estable, construir las celdas cuesta un 13–46 %
    menos con cualquier orden de fichero.
  - [ ] D3b. Escritor híbrido detrás de una opción del escritor, sin identidad propia
    ([ADR 0042](../adr/0042-escritor-hibrido-en-la-identidad-final-de-v7.md)). Comprobación: `check.sh` verde,
    `.oct` de Belvedere ≤ 530 MB, y tiempo total y RSS de conversión medidos frente a RQ-07.
    - [ ] D3b1. Opción `row_order` (fuente por defecto, híbrido) en el escritor de `viewer_format`. Regla:
      codificar como hoy; si gana la ocupación compacta, ordenar en Morton, codificar por diferencias y quedarse
      con lo más pequeño, permutando colores y atributos (+0,5 % sobre el óptimo en Belvedere, estimado con los
      tamaños por bloque de OQ-09). La opción entra en la configuración del checkpoint. Tests de ida y vuelta
      por conjunto de filas. Comprobación: tests de `viewer_format` y `viewer_convert_core` verdes.
    - [ ] D3b2. Textos que prometen el orden de la fuente (`occupied_point_block.rs:33,77`,
      `quantized_occupancy.rs:99,105`) y el comentario de la prueba que lo fija. Comprobación: `grep` sin
      promesas de orden de la fuente fuera de las especificaciones de revisiones anteriores.
    - [ ] D3b3. Medir con Belvedere: tamaño, tiempo total y RSS de conversión (≥ 3 ejecuciones intercaladas por
      opción) y carga remota en el navegador. Comprobación: registro con las cifras.
  - [ ] D3c. Congelar tras C4 (RQ-07): identidad final de V7 con el escritor híbrido por defecto (unos 30
    puntos: versiones, E/S paginada, CLI, especificación, índices, pruebas), regeneración de datasets y
    referencias. Comprobación: `check.sh` verde con la identidad final y referencias R1 regeneradas.
- [ ] D4. TW8 `{url}` en la API pública; pertenece a la etapa de API pública, después de D3.

## Limpieza

- [x] L1. Copias sin commitear de la revisión en el checkout principal (`feat/render-r3-node-cells`),
  ya integradas en `b50497f`: descartadas el 13-11-2026.
- [x] L2. Salidas temporales de los probes de B1c, B1d y B1b (fuera de git): borradas al registrarlas.
- [x] L3. Ramas `explore/b1c-cpu-fit`, `explore/b1d-gpu-pool`, `explore/b1b-hzb`, `explore/b2-color`, `explore/b2b-async-attrs`, `explore/b3-continuity`, `explore/b4-px-modes`, `explore/b5-device-lost`, `explore/b6-measure`, `explore/b6-default-cells`, `explore/d1-remote` y `explore/d2-gap`: integradas en `feat/r3-v75` y borradas.
- [x] L4. Salidas de tareas R3 anteriores en `target/r3-*` (fuera de git): 45 directorios (~21 GB) borrados
  con el visto bueno del propietario el 14-11-2026; se conservan los que usan probes, tests y benches
  (`r3-t16-cases`, `r3-t7c-pool`, `r3-fixtures`, `r3-t5-synthetic`, `r3-t7c`).

## Decisiones del propietario pendientes

| Tema | Bloquea |
| --- | --- |
| Factor de continuidad (RF-05): con 1 × el espaciado de referencia el efecto es mínimo en Belvedere y Majadas | Nada; ajuste visual |
| RQ-07 tras la fase C: TC5, revisar el objetivo o copia diferida | C4, promover V7 |
| OQ-10 visor nativo o sin cabeza (TA4) | Planificar la API pública |
| Criterio de ≤ 16,7 ms si no se mide el tiempo de GPU (B1b.3) | B1b |
| ADR 0008 (`release` optimiza tamaño; usa `opt-level = 3` desde el 02-11-2026) y ADR 0025 (aceptado; el codificador propio no existe) | Nada; requieren ADR nuevo o decisión |
| PR de `feat/r3-v75` a `master` | Integración |
