# Conversión paralela en el CLI y en el navegador (plan de implementación)

Estado: aprobado · 21-11-2026 · aprobado por el propietario; petición del propietario («continúa con el conversor web paralelo, y en CLI también
debería poder ejecutar en paralelo», 21-11-2026), que deja sin efecto la condición del plan anterior «M4/M5 solo si M6
supera 60 min»; pre-mortem hecho (enmiendas A1–A9 incorporadas)

Avance (21-11-2026): tareas 0, 1 y 2 hechas en el CLI (resultados en sus casillas); siguiente: tarea 3.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** que una conversión aproveche los núcleos disponibles en el CLI (más allá del techo actual de ~2,6×) y en el
navegador (hoy un solo worker, todo en serie), con **los mismos bytes** que la conversión en serie, en V8.

**Architecture:** el núcleo `viewer_convert_core` ya trocea el trabajo sin E/S ni hilos: rangos de la partición raíz
(`RangeJob`), subárboles (`NodeJob`, con cola de listos y *commit* en preorden), preparación por bloque
(`prepare_planned_block`) y escritura ordenada (`write_prepared`). Los ids de nodo y bloque se asignan en preorden al
hacer *commit*, y nada que dependa del reparto llega a los bytes de salida. El CLI reparte esas unidades entre hilos; el
navegador las ejecuta en serie (`convert_serial`). Este plan:
1. mide el camino crítico;
2. quita los techos del reparto nativo;
3. en el navegador, primero comprime los spools y después lleva el reparto a un pool de workers sin memoria compartida
   (ADR 0015), con un coordinador «por pasos» que sirve los spools y un único escritor.

**Spec:** el plan anterior, en pausa desde el 30-10-2026 ([conversor web paralelo](conversor-web-paralelo.md): R1–R10,
M4 y M5), que este sustituye, más los ADR [0014](../adr/0014-nucleo-de-conversion-sin-io.md),
[0015](../adr/0015-pool-de-workers-sin-memoria-compartida.md), [0017](../adr/0017-zonas-opfs-paginadas-para-spools-web.md)
y [0024](../adr/0024-hilos-wasm-opcionales-con-coop-coep.md).

## Situación de partida (medida el 21-11-2026, `main` bb4e889, equipo del propietario, 20 hilos)

| Nube | W = 1 | W = 2 | W = 4 | W = 8 | Notas |
| --- | --- | --- | --- | --- | --- |
| Belvedere (CLI) | 132 s | 78 s (×1,69) | 53 s (×2,50) | 49 s (×2,73) | 2 ejecuciones por W; CPU 117 % → 395 % |
| Majadas (CLI) | 843 s | — | 330 s (×2,56) | 325 s (×2,59) | 1 ejecución por W; CPU 109 % → 337 % |

- SHA idéntico para todo W en las dos nubes. `--pipeline-writes` no cambia el tiempo.
- **Por qué no escala de 4 a 8 (código, a confirmar con la tarea 0):**
  - la jerarquía bajo la raíz usa `threads = tasks.len()` (`crates/viewer_convert/src/executor.rs:267`): mín(W, hijos
    ocupados de la raíz), con una barrera por lote de W hijos (`builder.rs` ~723-747). Una nube de terreno en una raíz
    cúbica ocupa ≤ 4 hijos (inferido);
  - la partición de la raíz usa 4 rangos fijos, también con W = 1 (`options.rs:10`), lo que explica el 117 % de CPU con
    W = 1;
  - con W alto, el presupuesto por rama devuelve `MemoryBudgetExceeded` en lugar de bajar W.
- **Navegador:** un worker, `convert_serial` y `validate_output_serial`; `spatialWorkers` no tiene efecto. Con un worker
  ya llega a 3,44 GiB de PSS con 4 GB de presupuesto ([M3b](../records/benchmarks/M3B_BELVEDERE_2026-10-30.md), era V7).
  Sus spools no están comprimidos (la compresión de #108 es solo nativa). Las llamadas wasm son síncronas y, sin
  `SharedArrayBuffer`, el coordinador no puede esperar a un worker dentro de una llamada: hace falta un conductor por
  pasos (ADR 0017 ya lo anticipa).
- **GitHub Pages no admite COOP/COEP** (2026): la demo pública solo puede usar el pool de workers del ADR 0015.

## Global Constraints

- **Bytes idénticos (R1):** para la misma entrada y opciones, la salida es la misma con cualquier número de workers,
  en el CLI y en el navegador, e igual entre ambos (RQ-08). Los SHA fijados de V8 (`crates/viewer_convert/src/lib_tests.rs`)
  no cambian. El coordinador es el dueño de la reserva de ids (`node_job()`), del cursor de *commit* y del escritor; los
  contadores por worker no se usan.
- **Sin regresión con W = 1:** el CLI con W = 1 no empeora más de un 3 % (mediana pareada, ≥ 5 rondas A/B en
  Belvedere). Los rangos de la partición raíz siguen separados de W.
- **Memoria acotada:** el presupuesto (`--max-memory-mib`, `ram_budget`) manda; W y N efectivos se calculan a partir de
  él antes de empezar y se informan, en vez de fallar.
- **Sin COOP/COEP exigido** (ADR 0024); el navegador funciona desde GitHub Pages.
- Topes separados: el CLI puede subir W; el wasm mantiene su tope y sigue aceptando `spatialWorkers: 4`.
- `scripts/check.sh --offline` verde tras cada tarea (con `FORMAT_PARITY_REAL_INPUT` y `RENDER_DATA_DIR`);
  Playwright para todo cambio en `examples/web-host/`.
- Cifras con mediana y dispersión; solo valen para su revisión, perfil, navegador, dataset y equipo.
- Nunca `cargo clean` ni borrar `target/`.

## Review Focus

1. **W > núcleos o W > presupuesto:** W efectivo se reduce y se informa; nunca `MemoryBudgetExceeded` por subir W.
2. **Cancelación y errores en el pool web:** si un worker falla o el usuario cancela, todos terminan y se borran sus
   temporales OPFS; Chrome libera los handles síncronos ~2 s después de `terminate`, así que la limpieza lo espera.
3. **Orden de escritura:** con trabajos que terminan desordenados, la salida es la de la serie.
4. **Cuota y páginas OPFS con N workers:** el pico de temporales no crece más allá de lo estimado y no se agotan las
   páginas de zona a mitad de conversión; si no cabe, se rechaza antes de empezar.
5. **Navegadores y equipos pequeños:** Chrome/Edge es la puerta, Firefox mejor esfuerzo; con `hardwareConcurrency` = 2
   o poca RAM, el pool queda en 1–2 workers sin error.

---

### Task 0: Critical path and I/O attribution (gate)

- [x] Con `real_dataset_bench convert-profile` (ventanas de pared, eventos por nodo, E/S del proceso), reconstruir el
  camino crítico de Belvedere (×3) y Majadas (×1) con W = 1, 4, 8, y W = 1 con 0 y 4 rangos raíz. Registrar cuántos
  hijos de la raíz están ocupados y su reparto de bytes de spool (no de puntos).
- [x] Atribución de E/S: temporales y salida en tmpfs frente a btrfs (con y sin compresión), `iowait`, tiempo de sistema
  y presión de E/S (PSI) de `/proc`; la CPU de hilos del kernel no es medible (contador inutilizable).
- [x] Navegador con un worker en V8 (Belvedere y Majadas): tiempo por fase y RSS en Node con zonas en `fs`, con una
  sonda mínima de conversión; si Majadas cabe en la cuota de OPFS, PSS y Chrome quedan para la tarea 5.
- [x] Registro fechado ([CONVERSION_PARALELA_T0](../records/benchmarks/CONVERSION_PARALELA_T0_21-11-2026.md); la puerta no salta: con W = 4, tmpfs y btrfs difieren < 5 %). **Puerta:** si tmpfs y btrfs difieren > 10 % en pared, el límite es E/S y las tareas 1–2 se
  reajustan antes de seguir.

### Task 1: Native ready queue on W threads

- [x] La cola de listos de `NodeJob` corre en W hilos (no `tasks.len()`), en un solo lote sin barrera por lote de
  hijos, conservando el cursor de *commit* en preorden.
- [x] W efectivo se calcula del presupuesto antes de planificar (sin `MemoryBudgetExceeded` por W alto); el tope del
  CLI pasa de 8 a los núcleos disponibles; el del wasm no cambia (test de bindings con `spatialWorkers: 4`).
- [x] Tests de invariancia de bytes con W = 1, 2, 4, 8, 16 (unitarios, tarea 1a; en datos reales W = 1, 4, 8, 16); W = 1 dentro del 3 %; registro con la nueva curva
  ([CONVERSION_PARALELA_T1](../records/benchmarks/CONVERSION_PARALELA_T1_21-11-2026.md): W = 1 −1,2 %; SHA idéntica en todas las W).
- [ ] **Criterio:** si Majadas con W = 8 no baja de 1,3× frente a W = 4, el límite son los escaneos de nivel 1 → tarea 2.
  Resultado medido: Majadas W = 8 frente a W = 4 es ×1,19 (< 1,3) → la tarea 2 es la siguiente.

### Task 2: Range partitioning of large inner nodes (spike first)

- [x] **Spike:** partir por rangos (`RangeJob`) los nodos internos grandes, sobre las extensiones de sus spools hijos,
  como ya se hace con la raíz; medir en Majadas
  ([CONVERSION_PARALELA_T2_SPIKE](../records/benchmarks/CONVERSION_PARALELA_T2_SPIKE_21-11-2026.md): con lector
  encadenado, jerarquía de Majadas W = 8 de 161,6 a 108–110 s, bytes idénticos; con unión por copia, más lento).
- [x] Si el spike mejora ≥ 1,3× sin cambiar bytes: implementarlo con tests de invariancia; si no, registrar por qué y
  cerrar la tarea.
  Hecho ([CONVERSION_PARALELA_T2](../records/benchmarks/CONVERSION_PARALELA_T2_21-11-2026.md)): bytes idénticos,
  W = 1 +1,0 %, jerarquía de Majadas W = 8 110,0 s y total W = 8 frente a W = 4 ×1,34.

### Task 3: Browser spools compressed

- [ ] Llevar la compresión de spools de #108 al `HybridSpoolStore` del navegador (misma compresión que el nativo; se
  exige paridad de la salida, no de los spools). Medir el pico de RAM y de OPFS de Belvedere y Majadas con un worker.
  Va antes del pool porque reduce la memoria que cada worker necesita.

### Task 4: Browser worker pool

- [ ] **4a, ADR del conductor por pasos:** el coordinador pide «siguiente trabajo» y recibe «resultado» (sin esperar
  dentro de una llamada wasm); sirve los spools en trozos acotados; es dueño de ids, *commit* y escritor. Precisa los
  ADR 0015 y 0017 (nuevo ADR si cambia alguna decisión) y registra que el propietario dejó sin efecto la condición de
  M6 > 60 min.
- [ ] **4b, spike y M4:** pool de N workers con el `WebAssembly.Module` compilado una vez y enviado a cada uno;
  preparación de bloques en el pool, conducida desde JS, y escritor único en orden. Medir con Belvedere en Chrome:
  N = 1, 2, 4, la PSS por worker añadido (repartir `ram_budget` entre spools del coordinador y N × tope por worker;
  reciclar workers tras trabajos grandes) y el coste de copiar entre memorias wasm (transferir no evita las copias
  dentro y fuera de la memoria lineal).
- [ ] **Arnés de paridad del pool** en `check.sh`, sobre `worker_threads` de Node: N = 1, 2, 4, con fallo de un worker
  y cancelación; Playwright para Chrome.
- [ ] **4c, M5 (solo tras un sí o no del propietario con las cifras de 4b):** partición raíz y subárboles en el pool.
  Requiere rehacer el modelo de zonas para subárboles en paralelo (o margen y rechazo previo); Majadas web como puerta.
  Zonas por worker solo si un spike de handles compartidos (Chrome y Firefox) pasa.
- [ ] N por defecto = mín(`hardwareConcurrency` − 1, lo que permita `ram_budget` según la PSS medida), configurable
  desde la API.

### Task 5: Measurement, UI and docs

- [ ] Antes del registro final: modelo de memoria de los rangos de nodos partidos que admisión e informe calculen igual
  ([#120](https://github.com/Mili-ssm/OctreeViewer/issues/120)), y medir la partición de la raíz con 8 rangos en vez de 4
  (la raíz y los bloques limitan con W = 8–16, ver
  [CONVERSION_PARALELA_T2](../records/benchmarks/CONVERSION_PARALELA_T2_21-11-2026.md)).
- [ ] Registro final: CLI (Belvedere, Majadas, Albuera; W = 1…núcleos) y navegador (Belvedere y Majadas en Chrome,
  N = 1, 2, 4; Firefox mejor esfuerzo), con mediana y dispersión, picos de RAM y de OPFS.
- [ ] La UI del conversor muestra los workers en uso y permite fijarlos; README de la demo y del CLI.
- [ ] Playwright: varios workers, cancelación a mitad, error de un worker, cuota insuficiente.
- [ ] Documentación: inventario, requisitos (RQ-07, RF-28), `estado.md`; el plan anterior pasa al archivo como
  sustituido.

## Criterios de parada

- **Bytes:** una diferencia entre W, entre N o entre web y CLI que no se explique → parar y llevarlo al propietario.
- **W = 1:** empeora más del 3 % → revertir el cambio que lo cause.
- **E/S:** tmpfs y btrfs difieren > 10 % en pared → reajustar las tareas 1–2.
- **CLI:** tras la tarea 1, Majadas W = 8 < 1,3× W = 4 → tarea 2 (spike) antes de subir más W.
- **Web, M4:** con N = 4 la fase de bloques baja < 1,5× → no se empieza M5.
- **Web, memoria:** PSS con N = 2 > `ram_budget` (4 GB por defecto) → pool apagado por defecto y se lleva al
  propietario.
- **Web, zonas:** agotar páginas de zona o superar el presupuesto de PSS en Majadas → parar.

## Después de este plan

[#112](https://github.com/Mili-ssm/OctreeViewer/issues/112): nube densa y pesada para probar el render.
