# Reorganización por componentes, bloque 1 (plan de implementación)

Estado: en curso · 28-11-2026 · pre-mortem hecho (enmiendas incorporadas) · pedido por el propietario («procede, comienza ya con el primer bloque y después vayamos poco a poco», 28-11-2026)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** que los ficheros en disco sigan el árbol de módulos y los componentes (qué cambia junto), sin
cambiar comportamiento: render en `cells/`, wasm en `lod_dataset/`, tests gigantes divididos, sondas del
web host en `probes/`, índice de registros propio y un check que vigile la organización.

**Architecture:** solo movimientos. Los ficheros se mueven con `git mv` o se dividen por rangos de líneas
con `split.py` (copia byte a byte); a mano solo cambian líneas `use`, `mod`, `#[path]`, reexportaciones y
visibilidad (`pub(super)`/`pub(crate)`). La API pública del crate de render no cambia.

**Tech Stack:** Rust (workspace nightly), bash, Python 3 stdlib, JS (web host).

**Origen:** revisión de arquitectura del 28-11-2026 (tres revisiones `architect`; hallazgos 1, 2, 3, 7, 8 y 11
del resumen al propietario). Fuera de este bloque, por chocar con la PR #122: ciclo `pipeline`↔`spatial`,
`lib_tests.rs` de `viewer_convert`, `spool_frame`, reexportaciones antiguas de `viewer_convert`.

## Global Constraints

- Ningún cambio de comportamiento ni de bytes de salida; ninguna línea de código fuera de `use`/`mod`/`#[path]`/reexportaciones/visibilidad, salvo donde una tarea lo diga.
- Disposición de módulos: fichero padre `x.rs` + carpeta `x/`, nunca `x/mod.rs`.
- Los tests siguen en ficheros hermanos `x_tests.rs` enlazados con `#[path = "x_tests.rs"]` (convención del workspace).
- `docs/records/` es inmutable: no se edita salvo enlaces rotos; los ADR aceptados conservan los nombres antiguos.
- Commits en inglés, Conventional Commits (`refactor:`, `docs:`, `build:`), un commit por tarea como mínimo, sin menciones a IA.
- No ejecutar `cargo clean` ni borrar nada en `target/` (contiene datasets de prueba).
- Puerta por tarea: `.superpowers/sdd/limpieza/gate/gate.sh run <crates>` (fmt, clippy nativo y wasm32, doc, enlaces, índices, literales de versión, ningún `.rs` sin seguimiento, tests de node y Python del web host, `check_layout.py` cuando exista, tests de los crates). Referencia tomada el 28-11-2026 (`gate.sh baseline`, 1.315 tests).
- Lista de tests: un movimiento cambia el camino del módulo de los tests, así que la puerta compara el **multiconjunto de nombres hoja** y falla (`FAIL test leaf names`) si difiere; un test perdido o duplicado para la tarea.
- Regla de `#[path]` (rustc): un fichero cargado por `#[path]` resuelve sus `mod hijo;` como un `mod.rs` (en su propia carpeta, no en `<nombre>/`). Por eso un fichero que vaya a tener hijos se enlaza sin `#[path]`.

## Review Focus

- Un `#[path]` que deja de resolver compila igual si otro fichero con el mismo nombre existe: comprobar con `git status` que no queda ningún fichero huérfano en la raíz.
- Un test que desaparece de la lista (módulo no enlazado) pasa el gate en silencio: la comparación de nombres hoja lo detecta.
- Rutas en documentación viva (`docs/reference/inventory.md`, `CONTRIBUTING.md`, `docs/estado.md`, `examples/web-host/README.md`, `PROBES.md`) que apuntan a ficheros movidos: `scripts/check_docs_links.py` debe pasar.
- El sitio publicado no debe incluir sondas ni el spike: listar lo que copia `publish-demo.sh` antes y después.
- Visibilidad: una división que necesite `pub` (no `pub(super)`/`pub(crate)`) amplía la API; prohibido.

---

### Task 1: viewer_wasm, `lod_dataset/` sin `#[path]`

**Files:** `crates/viewer_wasm/src/lod_dataset.rs` y sus 7 `lod_*` enlazados por `#[path]`.

- [x] `git mv` según la tabla y borrar los `#[path]` correspondientes (los caminos de módulo no cambian):

| Antes (`src/`) | Después (`src/lod_dataset/`) |
| --- | --- |
| `lod_cells.rs` | `cells.rs` |
| `lod_dataset_tests.rs` | `tests.rs` |
| `lod_cells_tests.rs` | `cells_tests.rs` |
| `lod_cells_stream_tests.rs` | `cells_stream_tests.rs` |
| `lod_cells_bench_tests.rs` | `cells_stream_tests/bench.rs` |
| `lod_cells_sim_tests.rs` | `cells_stream_tests/sim.rs` |
| `lod_cells_glue_sim_tests.rs` | `cells_stream_tests/sim/glue.rs` |

- [x] `gate.sh run viewer_wasm` → `GATE PASS` y lista de tests idéntica (los caminos no cambian).
- [x] Actualizar menciones en documentación viva (`grep -rn 'lod_cells' docs/reference docs/estado.md CONTRIBUTING.md`); commit `refactor(wasm): move lod_dataset submodules into its folder`. Hecho 28-11-2026: GATE PASS, 1.315 nombres hoja idénticos, `viewer_wasm` 182 ok.

### Task 2: viewer_render_wgpu, subpaquete `cells/` (movimientos)

**Files:** los 24 `cell_*.rs`, `cell_pool/`, `node_cells.rs`, `node_cells_tests.rs`, `lib.rs`, `gpu.rs:10`, `gpu/{cells,init,pick,render}.rs`, `test_support.rs:138,237`, `examples/{cell_pool_bench,coverage_spike}.rs` (usan `cell_lod`/`cell_bench`; siguen por el alias). `viewer_wasm` no se toca (`cell_generation` sigue reexportado en la raíz).

**Interfaces:** Produces `crate::cells::{bench, codec, cull, cut, driver, lod, pages, pick, pool, range, render, node_cells}`; `lib.rs` mantiene todas sus reexportaciones actuales y añade `pub use cells::lod as cell_lod;` y `pub use cells::bench as cell_bench;`, así `viewer_render_wgpu::cell_lod::…` sigue valiendo.

- [x] Crear `src/cells.rs` (solo `mod` y comentario de una línea) y `git mv` `cell_X.rs` → `cells/X.rs`, `cell_X_tests.rs` → `cells/X_tests.rs`, `cell_pool/` → `cells/pool/`, `node_cells{,_tests}.rs` → `cells/`. Los tests GPU que cuelgan de `cell_render_tests.rs` por `#[path]` van a `cells/render_tests/{lod,cull,color,pick,driver}.rs` y se borran esos `#[path]`; **`render_tests` se declara en `cells.rs` como `mod render_tests;` sin `#[path]`** (con `#[path]` sus `mod lod;`… resolverían a los `cells/lod.rs` reales). Los `include_str!("shaders/…")` (`cell_pages.rs:506`, `cell_cull.rs:220,224`, `cell_render.rs:387,390,392`) pasan a `../shaders/…`: es la única edición permitida fuera de `use`/`mod`/visibilidad.
- [x] Ajustar `#[path]` restantes a los nombres nuevos, `crate::cell_X` → `crate::cells::X`, y las declaraciones de `lib.rs` (incluidos los tests que hoy cuelgan de la raíz: `cell_pages_tests`, `cell_render_tests`, `node_cells_tests`, que pasan a colgar de `cells`).
- [x] Commit `refactor(render): group the cell modules under cells/`, tras `gate.sh run viewer_render_wgpu viewer_wasm` → `GATE PASS` y nombres hoja idénticos. Hecho 28-11-2026: GATE PASS, 1.315 nombres hoja idénticos, render 466 ok, wasm 182 ok (`1808f2b`); revisión: 3 menores corregidos en la 2b.

### Task 2b: `NODE_WORDS` fuera de render y documentación viva

- [x] `NODE_WORDS` y `MASK_WORDS` se definen en `cells/pages.rs`; `cells/render.rs` hace `pub(crate) use super::pages::{NODE_WORDS, MASK_WORDS};` para no tocar sus otros usuarios; `cells/lod.rs` importa de `pages`. Sitios: `cell_render.rs:69,74`, `cell_lod.rs:13`, `cell_pick.rs:23`, `gpu/cells.rs:11`, `cell_render_tests.rs:14`, `lib.rs:69` (números de antes de la tarea 2). La reexportación pública de `lib.rs` apunta a `cells::pages`. Gate igual; commit `refactor(render): the cell page layout constants live in pages`.
- [x] Documentación viva: `inventory.md` y demás rutas `cell_*.rs` en `docs/reference`, `docs/estado.md`, `CONTRIBUTING.md`, `AGENTS.md` (ADR y registros no se tocan); `check_docs_links.py` en verde; commit `docs: point live docs at cells/`. Hecho 28-11-2026: GATE PASS, mismas cifras.

### Task 3: dividir `cells/pool_tests.rs` (6.172 líneas)

**Files:** `crates/viewer_render_wgpu/src/cells/pool_tests.rs` → `cells/pool/tests.rs` + `cells/pool/tests/<suite>.rs`.

- [x] `git mv cells/pool_tests.rs cells/pool/tests.rs` y en `cells/pool.rs` `mod tests;` **sin `#[path]`** (regla de `#[path]` arriba); gate verde antes de dividir.
- [x] Probar primero `split.py --dry-run` y `cargo check -p viewer_render_wgpu --tests` con un solo submódulo.
- [x] Spec de `split.py` (tabla de ítems con `items.py`, nunca a mano): el padre conserva la infraestructura (`Rig` y ayudantes, ~1–1349); cada suite a un submódulo según sus encabezados `// ----` (empiezan en ~1350, 1837, 3013, 4710, 5349): nombres por el requisito que prueban (p. ej. `fallback`, `refine`, `wait`, `stepping`, `data_bound`; confirmar con los encabezados reales).
- [x] Errores de compilación: solo `use`/`pub(super)` (`agent-skills:fixer` si son muchos); `moved.py` sin líneas alteradas salvo estructurales.
- [x] Antes de esta tarea, comentar en la PR #88 (aparcada, toca `cell_pool*`) que su rebase deberá portar los cambios a `cells/pool/tests/`.
- [x] `gate.sh run viewer_render_wgpu` → `GATE PASS`, nombres hoja idénticos; commit `refactor(render): split the cell pool tests by suite`.

Hecho 28-11-2026: GATE PASS, nombres hoja idénticos (1.315 tests; viewer_render_wgpu 466 pasan, 3 ignorados), `moved.py` sin líneas alteradas, 0 ediciones de visibilidad; los ayudantes usados por varias suites (y los `impl` de `Probe` y `Owner`) se quedan en `cells/pool/tests.rs`.

### Task 4: dividir `lod_dataset/cells_stream_tests.rs` (5.371 líneas)

**Files:** `crates/viewer_wasm/src/lod_dataset/cells_stream_tests.rs` y sus hijos `bench`/`sim`.

- [x] Con `split.py`, **la infraestructura y los oráculos (~43–1545) se quedan en el padre** (así sus campos y funciones privados siguen visibles para los hijos con `use super::*;`) y las suites salen a hijos: sintéticas (~1547–2056), color (~2429–3071), diagnósticos del plan ADR 0055 (~3072–5371, con 7 de sus 8 `#[ignore]`). Rangos a confirmar con `items.py`. Si hacen falta más de ~20 `pub(super)`, parar y replantear.
- [x] Mantener los nombres de los tests `#[ignore]` que citan los registros (nombre hoja idéntico).
- [x] `gate.sh run viewer_wasm` → `GATE PASS`, nombres hoja idénticos; commit `refactor(wasm): split the cell stream tests into support, suites and diagnostics`.

Hecho 28-11-2026: GATE PASS, nombres hoja idénticos (1315 tests), 1 `pub(super)` (`record_lag`); el padre queda en ~3.200 líneas con la infraestructura y los helpers compartidos, y salen `synthetic.rs` (381), `colour.rs` (548) y `diagnostics.rs` (1.238); los 20 `#[ignore]` de viewer_wasm se mantienen.

### Task 5: web host, sondas en `probes/`

**Files:** `examples/web-host/` (raíz), `scripts/publish-demo.sh:49-52`, `examples/web-host/package-local.py`, `PROBES.md`, `README.md` del web host, `docs/reference/inventory.md`, tests de `examples/web-host/tests/` que importen sondas.

- [ ] `git mv` a `examples/web-host/probes/`: `probe-*`, `spike-cube-pool.*`, `red-ref-server.mjs`, `pixel-metrics.mjs`, `sample-process-memory.py`, `collect-benchmarks.py`, `benchmark-metrics.mjs` si solo lo usan sondas, `run-r3-b6-matrix.sh`, `summarize-r3-b6.py`. `benchmark-metrics.mjs` se queda (lo importa `main.js`); en general, un fichero que importe la app (`main.js`, workers) se queda. `PROBES.md` se queda en la raíz.
- [ ] Rutas como texto que no son imports: `probe-cube-pool.mjs:17` (raíz del repo `"../.."` → `"../../.."`) y `:89` (`/examples/web-host/spike-cube-pool.html` → `/examples/web-host/probes/…`), `run-r3-b6-matrix.sh:35`, `tests/http-range-source.test.mjs:8` (`../red-ref-server.mjs`), `tests/test_sample_process_memory.py:8` y `tests/test_collect_benchmarks.py:8` (`parents[1]`); buscar el resto con `grep -rn 'web-host/\|\.\./\.\.' examples/web-host/probes`.
- [ ] Ajustar imports relativos (`./viewer-api.mjs` → `../viewer-api.mjs`, `../../pkg` → `../../../pkg`) y órdenes en la documentación.
- [ ] `publish-demo.sh`: quitar el `case` de exclusión; el glob no recursivo ya excluye `probes/`. Verificar con `bash -x` o listando: el conjunto de ficheros copiados al sitio es el mismo que antes.
- [ ] Tests del web host (`node --test examples/web-host/tests/*.test.mjs`, `python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'`, ambos ya en la puerta) y una sonda de humo en navegador con Playwright (`ui-verifier`): la demo carga y una sonda movida arranca. Commit `refactor(web-host): move probes and harness into probes/`.

### Task 6: índice propio de registros

**Files:** Create `docs/records/README.md`; Modify `docs/README.md:48-209`, `scripts/check_docs_index.py`.

- [x] Mover la tabla de registros de `docs/README.md` a `docs/records/README.md` (índice vivo, no registro), agrupada por línea de trabajo con encabezados que la nombran; corregir los prefijos de los enlaces. `docs/README.md` deja una línea con el enlace y la regla en «Dónde va cada cosa».
- [x] `check_docs_index.py`: los ficheros de `docs/records/` se buscan en `docs/records/README.md`.
- [x] `python3 scripts/check_docs_index.py` y `python3 scripts/check_docs_links.py` pasan; ninguna fila perdida (contar enlaces antes y después). Commit `docs: give the records their own index`.
- [ ] La PR #122 añade 4 filas a la tabla vieja (`CONVERSION_PARALELA_T0/T1/T2/T2_SPIKE`): comentar en #122 que, al rebasar, esas filas van a `docs/records/README.md`.

Hecho 28-11-2026: 155 filas movidas a `docs/records/README.md` en 8 grupos (18, 12, 13, 8, 50, 10, 13 y 31), 170 enlaces de tabla antes y después; `check_docs_index.py` ok (falla con un fichero sin enlazar, comprobado), `check_docs_links.py` ok, tests de `scripts/tests` ok. Pendiente el comentario en #122.

### Task 7: check de organización y convención escrita

**Files:** Create `scripts/check_layout.py`; Modify `scripts/check.sh`, `CONTRIBUTING.md` («Convenciones»), `docs/reference/inventory.md` si hace falta.

- [ ] `check_layout.py` (stdlib) falla si:
  - hay un `#[path]` (fuera de comentarios) cuyo destino no es un hermano `*_tests.rs` ni está en una lista de permitidos escrita en el script: hoy `reference_spacing_probe.rs`, `spatial_lod_priority.rs`, `row_order_tests.rs`, `tests/common/v8_fixture.rs` y `v8_tree.rs` de `viewer_format/tests`, y los de `viewer_convert/examples/{quantized_occupancy_bench,real_dataset_bench}.rs` (si pasan de 15 entradas, replantear la regla);
  - hay un `*.rs` en la raíz de `crates/viewer_render_wgpu/src` que empieza por `cell_` o en la raíz de `crates/viewer_wasm/src` que empieza por `lod_cells`;
  - los ficheros de política de CPU de `cells/` (`cut`, `lod`, `pool` y `pool/`, `driver`, `pages`, `codec`, `range`, `node_cells`, sin sus tests) mencionan `wgpu::`, `crate::gpu`, `cells::render` o `super::render`;
  - una arista entre crates del workspace (de `cargo metadata --offline --format-version 1 --no-deps`, dependencias normales y dev) no está en la lista de aristas permitidas del script (la actual).
- [ ] Probarlo en rojo: introducir temporalmente cada violación y ver que falla con mensaje `fichero:línea`; luego verde. Añadir `step "layout" python3 scripts/check_layout.py` a `check.sh` y a `gate.sh`.
- [ ] Una línea en CONTRIBUTING «Convenciones»: carpeta por componente, `x.rs` + `x/`, tests hermanos `x_tests.rs`, prefijo repetido = subcarpeta, y que lo vigila `check_layout.py`.
- [ ] `scripts/check.sh` completo en segundo plano → todo `ok`; commit `build: check the source layout`.
- [ ] Cierre: `Estado: completado` en este plan, moverlo a `docs/records/archive/` con enlaces corregidos y su fila en `docs/records/README.md`, nota en `docs/estado.md`, PR a `main`.
