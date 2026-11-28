# Reorganización por componentes, bloque 2b (plan de implementación)

Estado: en curso · 28-11-2026 · pre-mortem hecho (enmiendas incorporadas) · pedido por el propietario («continúa hasta terminar con los refactors y la limpieza», 28-11-2026)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** cerrar la reorganización sin cambiar comportamiento ni bytes: sin ciclo de módulos entre
`pipeline` y `spatial` en `viewer_convert_core`, sin shims de reexportación en `viewer_convert`, el
`lib_tests.rs` de 3.064 líneas dividido por tema, el experimento v6.2 retirado y un único doble de GPU
estricto.

**Architecture:** movimientos con `git mv` o `split.py` (copia byte a byte); a mano solo `use`, `mod`,
`#[path]`, visibilidad y comentarios que nombran rutas. La tarea 5 es la única que cambia código de
test (fusiona dos dobles); su criterio es no perder ninguna comprobación.

**Tech Stack:** Rust (workspace nightly), Python 3 stdlib.

**Origen:** revisión de arquitectura del 28-11-2026; queda fuera del bloque 2a
([plan cerrado](../records/archive/reorganizacion-bloque-2a.md)). Rama `refactor/layout-2b`, apilada sobre
`refactor/layout-2a` (PR #124).

**Decisiones del propietario (28-11-2026):** hacer el 2b ahora y que la sesión principal rebase después la
PR #122 encima (repartiendo sus tests nuevos de `lib_tests.rs` en los ficheros divididos); retirar
`quantized_occupancy_bench.rs`; unificar los dobles de GPU tras una feature `test-support` de
`viewer_render_wgpu` que `viewer_wasm` activa solo en `dev-dependencies`.

**Fuera de este bloque:** `viewer_format` plano (V8 congelada; se divide cuando haya trabajo de V9) y los
internos `doc(hidden)` del núcleo que usa `viewer_wasm` (ADR 0026: los crates son internos).

## Global Constraints

- Ningún cambio de comportamiento ni de bytes de salida (conversión, formato, render).
- Disposición: padre `x.rs` + carpeta `x/`, nunca `x/mod.rs` nuevo; tests en hermanos `x_tests.rs` con `#[path]`, salvo que el fichero de tests tenga hijos (un fichero cargado con `#[path]` resuelve sus `mod hijo;` como un `mod.rs`).
- ADR 0014: el núcleo de conversión no hace I/O de sistema (su `clippy.toml` prohíbe `std::fs`, hilos y tiempo).
- ADR 0026: los crates son internos; los módulos internos del núcleo se exponen `pub` + `#[doc(hidden)]` como los existentes.
- Commits en inglés, Conventional Commits, sin menciones a IA; no `cargo clean` ni borrar nada en `target/`.
- Puerta por tarea: `.superpowers/sdd/limpieza/gate/gate.sh run <crates>` → `GATE PASS` (multiconjunto de los 1.315 nombres hoja de test, `check_layout.py`, clippy nativo y wasm32, doc, enlaces).

## Review Focus

- Un test que deja de enlazarse (módulo no declarado) desaparece en silencio: la puerta compara nombres hoja.
- Rutas públicas del núcleo que cambian (`pipeline::node_job` → `spatial::node_job`…): sus usuarios están en `viewer_convert`, `viewer_wasm`, `tools/` y `fuzz/` (lockfile propio: `cargo check` en `fuzz/`).
- La fusión de los dobles de GPU no debe debilitar ninguna comprobación de riesgo de ninguno de los dos.
- Paridad: las tareas 1 y 2 ejecutan además `format parity wasm32` y `convert parity wasm32` de `scripts/check.sh`.

---

### Task 1: sin ciclo `pipeline`↔`spatial` en `viewer_convert_core`

`pipeline` son las costuras con el entorno (reloj, entrada de puntos, almacén de spools, cola de ejecución nativa);
`spatial` es el planificador del octree. Hoy `pipeline` contiene piezas del planificador y se importan en los dos sentidos.

**Files:** `crates/viewer_convert_core/src/pipeline/{node_job,nn_distance,executor}.rs` → `spatial/`; `pipeline/hybrid/estimate.rs` → `spatial/estimate.rs`; `pipeline/mod.rs` → `pipeline.rs`; `pipeline/hybrid/mod.rs` → `pipeline/hybrid.rs`; `spatial.rs`, `lib.rs`, `serial.rs`; usuarios externos (`viewer_convert_core/src/range_tests.rs`, `viewer_convert/src/{executor,paged_tests,lib_tests}.rs`, `viewer_convert/src/pipeline/node_job.rs`, `viewer_wasm`, `tools/`, `fuzz/`); `scripts/check_layout.py`; rutas en `docs/reference/format/FORMAT_V8.md:88` y `docs/plans/temporales-del-conversor-fase-a.md:152`. Enmiendas del pre-mortem: `pipeline/hybrid.rs` deja de reexportar `estimate` (lo reexporta `lib.rs` desde `spatial`); `spatial.rs:44` (aserción sobre `GLOBAL_MAX_INTERNAL_LEVEL`) cambia de ruta.

**Interfaces:** Produces `viewer_convert_core::spatial::{node_job, nn_distance, executor, estimate}` (los que hoy usan otros crates, `pub` + `#[doc(hidden)]`); las reexportaciones de la raíz (`estimate_spools`, `inspect_source`, `SpoolEstimate`, `NoClock`…) no cambian de nombre.

- [x] `git mv` de los cuatro ficheros y de los dos `mod.rs`; ajustar `use`, `mod` y visibilidad.
- [x] Regla nueva en `check_layout.py`: ningún fichero de `crates/viewer_convert_core/src/pipeline.rs` ni de `pipeline/` menciona `crate::spatial` ni `super::super::spatial` fuera de comentarios, también dentro de grupos `use {…}` de varias líneas (probarla con una infracción temporal).
- [x] Gate `viewer_convert_core viewer_convert viewer_wasm` + `format parity wasm32` + `convert parity wasm32` + `cargo check` en `fuzz/`; commit `refactor(convert): spatial planning leaves the pipeline seams`.
  Hecho 28-11-2026 (`51f0990`): `GATE PASS` (`viewer_convert_core viewer_convert viewer_wasm`; `docs index` exigía enlazar este plan en `docs/README.md`, se comprobó con la fila temporal y queda para la tarea 6), `format parity wasm32` y `convert parity wasm32` ok, `cargo check` en `fuzz/` ok.

### Task 2: sin shims de reexportación en `viewer_convert`

**Files:** `crates/viewer_convert/src/pipeline/{mod,clock,node_job,point_source,spool_store}.rs`, `diagnostics.rs`, `lib.rs` y sus usuarios (`executor.rs`, `las_source.rs`, `publish.rs`, `diagnostic_timer.rs`, `lib_tests.rs` y los `*_tests.rs`).

**Interfaces:** Produces `crate::pipeline` como el módulo de implementaciones nativas de las costuras: `pipeline.rs` con `SystemClock` (y nada que solo reexporte); los tests que hoy viven en los shims `pipeline/node_job.rs` y `pipeline/point_source.rs` pasan a hermanos `pipeline/node_job_tests.rs` y `pipeline/point_source_tests.rs` (o `pipeline_tests.rs` si es más simple) sin cambiar sus nombres hoja. Los usuarios importan las rutas del núcleo (`viewer_convert_core::…`), `crate::file_spool_store::FileSpoolStore` y `crate::diagnostic_timer::DiagnosticTimer` directamente.

- [x] Quitar cada `pub(crate) use …::*` y `diagnostics.rs`; reescribir los `use` de sus usuarios.
- [x] Gate `viewer_convert` + `convert parity wasm32`; commit `refactor(convert): import the core seams directly instead of re-export shims`.
  Hecho 28-11-2026 (`bc910cd`): gate PASS y paridad wasm32 ok; los tests de los shims pasan a `pipeline/{node_job,point_source}_tests.rs` (una firma se reajusta por rustfmt al quitar un nivel de sangría).

### Task 3: dividir `viewer_convert/src/lib_tests.rs` por tema

**Files:** `crates/viewer_convert/src/lib_tests.rs` → `lib_tests.rs` (fixtures y ayudas compartidas) + `lib_tests/{conversion,diagnostics,workers,planner,spool_lifecycle,serial_core,root_ranges,pinned_output}.rs`; `lib.rs` (`#[cfg(test)] mod lib_tests;` sin `#[path]`, para que los hijos resuelvan en `src/lib_tests/`).

Reparto orientativo por líneas actuales (el implementador lo ajusta a lo que comparte cada grupo; una ayuda usada por un solo grupo va con él):

| Fichero | Contenido |
| --- | --- |
| `conversion.rs` | conversión básica, atributos, fallos y cancelación (19–32, 302–500, 906–1201) |
| `diagnostics.rs` | `diagnostics_*` y `spool_diagnostics_*` (66–237, 1218–1329) |
| `workers.rs` | trabajadores espaciales y pipeline (503–903) |
| `planner.rs` | caracterización del planificador (1334–1866) |
| `spool_lifecycle.rs` | spools en memoria, preorden, `FailingStore`, cola lista (1874–2563) |
| `serial_core.rs` | núcleo serie frente a nativo, híbrido, validación serie (2567–2830) |
| `root_ranges.rs` | rangos de raíz y segmentos unidos (2834–2974) |
| `pinned_output.rs` | SHA fijadas de la salida V8 (2978–3064) |

- [x] El reparto se escribe **por nombre de ítem** (`lib_tests.names.json`: ítem → fichero, en `.superpowers/sdd/limpieza/gate/`) y un script `spec_from_names.py` genera de él el spec de líneas de `split.py` con `items.py`; así la tarea 7 repite el split sobre el `lib_tests.rs` de la #122 (enmienda del pre-mortem).
- [x] Dividir con `split.py`; `moved.py` sin líneas reescritas salvo `use`/`mod`/visibilidad.
- [x] Gate `viewer_convert`; commit `refactor(convert): split the crate tests by topic`.

### Task 4: retirar `quantized_occupancy_bench.rs`

- [x] Borrar `crates/viewer_convert/examples/quantized_occupancy_bench.rs` y su entrada de `ALLOWED_PATHS`; buscar sus menciones vivas fuera de `docs/records/` (el registro `docs/records/benchmarks/QUANTIZED_OCCUPANCY_BENCH.md` queda como histórico).
- [x] `check_layout.py` y `cargo check --examples`; commit `chore(convert): retire the v6.2 quantized occupancy bench`.
  Hecho 28-11-2026 (`5036b91`): el ejemplo no tenía tests y la API que usaba sigue en uso en `viewer_format`.

### Task 5: un único doble de GPU estricto

`viewer_render_wgpu/src/test_support.rs` (`FakeGpu`, `cfg(test)`) y `viewer_wasm/src/lod_dataset/cells_stream_tests.rs` (`StrictTarget`, que se declara reescritura de `FakeGpu` por no poder alcanzarlo) comprueban los mismos riesgos del protocolo del pool con estado distinto.

**Files:** `crates/viewer_render_wgpu/{Cargo.toml,src/lib.rs,src/test_support.rs}`, `crates/viewer_wasm/{Cargo.toml,src/lod_dataset/cells_stream_tests.rs}` y sus hijos que usen `StrictTarget`; `docs/reference/inventory.md` (feature nueva).

Enmiendas del pre-mortem: `FakeGpu` deduce la identidad de página de `box_max` de `fake_level` (`test_support.rs:82-109,246-251`) y fallaría con los `PackedLevel` reales de wasm, así que el doble único guarda `page_at` como `StrictTarget`; `test_support.rs` importa de `scene_digest_tests` (`cfg(test)`), que no compila con `--all-features` (las constantes y `cameras` pasan a `test_support`); el crate de render sí hereda los lints del workspace; `StrictTarget` devuelve `Err` en los riesgos (el driver recupera) y `FakeGpu` hace `panic`.

**Interfaces:** Produces la feature `test-support` de `viewer_render_wgpu` (apagada por defecto) que expone `pub mod test_support` (`#[cfg(any(test, feature = "test-support"))]`, `#[doc(hidden)]`) con el doble único; `viewer_wasm` la activa solo en `[dev-dependencies]`.

- [ ] **5a, spike:** convertir temporalmente los `Err` de riesgo de `StrictTarget` en `panic!` y pasar la suite de wasm. Si falla, parar e informar (riesgos latentes que la recuperación oculta); no relajar comprobaciones.
- [ ] **5b:** el doble único es la unión de las comprobaciones de ambos (ninguna se pierde ni se relaja) y los registros que usan los tests de ambos crates; `StrictTarget` desaparece. Si unir algún punto obligara a cambiar lo que un test afirma, parar e informar.
- [ ] `cargo tree -p viewer_wasm -e features --target wasm32-unknown-unknown` sin `test-support` fuera de los tests; clippy `--all-features` limpio.
- [ ] Gate `viewer_render_wgpu viewer_wasm`; commit `refactor(render): one strict GPU test double behind a test-support feature`.

### Task 6: documentación y cierre

- [ ] `docs/reference/inventory.md`, `docs/estado.md`, docstring de `check_layout.py`; fila del plan en `docs/records/README.md` al archivarlo en `docs/records/archive/`.
- [ ] `scripts/check.sh` 25/25 (en segundo plano); revisiones finales; PR con base `refactor/layout-2a`.

### Task 7: rebasar la PR #122 sobre `refactor/layout-2b`

- [ ] Antes: etiqueta local `pr122-pre-rebase` y lista de tests de la #122 (la `list()` de `gate.sh`, ejecutada en el worktree con su `CARGO_TARGET_DIR`) como baseline propio; `.superpowers/` no existe en el worktree (está en `.gitignore`), las herramientas se ejecutan por ruta absoluta.
- [ ] Método (pre-mortem): aplastar la #122 en un commit sobre su base, rebasarlo sobre `refactor/layout-2b` (git sigue los renombres de la tarea 1) y resolver `lib_tests` regenerando: el `lib_tests.rs` sin dividir de la #122 con los `use` de la tarea 2, `spec_from_names.py` con sus tests nuevos asignados y `split.py`; `moved.py` para comprobarlo. La línea que la #122 añade al shim `viewer_convert/src/pipeline/node_job.rs` (que la tarea 2 borró) debe reaparecer en `pipeline/node_job_tests.rs` o en el `use` que la sustituya.
- [ ] Criterio de aborto: si el multiconjunto de nombres hoja difiere del baseline, `git reset --hard pr122-pre-rebase` y replantear.
- [ ] `scripts/check.sh` verde en el worktree; nombres hoja de test: los de #122 antes del rebase.
- [ ] `git push --force-with-lease`; base de la PR #122 → `refactor/layout-2b`; comentario en la PR con lo que cambió de sitio.
