# Formato V8 congelado y retirada de los demás formatos (D4, plan de implementación)

Estado: aprobado · 21-11-2026 · pre-mortem hecho (enmiendas A1–A11 incorporadas); el propietario lo aprueba y pide fusionar #111 antes de empezar, así que la rama sale de `main`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** congelar la V8 sobre el layout de `v8.1-pre` con identidad final y dejar el visor y el conversor con **solo
V8**. Se retiran v5, v6, v7, todas las revisiones previas (también `v8.1-pre` y `v8.2-pre`), zstd, la entrada EPT y los
checkpoints.

**Architecture:** primero se añade la identidad final `V8`, igual que `V7` se creó sobre `v7.5-pre`
([ADR 0044](../adr/0044-v7-final-congelada-y-predeterminada.md)): mismos bytes que `v8.1-pre`, salvo la revisión de
cabecera y los magics. Pasa a ser la salida por defecto, y los fixtures se regeneran. Después se borra cada familia en
una tarea propia, de la más externa a la más interna: zstd, EPT y checkpoints, y por último las versiones antiguas. Así
cada tarea deja `scripts/check.sh` en verde y se puede revertir sola.

**Tech Stack:** Rust (nightly fijado), wasm-bindgen, Node/Playwright para la demo.

**Spec:** [formato-v8.md](formato-v8.md) («Solo V8 (D4)», resoluciones D3 y D4), [ADR 0058](../adr/0058-formato-v8-experimental.md),
[ADR 0060](../adr/0060-v8-solo-lz4.md) (solo LZ4), [ADR 0061](../adr/0061-ept-y-checkpoints-se-retiran-con-la-v8.md)
(EPT y checkpoints), [FORMAT_V8_PRE1](../reference/format/FORMAT_V8_PRE1.md).

**Rama:** `feat/formato-v8-final`. Precondición (A9): el propietario aprueba o congela #111 (`experiment/formato-v8-pre1`)
antes de la tarea 3; si se fusiona, la rama sale de `main`, y si no, de la rama del experimento, con su PR apilada. Solo se
rebasa entre tareas. Nada se fusiona sin el visto bueno del propietario.

## Global Constraints

- Nunca `cargo clean` ni borrar nada de `target/` (datos de prueba, 38 GB en el `target/` del checkout principal).
  Los worktrees usan su propio `CARGO_TARGET_DIR`.
- `FormatVersion::V<n>` solo dentro de `viewer_format` (`scripts/check_version_literals.sh`).
- Cada tarea termina con `scripts/check.sh --offline` en verde (24 pasos), con
  `FORMAT_PARITY_REAL_INPUT=<LAZ de Belvedere del checkout principal>`.
- Paridad web/CLI byte a byte (RQ-08) en V8, y nativo = wasm32 en los bloques.
- Documentación en español y código, comentarios y commits en inglés; fechas `dd-mm-aaaa`; sin menciones de IA.
- `docs/records/` no se edita: la evidencia nueva va en registros nuevos fechados.
- Publicar la demo (`scripts/publish-demo.sh`, push a `gh-pages`) es una acción externa: solo con el visto bueno
  explícito del propietario en ese momento.
- Los ficheros de datos ignorados (datasets y `target/render-data`) se reconvierten a ficheros nuevos; los antiguos se
  borran solo con el visto bueno del propietario.
- **Portar, no borrar (A1):** un test se borra solo si el código que cubre se va. Los tests de lo que se queda (lectura
  con presupuesto, lecturas paralelas, CRC antes de expandir, planes de lectura, planos de bytes, diccionario, índice
  paginado, marco global, filas híbridas, omisión de LOD, color derivado) se pasan a V8 aunque vivan hoy en
  `tests/v6.rs`, `v7_*.rs` o `v8_*.rs`. Puerta: el recuento de tests por módulo que se queda no baja (`cargo llvm-cov` si
  está instalado).
- **Salida V8 fijada (A3):** desde la tarea 1, el SHA-256 de la salida V8 de `data/samples/*.las` (test) y de Belvedere
  (test ignorado y registro) no cambia en las tareas 3–5; un cambio sin causa explicada para el plan y se biseca.

## Review Focus

1. **Fichero antiguo abierto** (v5, v6, v7 o `v8.N-pre`, en el visor, la demo, `verify` o el lector wasm): error claro
   que nombra la versión encontrada y pide reconvertir; nunca un pánico ni un error de parseo genérico. Lo fija un test
   en la tarea 5.
2. **Opciones retiradas** (`--format-version`, `--checkpoint`, `--resume`, `--attr-codec`, `--row-order`,
   `--points-per-block`, `--spatial-divisions`, una entrada EPT; en JS `formatVersion`, `attrCodec`, `rowOrder`): error
   claro, nunca ignoradas en silencio. Tests en las tareas 3, 4 y 5.
3. **Pareja `.oct`/`.octattrs` mezclada** (uno V8 y el otro antiguo): el lector la rechaza como hoy por identidad. Test
   en la tarea 1.
4. **Demo publicada**: la muestra y el recorte de Belvedere se regeneran juntos en V8; un visitante con caché no ve un
   error opaco. Comprobación en el navegador en la tarea 6.
5. **Salidas `v8.1-pre` del experimento** (p. ej. `target/v82-measure/`): el lector V8 las rechaza (otra identidad) con
   el mismo mensaje de reconvertir. Lo cubre el test de la tarea 5.

---

### Task 0: Gate — V1 and V4 before losing v7

V7 deja de escribirse en la tarea 5, así que la comparación con v7 se cierra antes.
- [x] **V1 (mismo dibujo, A7):** un ejemplo o test ignorado de `viewer_format` lee la misma entrada convertida en v7 y
  en `v8.1-pre`, ambas recién convertidas con el binario de HEAD (no se reutilizan los ficheros de `render-data`).
  - **Precondición:** mismos ids de nodo y de bloque, mismo número de puntos y mismos bytes de geometría.
  - **Comparación por nodo:** byte a byte en todas las columnas de las hojas (también RGB16, tiempo GPS, extra bytes,
    flags), más el color de pantalla decodificado, y en los LOD la geometría, el color de pantalla, la intensidad y la
    clase.
  - **LOD de v8:** solo faltan las columnas que D2 retira.
  - **Orden de filas distinto:** se compara el multiconjunto ordenado.

  Se ejecuta con `data/samples`, Belvedere y Majadas. Cero diferencias y la precondición cumplida es la condición de
  paso. Va en un registro nuevo fechado, con la orden exacta. Una diferencia para el plan y se lleva al propietario.
  Resultado: [registro del 21-11-2026](../records/benchmarks/V1_EQUIVALENCIA_V8_PRE1_21-11-2026.md), 0 diferencias en
  las cuatro entradas (ejemplo `v1_equivalence`).
- [x] **V4 (coste de conversión):** se enlaza la medida v7 frente a `v8.1-pre` del
  [registro del 20-11-2026](../records/benchmarks/FORMATO_V8_PRE1_20-11-2026.md) (mismo binario). Si el propietario la
  da por buena como RQ-07, no se repite. El propietario la da por buena el 21-11-2026: no se repite.
- [x] Commit `docs: V1 equivalence of v8.1-pre against v7 before retiring v7`.

### Task 1: Final V8 identity, default output and web default

**Files:** `crates/viewer_format/src/{version.rs,layout.rs}` y lo que despacha por capacidad; tests en
`crates/viewer_format/tests/v8_lod.rs`. CLI y conversor web: valor por defecto.
- [x] Test primero: un fichero `V8` es byte a byte uno `v8.1-pre` salvo la revisión de cabecera y los magics (como el
  test de v8.2-pre); una pareja V8/`v8.1-pre` mezclada se rechaza.
- [x] `FormatVersion::V8`: cabecera mayor `8`, revisión final y magics propios siguiendo el patrón de V7
  (`OCTVEND7` → `OCTVEND8`). Las capacidades son las de `v8.1-pre`, y `DEFAULT_OUTPUT = V8`.
- [x] El conversor web toma `v8` por defecto; `FORMAT_VERSIONS` lo añade (se quita en la tarea 5).
- [x] La paridad (`tools/format_parity`, `tools/convert_parity`) incluye V8 con su propia línea de digest; `check.sh` en
  verde. Los tests que fijan `DEFAULT_OUTPUT == V7`, los nombres de `WRITABLE`, `"v8".parse()` y `create_web_sample.rs`
  se ajustan.
- [x] Se fija el SHA-256 de la salida V8 (A3): test sobre `data/samples/*.las`, y test ignorado más registro para
  Belvedere.
- [x] Commit `feat(format): final V8 identity on the v8.1-pre layout, default output`.

### Task 2: Regenerate fixtures and test data as V8

**Files:** `data/samples/*.oct{,attrs}` (hoy V5), `examples/web-host/sample.oct{,attrs}` (hoy V7), las semillas de
`fuzz/src/bin/gen_seeds.rs`, y los tests que leen fixtures.
- [x] Se reconvierten los fixtures con el binario de la tarea 1 y se pasan a V8 los tests que los leen, de modo que
  ninguno dependa de un formato que se vaya a retirar. `sample-v5.*` y `sample-v7.*` quedan hasta la tarea 5, porque
  prueban la lectura antigua. Excepción: los tests `*.browser.mjs` que leen `sample-v5` se portan en la
  tarea 5 (ver el inventario).
- [x] **Fixtures de error (A4):** mientras los escritores aún existen, se guarda la cabecera real (24 bytes) y el pie
  (40 bytes) de un fichero de cada familia que se retira, como fixtures del test de la tarea 5.
- [x] **Datos ignorados (A6):** los tests ignorados leen `target/render-data` desde una variable de entorno (o un enlace
  documentado), así que se pueden ejecutar desde un worktree. Se reconvierten a V8 nuevos los ficheros que el propietario
  elija (`belvedere-v7.oct`, `majadas-v7.oct`, `data/datasets/*.oct`; `MANIFEST.txt` al día). Se **ejecutan** los tests
  ignorados, y sus valores fijados (p. ej. `lod_cells_sim_tests.rs:488-505`) se rehacen para V8 en un registro fechado,
  con el log.
- [x] **Inventario de consumidores (A5):** cada uno se porta a V8 o se borra, y la decisión queda anotada:
  - los tests `*.browser.mjs` que piden `sample-v5`/`sample-v7` (`check.sh` no los ejecuta);
  - `storage_suite.py`, `profile_conversion.py` (v5 por defecto), `compare_storage_revisions.py` y
    `run-r3-b6-matrix.sh`;
  - `lod_cells_*_tests.rs`;
  - `viewer_render_wgpu` (`cell_pool/plan.rs`, `examples/coverage_spike.rs`);
  - las órdenes de `PROBES.md` y de los README que pasan `--format-version` u opciones retiradas.

  Decisiones (tarea 2a, 21-11-2026). Se ha regenerado `sample.*` y `data/samples/fs_*.*` como V8; la semilla de fuzz
  `gen_seeds` escribe V8 (más `-v7-` heredadas); los fixtures de error están en
  [`crates/viewer_format/tests/fixtures/retired/`](../../crates/viewer_format/tests/fixtures/retired/README.md).

  | Consumidor | Decisión | Motivo |
  | --- | --- | --- |
  | `web_sample.rs` (`sample.*`) | Portado: ahora afirma V8 | Mismos recuentos y rangos; solo cambia la identidad |
  | `*.browser.mjs` con `sample-v5` (`lod-attributes`, `lod-reuse`, `lod-session`, `presentation`, `storage-format`, `surface-memory`) | Decidir en la tarea 5 | Prueban la ruta CPU sobre la rejilla de 4.096 puntos en V5; portarlos exige un fixture V8 de esa rejilla y comprobar que la ruta CPU lo admite. No se ejecutaron (`check.sh` no las cubre) |
  | `async-format-reader.browser.mjs` (`sample-v7`) | Portar a V8 en la tarea 5 | Prueba la lectura asíncrona por Blob, que V8 conserva (A1: portar, no borrar); hoy usa un fichero `v7.1-pre` |
  | Tests `*.browser.mjs` que piden `sample.oct` | Ninguno | `sample.oct` solo lo usan `main.js`, `package-local.py` y `viewer_wasm` (`lod_cells_stream_tests.rs`), que siguen igual |
  | `storage_suite.py`, `profile_conversion.py` | Portar en la tarea 5 | Defecto `v5` y lista `v5…v7.3-pre`; V8 fija 64.000 puntos por bloque y rechaza lo demás, así que cambiar el defecto cambia lo que miden y hay que decidir qué se compara |
  | `compare_storage_revisions.py` | Portar en la tarea 5 | Compara `v5` y `v7.2-pre` (ambos se retiran); decidir contra qué V8 se compara |
  | `run-r3-b6-matrix.sh` | Portado en 2b | Lee `$RENDER_DATA_DIR/*-v8.oct` (defecto `target/render-data`); no se ejecuta (necesita navegador) |
  | `lod_cells_*_tests.rs` | Portado en 2b | Leen `$RENDER_DATA_DIR/*-v8.oct` (ignorados); mismos valores que con v7, nada que rehacer ([registro](../records/benchmarks/RENDER_DATA_V8_21-11-2026.md)) |
  | `viewer_render_wgpu` (`cell_pool/plan.rs`, `examples/coverage_spike.rs`) | Sin cambio | No fijan versión ni leen fixtures versionados; `coverage_spike` recibe el `.oct` por argumento (su texto cita «v7.5») |
  | `PROBES.md` y READMEs | Portado en lo barato | Se corrigen las frases sobre `sample.oct` y los fixtures V5; quedan los comandos con `formatVersion`/`--attr-codec` (en `PROBES.md`, `web-host/README.md`, `converter-ui.mjs`, `tools/convert_parity`) para las tareas 3 a 5, cada una con su opción |
- [x] Commit `test: fixtures and seeds as V8`.

### Task 3: Retire zstd, v8.2-pre and --attr-codec (ADR 0060)

- [x] Fuera:
  - la dependencia `zstd`;
  - los códecs 5 y 6 y la etiqueta interna 2 (el lector V8 los trata como mal formados);
  - `attribute_scheme.rs` y la parte zstd de `codec.rs`, `attribute_codec.rs` y `read_plan.rs`. Las rutas LZ4, planos de
    bytes y diccionario se quedan;
  - `--attr-codec`, `attrCodec` (core, wasm, `converter-ui.mjs`) y sus casos de paridad y tests.
- [x] Test: `--attr-codec` y `attrCodec` dan un error claro de opción retirada.
- [x] Se mantiene la paridad nativo = wasm de los bloques (el digest cambia; se anota el nuevo).
- [x] Commit `refactor(format)!: drop zstd and the attribute codec option (ADR 0060)`.

### Task 4: Retire EPT input and checkpoints (ADR 0061)

- [x] Fuera:
  - `ept.rs`, `ept_checkpoint.rs`, `ept_tests.rs` y `checkpoint_record.rs`;
  - las partes de `checkpoint.rs` que solo usan los checkpoints (lo que use la ruta LAS sin checkpoints se queda; hoy
    `las_source.rs:327` llama a `verify_source`);
  - `--checkpoint` y `--resume`, `CheckpointMode`, los errores `UnsupportedEpt` e `InvalidEpt`;
  - `serde_json`, si solo lo usa EPT.
- [x] Test: una entrada EPT, `--checkpoint` y `--resume` dan un error claro.
- [x] Commit `refactor(convert)!: drop EPT input and checkpoints (ADR 0061)`.

### Task 5: Retire v5, v6, v7 and every pre-revision

**Antes de empezar (A8):** etiqueta `legacy-readers-final` y un `viewer_convert` `release` de esa etiqueta guardado
fuera de `target/`, para seguir leyendo los datos antiguos. También se añaden SHA de caracterización del planificador
en V8, equivalentes a los de `lib_tests.rs:1764-1829`.

**Orden de fuera hacia dentro (A2):** cada subtarea deja `check.sh` en verde y tiene su estimación y un tope (A11). Si
pasa de unas 3 000 líneas que no sean borrados, o de dos veces su estimación, se vuelve a partir.
- 5a: opciones del CLI, JS y wasm, `FORMAT_VERSIONS`, benchmarks y scripts.
- 5b: paridad, fuzz y `gen_seeds`.
- 5c: rutas antiguas del conversor y del core, y los ayudantes `v5_*`.
- 5d: portar los tests (A1).
- 5e: por último, quitar las variantes y los lectores.
- [ ] Fuera:
  - `FormatVersion` y `StorageLayout` quedan solo con `V8`;
  - se borran los lectores y escritores de V5, de v6 y sus previas y de v7 y sus previas, sus tests (`tests/v6.rs`,
    `v7_4*.rs`, `v7_5*.rs`, `v7_final.rs`, los de v8.1/v8.2), `create_legacy_sample.rs` y `sample-v5.*`/`sample-v7.*`.
- [ ] El conversor escribe V8 sin opción de versión. Se retiran `--format-version`, `--row-order`,
  `--points-per-block` y `--spatial-divisions` (V8 los fija), con sus equivalentes JS y la lista `FORMAT_VERSIONS`.
- [ ] Test (Review Focus 1 y 5; A4): `retired_version_name(major, rev)` en `viewer_format` nombra cada familia retirada.
  Cuando el pie no es V8, `open()` del wasm lee y valida la cabecera; solo en la ruta de error, así que una apertura
  correcta no hace otra petición Range. Con los fixtures de la tarea 2 se prueba el lector síncrono, el wasm desde node,
  `verify` y la lectura por HTTP Range: todos dan «versión N: reconvierte a V8», nunca `unknown(7, 0)` ni
  `InvalidFooter`.
- [ ] La paridad queda solo con V8, y `check_version_literals.sh` se adapta.
- [ ] Commit(s) `refactor(format)!: V8 is the only format`.

### Task 6: Demo and browser verification

- [ ] Se regeneran la muestra y el recorte de Belvedere (`scripts/make-demo-belvedere.sh`) en V8, con nombres
  versionados (`sample-v8.oct`, `belvedere-crop-v8.oct`) y el wasm con `?v=<sha>`, para que una caché antigua no mezcle
  ficheros (A10). El README de la demo y `PROBES.md` se actualizan.
- [ ] Se ejecuta la suite `*.browser.mjs` sobre la muestra V8 (A5).
- [ ] Playwright (`ui-verifier`) con la demo servida en local, en escritorio y móvil:
  - abrir la muestra;
  - los modos de color;
  - el conversor web en V8;
  - abrir un fichero v7, que debe dar el error claro;
  - el caso de un wasm antiguo en caché;
  - la consola.
- [ ] **No se publica:** se pregunta al propietario antes de `scripts/publish-demo.sh`.
- [ ] Commit `chore(web): V8 demo sample`.

### Task 7: Documentation, requirements and trackers

- [ ] `docs/reference/format/FORMAT_V8.md` pasa a ser la referencia única: se consolida `FORMAT_V8_PRE1.md` y se quitan
  las capacidades retiradas.
- [ ] Las referencias de v5, v6, v7 y las previas quedan como históricas, enlazadas desde un índice, porque explican
  los registros. No se borran.
- [ ] Se actualizan inventario, requisitos (RF-26, fila D3c), `estado.md`, el `README.md` raíz y el del
  conversor/demo.
- [ ] Estados de los ADR: 0004 y 0047 cumplidos o sustituidos por 0058 y 0061; nota en 0044 y 0058.
- [ ] Este plan y `formato-v8.md` pasan a «hecho».
- [ ] Issues: se cierran #28 y #13 con enlace, se reescribe #12 para V8, y se anota en #16 lo que queda resuelto.
- [ ] `check_docs_links.py` y `check_docs_index.py` en verde; commit `docs: V8 is the only format (D4)`.

## Criterios de parada

- **V1:** una diferencia o una precondición fallida → parar y llevarlo al propietario.
- **Bytes de V8:** el SHA fijado (A3) cambia en las tareas 3–5 sin causa explicada → parar y bisecar.
- **Tests:** baja el recuento de tests de un módulo que se queda → parar y portar.
- **Tamaño:** una subtarea de la 5 deja `check.sh` en rojo tras su commit o supera su tope → volver a partir.
- **Datos:** los tests ignorados o los de navegador no se pueden ejecutar sobre datos V8 → la tarea 2 o la 6 no está
  hecha.
- **Ficheros antiguos:** abrir uno v7 o `v8.1-pre` en la demo, `verify` o el wasm muestra otra cosa que «reconvierte a
  V8» → se para la tarea 6.
- **#111:** cambios pedidos que tocan `version.rs` o `layout.rs` → pausa y rebase.

## Después de este plan

[#112](https://github.com/Mili-ssm/OctreeViewer/issues/112): una nube densa y pesada para probar el render, en la demo si
es posible y, si no, offline.
