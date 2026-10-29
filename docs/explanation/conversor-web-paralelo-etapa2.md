# Conversor web paralelo — plan de la etapa 2

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** cerrar las tres incógnitas que dejó la etapa 1 (pico de los trabajos por nodo, S0
con bloques reales, cuota real) y llegar a M1: el pipeline nativo expresado con traits y trabajos
por nodo dentro de `viewer_convert`, con los mismos bytes y sin regresión.

**Arquitectura:** la [spec](conversor-web-paralelo.md) es la autoridad. Decisiones del propietario
del 29-10-2026 que este plan aplica: **trabajos por nodo** (cada trabajo lee la entrada de un nodo y
escribe sus hijos en una arena nueva; el host borra la arena del padre cuando todos sus hijos la han
consumido), **Chrome `file://` solo con spools en RAM** (R8) y **medir la cuota real** antes de M6
(S8). Etapa anterior: [etapa 1](conversor-web-paralelo-etapa1.md); evidencia:
[referencia M0](../records/benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md) y
[spike S1](../records/benchmarks/OPFS_ARENA_SPIKE_2026-10-29.md).

**Tecnología:** Rust nightly fijado, `viewer_convert` (planificador en `src/spatial.rs`),
`tools/format_parity` (digest nativo/wasm32 bajo node), OPFS `FileSystemSyncAccessHandle`,
Playwright para los navegadores.

**Spec:** [conversor-web-paralelo.md](conversor-web-paralelo.md).

## Restricciones globales

- `scripts/check.sh` en verde (todos los pasos) al cerrar cada tarea.
- No cambiar la fecha del nightly ni añadir `#![feature]`.
- `FormatVersion::V<n>` solo en `crates/viewer_format`.
- R1: los SHA de salida del CLI no cambian; referencia: [M0](../records/benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md) (LAS pequeño `413858a9…`, Belvedere `6ca0f86d…`, Belvedere divisiones 8 `fb90a490…`, Majadas `74226286…`).
- R7: mediana pareada de `real_dataset_bench convert` en Belvedere (4 workers, perfil `perf`), ≥ 5 rondas A/B alternadas, dentro de ±3% de la rama base medida en la misma sesión.
- Registros nuevos fechados con el día de ejecución (`dd-mm-aaaa` en el texto, `<aaaa-mm-dd>` ISO en el nombre), en español; no editar registros ya mergeados; no trasladar cifras entre revisiones, perfiles ni equipos.
- Commits convencionales en inglés con la línea `Co-Authored-By` del modelo que los escribe; sin push.
- No enviar avisos upstream (backlog, prioridad 9 del README).

## Review Focus

1. **Raíz con todos los puntos iguales o no divisible** con el planificador por nodos → Tarea 4 (tests de la etapa 1 en verde y `coincident_fixture` en la matriz).
2. **Spools vacíos** (hijos sin puntos) en arenas por nodo: no deben crear arena ni romper el orden de identificadores → Tarea 4.
3. **Bloques reales con atributos dispersos** (clasificación, retornos, GPS) en S0 → Tarea 2 usa bloques de Belvedere, no sintéticos.
4. **Cuota denegada o `persist()` rechazado** en S8 → Tarea 3 registra el comportamiento, no solo el caso feliz.
5. **Errores con índice global de punto** al partir la entrada por rangos de chunks → Tarea 4 compara el mensaje de error de un LAS corrupto entre ejecución en serie y por rangos.

---

### Tarea 1: modelo de pico de los trabajos por nodo (S1b)

**Archivos:**
- Modificar: `crates/viewer_convert/src/spatial.rs` (registro de eventos de spool, solo con diagnósticos activos), `crates/viewer_convert/examples/real_dataset_bench.rs`
- Crear: `tools/arena_model/` (binario de simulación, `publish = false`) o un script Python en `crates/viewer_convert/benchmarks/` si encaja mejor con los existentes
- Crear: `docs/records/benchmarks/NODE_JOB_ARENA_MODEL_<aaaa-mm-dd>.md` y `docs/records/benchmarks/data/node-job-arena-model-<aaaa-mm-dd>.json`

**Interfaces:**
- Produce: un registro de eventos por nodo en el orden real de ejecución nativa, con `node_id`, `parent_id`, `depth`, `input_spool_bytes`, `child_spool_bytes: Vec<u64>` (por hijo) y el instante relativo de inicio y fin del nodo (secuencia, no reloj). Solo se emite con `collect_spool_diagnostics`; no cambia bytes de salida.

- [ ] **Paso 1: test del registro** — con la fixture de 4 000 puntos y divisiones 4: la suma de `child_spool_bytes` de todos los eventos es igual a `child_spool_bytes_written` de los diagnósticos actuales, y cada `parent_id` aparece antes como `node_id`.
- [ ] **Paso 2: implementar el registro** junto a los contadores de la etapa 1 (`SpoolLedger`, `spatial.rs:616`; `build_root_from_las` `:803`, `build_node` `:977`, `process_child_tasks` `:1158`). Comprobar los SHA de LAS pequeño y Belvedere contra M0.
- [ ] **Paso 3: simulación** — reproducir los eventos con N trabajos concurrentes (N = 4 y 8, reparto en el orden de los eventos): cada trabajo de nodo crea una arena con todos sus hijos; la arena del padre se borra cuando el último hijo termina su trabajo; las arenas hoja viven hasta la fase de bloques. Pico = máximo de la suma de arenas vivas. Ejecutar con Belvedere (divisiones 2 y 8) y Majadas (divisiones 2).
- [ ] **Paso 4: registro** — tabla pico modelado frente a `peak_live_spool_bytes` nativo de la misma ejecución, con la fórmula y supuestos. Criterio de parada (spec): > 1,5 × → volver al propietario antes de la Tarea 4.
- [ ] **Paso 5: commits** — `feat(convert): log per-node spool events for arena modelling` y `docs(bench): record per-node job arena model`.

---

### Tarea 2: S0 con bloques reales de Belvedere

**Archivos:**
- Modificar: `tools/format_parity/src/lib.rs`, `tools/format_parity/src/bin/native_digest.rs`, `tools/format_parity/run-wasm.mjs`, `scripts/check.sh`
- Crear: un volcador de bloques (`tools/format_parity/src/bin/dump_blocks.rs`) que escribe los primeros K = 4 `SourcePointBlock` planificados de una conversión real a un fichero binario en `target/format_parity/`

**Interfaces:**
- Consume: `parity_digest()` y `format_parity_digest()` de la etapa 1 (`lib.rs:178`, `:193`).
- Produce: `pub fn parity_digest_of_blocks(serialized: &[u8]) -> u64`; exportaciones wasm `format_parity_alloc(len: usize) -> *mut u8` y `format_parity_digest_blocks(ptr: *const u8, len: usize) -> u64`; `run-wasm.mjs` acepta una ruta opcional y copia el fichero en la memoria wasm.

- [ ] **Paso 1:** volcar 4 bloques de Belvedere (`data/datasets/belv_2009_digital-aerial_pcd_copc.laz`, opciones por defecto del CLI) y comprobar que el digest nativo de esos bloques es estable entre dos ejecuciones.
- [ ] **Paso 2:** digest wasm32 del mismo fichero bajo node → igual al nativo.
- [ ] **Paso 3:** paso opcional en `scripts/check.sh` («format parity real blocks»), que se omite con un aviso si no existe el fichero de Belvedere, igual que `FUZZ_CHECK` (`check.sh:39-42`); el paso sintético existente no cambia.
- [ ] **Paso 4: commit** — `test(format): check native/wasm32 parity on real Belvedere blocks`. Cierra el criterio S0 de la spec.

---

### Tarea 3: cuota real de OPFS (S8)

**Archivos:**
- Crear: `docs/records/benchmarks/data/opfs-quota-<aaaa-mm-dd>.{html,js,jsonl}` y `docs/records/benchmarks/OPFS_QUOTA_<aaaa-mm-dd>.md`

- [ ] **Paso 1:** página que llama a `navigator.storage.persisted()`, `persist()` y `estimate()` y escribe en OPFS hasta `QuotaExceededError` (bloques de 1 MiB, borrando al terminar), registrando la cuota declarada, los bytes escritos al fallar y el resultado de `persist()`.
- [ ] **Paso 2:** ejecutar en Chrome y en el Firefox del sistema (158), con perfil nuevo persistente y con `persist()` concedido y denegado, por `http.server` y por `file://` (en Chrome `file://` se espera `SecurityError`, ya registrado en S1). Si conceder `persist()` exige un diálogo que Playwright no maneja, documentar el paso manual y repetirlo a mano.
- [ ] **Paso 3: registro** — tabla por navegador × modo × `persist()`; conclusión explícita: ¿cabe Majadas (≈ 19 GiB de spools vivos, según el modelo de la Tarea 1, más ~7,5 GB de salida)? Si no cabe en ningún perfil, volver al propietario antes de M6.
- [ ] **Paso 4: commit** — `docs(bench): record real OPFS quota per browser`.

---

### Tarea 4: M1 — traits y trabajos por nodo dentro de `viewer_convert`

Empieza solo si la Tarea 1 no activa su criterio de parada.

**Archivos:**
- Modificar: `crates/viewer_convert/src/spatial.rs`, `crates/viewer_convert/src/las_source.rs`, `crates/viewer_convert/src/lib.rs`
- Crear: `crates/viewer_convert/src/pipeline/` con `spool_store.rs`, `point_source.rs`, `clock.rs`, `node_job.rs`

**Interfaces** (los nombres son los de la spec; las firmas exactas las fija el paso 1 y se anotan aquí antes de implementar):
- `SpoolStore`: crear una arena por trabajo, añadir extensiones de spool, leer un spool lógico (lista de extensiones), liberar una arena; implementación nativa sobre ficheros con los mismos bytes que hoy.
- `PointSource`: leer puntos por rango de chunks LAZ o de registros LAS, con offset global de punto; sin `Reader::seek` en chunks variables (spec, bloqueos comprobados).
- `Clock`: `Instant` en nativo.
- `NodeJob`: entrada = nodo e identificador de spool de entrada; salida = hijos (identificadores de spool, recuentos y acumuladores LOD) y bloques planificados; el planificador nativo ejecuta trabajos con `std::thread::scope` y fusiona en preorden como `append_branch`.

- [ ] **Paso 1: diseño de firmas** — un arquitecto (Opus) fija firmas y el flujo de trabajos por nodo a partir de `spatial.rs` (`build_node` `:977`, `process_child_tasks` `:1158`) y lo anota en este documento; revisión antes de seguir.
- [ ] **Paso 2: `Clock` y `PointSource`** sin cambio de bytes; tests existentes y SHA M0.
- [ ] **Paso 3: `SpoolStore` nativo** con arenas por trabajo; SHA M0.
- [ ] **Paso 4: planificador por trabajos de nodo** (sustituye el paralelismo solo en hijos raíz); tests de independencia de la etapa 1 (`lib_tests.rs`) y los casos del Review Focus 1, 2 y 5.
- [ ] **Paso 5: R7** — ≥ 5 rondas A/B alternadas frente a la rama base en la misma sesión; registro fechado. Criterio de parada (spec): regresión > 3% tras dos intentos → revertir la extracción.
- [ ] **Paso 6: commits** por paso, convencionales.

---

## Después de la etapa 2

Se planifican con los resultados de las Tareas 1–4, en este orden (spec, «Hitos»):

| Hito | Contenido | Depende de |
| --- | --- | --- |
| M2 | Crate `viewer_convert_core` sin I/O que compila para wasm32; partición por rangos ejecutada en nativo con W = 1..8 y mismos SHA | Tarea 4 |
| M3a | Conversión web con un worker sobre el núcleo, LAS pequeño, paridad en Chrome y Firefox | M2, Tarea 2 |
| M3b | Un worker con arenas OPFS a escala Belvedere; eliminar `browser_convert.rs`; Chrome `file://` solo RAM (R8) | M3a |
| M4 | Pool de preparación y escritor ordenado; dimensionado S7 | M3b |
| M5 | Pool de partición y trabajos por nodo; validación paralela | M4 |
| M6 | Escala Majadas: preflight de cuota, spools en RAM con volcado, limpieza | M5, Tarea 3 |
