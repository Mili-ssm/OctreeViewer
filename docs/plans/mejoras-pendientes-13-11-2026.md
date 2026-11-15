# Mejoras y pendientes tras la revisión del 13-11-2026: plan de trabajo

Estado: activo · 15-11-2026 (fases A, B y C cerradas; D1, D2, D3a, D3b y D3c hechos; siguiente: fusionar la [PR #1](https://github.com/Mili-ssm/OctreeViewer/pull/1), luego D4 ([#11](https://github.com/Mili-ssm/OctreeViewer/issues/11)); ver
[Cómo retomar](#cómo-retomar-15-11-2026))

**Objetivo:** ordenar en un solo plan lo que queda de R3, los pendientes menores del README y las
propuestas de la [revisión de técnicas](../records/benchmarks/REVIEW_TECHNIQUES_13-11-2026.md),
según el [orden de trabajo acordado](../requirements.md#orden-de-trabajo-acordado): R3 → R5 (completado;
queda la robustez remota) → congelar V7 final → API pública. La velocidad de conversión (RQ-07) es
requisito para promover V7. El resumen de prioridades está en el [README](../../README.md#prioridades-pendientes) y lo pendiente, en [issues](https://github.com/Mili-ssm/OctreeViewer/issues).

**Ramas:** `feat/r3-v75` (worktree `~/.cache/codex-worktrees/octree-r3-v75`, que guarda los datasets
v7.5 en `target/render-data`) recibe lo verificado. El trabajo sin verificar va en una rama `explore/`
y se integra con avance rápido cuando pasa su check. La revisión se escribió sobre `c473c85`; su
vigencia en esta rama se anota en cada tarea.

**Reglas:** cada tarea se cierra con su check en verde y `scripts/check.sh --offline`. Las
optimizaciones que pueden rechazarse (fase C) se integran solo con el visto bueno del propietario. Si
salta un criterio de parada de la Tarea 11, se consulta al propietario. Una subtarea cerrada se
borra al cerrar su tarea; el detalle queda en git y en los registros.

## Cómo retomar (15-11-2026)

**Estado del código:** `feat/r3-v75` con D3c integrada: `v7` final es la salida predeterminada del CLI, el wasm
y la app web ([ADR 0044](../adr/0044-v7-final-congelada-y-predeterminada.md)); los datasets de render son
`target/render-data/belvedere-v7` y `majadas-v7` (los `*-v75` siguen en disco, sin uso). La rama está publicada
en `origin` y aún no está integrada en `main`. El checkout principal sigue en
`feat/render-r3-node-cells` (`c473c85`, ancestro) con una línea `.superpowers/` sin commit en `.gitignore`
que no es de esta línea de trabajo.

**Decisiones del propietario del 15-11-2026** (resueltas; [requisitos](../requirements.md#resoluciones-del-15-11-2026)):

- Se revisa la cifra de conversión de RQ-07 ([ADR 0043](../adr/0043-coste-de-conversion-de-v7-frente-a-v5.md)).
  D3c ya no está bloqueado.
- Remoto `origin` = `https://github.com/Mili-ssm/OctreeViewer.git` (privado). Su historial (9 commits de 2023,
  otro proyecto) no tiene relación con este y se sobrescribe; antes se guarda en la ref local
  `refs/legacy/octreeviewer-2023`. `master` se publica como `main` y `feat/r3-v75` se integra por PR.

**Siguiente trabajo, en orden:**

- [x] Publicar en el remoto (15-11-2026): `master` (`ded7ee8`) → `main` con `--force-with-lease` y `feat/r3-v75`
  con seguimiento; `git ls-remote origin` muestra los dos SHA locales.
- [x] D3c (ver la fase D, hecho el 15-11-2026): [plan propio](d3c-congelado-v7-final.md), con pre-mortem y decisiones del propietario en el
  [ADR 0044](../adr/0044-v7-final-congelada-y-predeterminada.md). Comprobación: la de D3c.
- [x] PR de `feat/r3-v75` a `main` abierta (15-11-2026): [PR #1](https://github.com/Mili-ssm/OctreeViewer/pull/1); la fusión la decide el propietario.
- [x] Flujo de GitHub adoptado (15-11-2026): lo pendiente pasa a [issues](https://github.com/Mili-ssm/OctreeViewer/issues) ([reglas](../../CONTRIBUTING.md#issues-y-prs)); los experimentos son PRs en borrador (#2–#10).
- [ ] D4: [#11](https://github.com/Mili-ssm/OctreeViewer/issues/11), dentro del [proyecto API pública](https://github.com/Mili-ssm/OctreeViewer/issues/27).

**Ramas conservadas como evidencia** (sin integrar): `explore/convert-speed`, `explore/convert-batch` (fase
C), `explore/oq09-morton` (worktree de un agente en `.claude/worktrees/agent-a218fbc1eaf663ffd`
del checkout principal), `explore/oq09-remote` (ejemplos `oq09_content_hash` y `oq09_block_sizes` y el
escritor experimental `OQ09_MORTON=best`), `explore/oq09-viewer-sort`, `explore/oq09-build-cause`.

**Notas de medida que costaron tiempo:**

- Los probes de navegador necesitan `python3 -m http.server 8767 --bind 127.0.0.1` en la raíz del
  worktree, y `--rest-first` para medir el tiempo hasta el objetivo sin órbita.
- `viewer_convert` rechaza una salida que ya existe: borrarla antes de cada conversión.
- `/tmp` es un tmpfs de 16 GB; una conversión de Belvedere escribe ~3,4 GB. Las salidas de medida van en
  `target/` del worktree.
- No editar un worktree mientras `check.sh` corre sobre él (tarda ~15 min): invalida el resultado.
- Comparar RSS sobre medianas en KiB, no en MiB redondeados, y medir tiempos con la máquina libre y rondas
  intercaladas (≥ 5 cuando la diferencia buscada es de pocos puntos).

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

## Fase C: velocidad de conversión, RQ-07 (cerrada)

[Registro](../records/benchmarks/EXPLORE_CONVERT_SPEED_14-11-2026.md). Línea base Belvedere: V5 20,9 s,
v7.5 40,2 s (razón 1,92). La diferencia está en construir la jerarquía (+12,4 s, muestreo). TC3, TC1 y
TC2(b) dan salida idéntica pero ≤ 8 % de mejora: salta el criterio de parada. Nada se integra; las ramas
`explore/convert-speed` y `explore/convert-batch` se conservan como evidencia.

- [x] C4. La copia diferida del más cercano se probó y se descartó: −1,8 % en la jerarquía, razón v7.5/V5 en
  ~2,0 ([registro](../records/benchmarks/C4_DEFERRED_COPY_15-11-2026.md)). El propietario revisó RQ-07 en
  lugar de TC5 ([ADR 0043](../adr/0043-coste-de-conversion-de-v7-frente-a-v5.md)).

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
  - [x] D3b. Escritor híbrido detrás de una opción del escritor (`--row-order hybrid`, `rowOrder`), sin
    identidad propia ([ADR 0042](../adr/0042-escritor-hibrido-en-la-identidad-final-de-v7.md),
    [registro](../records/benchmarks/D3B_HYBRID_WRITER_15-11-2026.md)): Belvedere 815 → 531 MB con un +1,3 %
    de tiempo total; Majadas sin cambio apreciable (+0,26 % sobre su óptimo).
  - [x] D3c (15-11-2026; [plan](d3c-congelado-v7-final.md), [ADR 0044](../adr/0044-v7-final-congelada-y-predeterminada.md); registros [RQ-07](../records/benchmarks/D3C_RQ07_V7_15-11-2026.md), [web Belvedere](../records/benchmarks/D3C_WEB_BELVEDERE_15-11-2026.md) y [referencias](../records/benchmarks/D3C_V7_REFERENCES_15-11-2026.md)). Congelado (RQ-07 revisado por el ADR 0043): identidad final de V7 con el escritor híbrido por defecto (unos 30
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

Viven en issues con la etiqueta [`pregunta-propietario`](https://github.com/Mili-ssm/OctreeViewer/issues?q=is%3Aopen+label%3Apregunta-propietario):
RF-05 ([#17](https://github.com/Mili-ssm/OctreeViewer/issues/17)), OQ-10 ([#18](https://github.com/Mili-ssm/OctreeViewer/issues/18)), B1b.3 ([#19](https://github.com/Mili-ssm/OctreeViewer/issues/19)) y ADR 0008
([#20](https://github.com/Mili-ssm/OctreeViewer/issues/20)); el codificador propio del ADR 0025 va tras el congelado ([#14](https://github.com/Mili-ssm/OctreeViewer/issues/14)).
