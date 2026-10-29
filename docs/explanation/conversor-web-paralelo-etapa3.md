# Conversor web paralelo — plan de la etapa 3

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Estado (29-10-2026):** plan redactado y enmendado tras el pre-mortem (abajo); Tareas 1–3 en curso.

**Objetivo:** llegar a M3a: un núcleo `viewer_convert_core` sin E/S que compila para wasm32 (M2),
la partición raíz por rangos de chunks con los mismos bytes en nativo, y una conversión web con un
worker sobre ese núcleo con paridad byte a byte en Chrome y Firefox. Los spikes S2 y S10 cierran
las dos incógnitas que M2 y M3b necesitan.

**Arquitectura:** la [spec](conversor-web-paralelo.md) es la autoridad; la etapa anterior es la
[etapa 2](conversor-web-paralelo-etapa2.md), cuyas firmas (`SpoolStore`, `PointSource`, `Clock`,
`run_node_job`, `run_root_partition`) se conservan. El núcleo recibe la lógica pura de
`viewer_convert` (planificador por nodos, codificación de spools, acumuladores LOD, presupuestos);
el shell nativo conserva ficheros, hilos, checkpoints, EPT y publicación. La partición por rangos
escribe un segmento por (rango, hijo) y el host los une en orden de rango.

**Tecnología:** Rust nightly fijado, `las` 0.11 sin `laz-parallel` en el núcleo, `laz` 0.13,
wasm-bindgen 0.2.129, Node 22 (`node:test` y módulos wasm sin imports, como `tools/format_parity`),
Playwright 1.64 vía scripts Node (como los drivers de S8/S9).

**Spec:** [conversor-web-paralelo.md](conversor-web-paralelo.md).

## Restricciones globales

- `scripts/check.sh` en verde (todos los pasos) al cerrar cada tarea.
- No cambiar la fecha del nightly ni añadir `#![feature]`.
- `FormatVersion::V<n>` solo en `crates/viewer_format`.
- R1: los SHA de salida del CLI no cambian; referencia [M0](../records/benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md) (LAS pequeño `413858a9…`, Belvedere `6ca0f86d…`, Belvedere divisiones 8 `fb90a490…`, Majadas `74226286…`).
- R7: mediana pareada de `real_dataset_bench convert` en Belvedere (4 workers, perfil `perf`), 7–10 pares A/B alternados en máquina en reposo, registrando `/proc/pressure`, governor y perfil de energía; dentro de ±3% de la rama base medida en la misma sesión.
- El núcleo no usa `std::fs`, `std::thread`, `rayon`, `Instant::now`, `SystemTime::now`, `std::process` ni `laz-parallel`; un lint `disallowed-methods` (denegado) y `cargo check --target wasm32-unknown-unknown` en `check.sh` lo garantizan. Sin `dyn` ni asignación por punto en el bucle caliente (criterio de parada de M2).
- Registros nuevos fechados con el día de ejecución (`dd-mm-aaaa` en el texto, `<aaaa-mm-dd>` ISO en el nombre), en español; no editar registros ya mergeados; no trasladar cifras entre revisiones, perfiles ni equipos.
- Commits convencionales en inglés con la línea `Co-Authored-By` del modelo que los escribe; sin push.
- No enviar avisos upstream (backlog, prioridad 9 del README).

## Review Focus

1. **Fronteras de rango LAZ en chunks atípicos**: chunks diminutos, último chunk parcial y tamaños variables `[7, 13, 5, 11, 50000, 3]` → mismos puntos que la lectura secuencial (Tarea 5, fixture multi-chunk variable).
2. **Empates del punto más cercano entre rangos**: un punto duplicado a ambos lados de una frontera de rango debe elegir el del rango anterior, como la serie (Tarea 5).
3. **Hijos vacíos en algunos rangos**: un hijo con puntos solo en el último rango, o en ninguno, no crea spool ni segmento vacío y conserva el orden de ids (Tarea 5).
4. **Errores en un rango posterior**: LAS truncado, punto fuera de la cabecera o pánico del decodificador en el rango k > 0 → mismo `Display` e índice global que la serie; si fallan dos rangos, gana el menor `PointRange::start` (Tarea 5).
5. **Determinismo de todo el pipeline en wasm32**, no solo del formato: la pareja completa convertida en wasm32 bajo Node tiene el SHA del CLI (Tarea 7).

## Enmiendas del pre-mortem (29-10-2026)

Ya aplicadas en el texto de las tareas; se listan para trazabilidad.

| # | Enmienda | Tareas |
| --- | --- | --- |
| B1 | El LAS pequeño (111 190 puntos < 262 144 por bloque) deja la raíz como hoja: la paridad de M3a también usa 4096 puntos por bloque × divisiones {2, 8}, autzen y la fixture multi-chunk variable, con más de un nodo comprobado; SHA de referencia del CLI serie registrados antes de la Tarea 5 | 1, 7 |
| B2 | El bucle host serie (`inspect_source`, metadatos, plan y fase de bloques serie sobre `Write + Seek`) va al núcleo en la Tarea 4, antes de que las Tareas 5 y 7 se separen; la fase de bloques nativa y la serie comparten la función por bloque | 3, 4, 7 |
| B3 | El `PointSource` por rangos de chunks vive en el núcleo (genérico en `Read + Seek`); las llamadas a `fill` se cortan en múltiplos globales de `input_batch_point_limit`; con error en el rango k se cancelan solo los rangos > k y gana el de menor inicio; los errores de `laz` dan el mismo `Display` que la serie | 3, 5 |
| B4 | Las firmas de `PointSource` y de rangos son provisionales hasta el paso 1 de S2 | 3 |
| B5 | Tareas en paralelo con código: worktree y `CARGO_TARGET_DIR` propios; `git add` solo de rutas propias; el miembro nuevo del workspace (Tarea 1) se integra antes de la Tarea 4 | todas |
| B6 | `#[inline]` en las funciones calientes que pasan a ser entre crates y puerta de instrucciones (callgrind) + R7 corto antes de la Tarea 5 | 4 |
| B7 | Mediciones de tiempo (S2 paso 2, R7) sin otros agentes activos, anotado en el registro | 1, 6 |
| B8 | S10 reserva el peor caso en la admisión, modela 513 handles por trabajo en divisiones 8 y la raíz por rangos (W × d³), y añade P3: solo el coordinador abre handles y vuelca | 2 |
| B9 | Cancelación en M3a: API por pasos o `terminate()` más limpieza de huérfanos, fijada en la Tarea 3 y probada en la Tarea 7 | 3, 7 |
| B10 | El lint añade `disallowed-types` (`File`, `OpenOptions`, `Instant`, `SystemTime`, tipos de hilo); el núcleo no depende de rayon, ctrlc ni `laz-parallel` | 4 |
| B11 | La estimación de memoria suma W × d³ búferes de hijo solo con rangos activos; los tests pasan también W = 1 por la unión de segmentos | 5 |

**Criterios de parada añadidos:** instrucciones del LAS pequeño en divisiones 8 > +2% frente a
`fb9a3b7` tras B6 → parar antes de la Tarea 5 y rediseñar la frontera del bucle caliente (o revertir);
S2 necesita parchear `laz` → decide el propietario antes de fijar firmas; SHA por rangos (W > 1)
distinto tras dos días → rangos desactivados y M2 se cierra sin ellos; S10 con > 1 s de handles por
trabajo volcado en Firefox en todas las políticas, o > 664 handles simultáneos → replantear los
spools con el propietario antes de M3b.

---

### Tarea 1: spike S2 — rangos de chunks LAZ y decodificador en wasm32

**Pregunta:** ¿se puede decodificar un rango de chunks LAZ arrancando un decodificador nuevo en el
offset de bytes del chunk (tabla ya validada, ADR 0012) con los mismos puntos que la lectura
secuencial, sin `Reader::seek` (falla con chunks variables)? ¿Cuánto más lento es el decodificador
secuencial en wasm32 que en nativo?

**Archivos:**
- Crear: `tools/laz_chunk_probe/` (miembro del workspace: bin nativo y `cdylib` wasm32 sin imports con runner Node, como `tools/format_parity`)
- Crear: `docs/records/benchmarks/LAZ_CHUNK_RANGES_2026-10-29.md` y sus datos en `docs/records/benchmarks/data/`

**Interfaces:**
- Produce: un prototipo `decode_chunk_range(bytes_or_reader, header, chunk_table, chunks: Range<usize>, out: &mut las::PointData) -> Result<u64, _>` y la receta exacta de la API de `laz` 0.13 que usa, para que la Tarea 5 lo convierta en `PointSource`.

- [ ] **Paso 1:** prototipo nativo; test que compara, para cada partición de los chunks en W = 1..8 rangos, la concatenación de puntos decodificados por rango con la lectura secuencial completa: autzen COPC, Belvedere COPC (ignorado si falta el fichero) y una fixture sintética con chunks `[7, 13, 5, 11, 50000, 3]`.
- [ ] **Paso 1b (B1):** SHA de referencia del CLI serie (base de la etapa) para la fixture multi-chunk variable con 4096 puntos por bloque, divisiones 2 y 8; fixture versionada si ocupa < 2 MB o generador con semilla fija.
- [ ] **Paso 2 (B7, sin otros agentes activos):** coste de arranque por chunk y puntos/s nativo frente a wasm32 bajo Node, sobre los primeros ≥ 64 chunks de Belvedere, 5 rondas alternadas; mediana y rango.
- [ ] **Paso 3:** registro con método, cifras, límites y decisión: receta para la Tarea 5. Si `laz` 0.13 no lo permite sin parche, el registro compara parche vendorizado (como `vendor/lz4_flex`) frente a otra ruta y **para** antes de la Tarea 5. Si wasm32 > 4 × nativo, se anota como riesgo del criterio de parada de M3b.
- [ ] **Paso 4:** commit.

### Tarea 2: spike S10 — pico de RAM del híbrido y apertura de handles

**Pregunta:** con spools como segmentos `ArrayBuffer` en RAM volcados a un fichero OPFS por spool,
¿qué pico de RAM resulta con el presupuesto de la spec, y cuándo se abren los handles de volcado?
El trabajo de partición raíz escribe todo el dataset, así que «volcar solo entre trabajos» no basta.
Políticas a comparar:

| Política | Regla |
| --- | --- |
| P0 | Solo RAM (cota de RAM, Chrome `file://`) |
| P1 | Admisión por entrada: un trabajo cuyo peor caso (`input_spool_bytes` + LOD) no cabe en el presupuesto libre recibe del host, antes de empezar, handles abiertos para sus `divisions³ + 1` salidas y escribe en ellos; si cabe, solo RAM. Entre trabajos, el host vuelca primero los spools retenidos que se consumen más tarde (LOD, hojas). Se modela el coste de 513 handles por trabajo en divisiones 8, la raíz por rangos (W × d³) y los handles que acaban vacíos |
| P2 | Handles abiertos para todo trabajo (coste de referencia de S9) |
| P3 | Solo el coordinador toca OPFS: el worker llena segmentos de tamaño fijo y los transfiere; el coordinador lleva el libro de RAM y vuelca spools enteros de forma asíncrona entre mensajes; contrapresión tras K segmentos sin confirmar |

**Archivos:**
- Crear: `crates/viewer_convert/benchmarks/model_hybrid_spools.py` y `test_model_hybrid_spools.py` (reutiliza los eventos y el simulador de `model_node_job_arena.py`)
- Crear: `docs/records/benchmarks/data/hybrid-spool-ram-2026-10-29.{html,js,-driver.mjs,jsonl,json}`
- Crear: `docs/records/benchmarks/HYBRID_SPOOL_RAM_2026-10-29.md`

- [ ] **Paso 1:** test del modelo con un árbol manual de 5 nodos: pico, bytes volcados, ficheros y handles por política coinciden con valores calculados a mano.
- [ ] **Paso 2:** modelo sobre los eventos de Belvedere (2 y 8 divisiones) y Majadas: pico de RAM de spools, bytes y ficheros volcados, handles abiertos y su coste con las latencias medidas de S9, para presupuestos de spools de 0,5/1/2/3 GiB y N = 1, 4, 8.
- [ ] **Paso 3:** navegador (Chrome y Firefox, HTTP, Playwright): PSS frente a bytes retenidos para segmentos de 32 KiB y 1 MiB hasta 2 GiB en un worker, y coste de transferirlos al host; muestreo con `examples/web-host/sample-process-memory.py`.
- [ ] **Paso 4:** registro con decisión: política, tamaño de segmento, presupuesto inicial y fórmula del pico para M3b/M6. Commit.

### Tarea 3: M2 paso 1 — firmas del núcleo y de la partición por rangos (arquitecto)

Un arquitecto (Opus) fija, a partir del código de M1, lo que las Tareas 4, 5 y 7 implementan, y lo
anota en una sección «Firmas fijadas» de este documento antes de seguir:

- Lista de módulos y tipos que pasan al núcleo y los que quedan en el shell; visibilidad (`pub` del núcleo, reexportes en `viewer_convert` para no tocar llamadores).
- `ConvertError` del núcleo frente al del shell: variantes con `PathBuf`/`Checkpoint`/`io::Error`, sin cambiar ningún `Display` ni los `match` de los tests.
- Si `PointSource::header() -> &las::Header` se mantiene (dependencia `las` sin `laz-parallel` en el núcleo).
- `SpatialBuilder` genérico en `Clock`; `ConvertOptions::default` sin `available_parallelism` en el núcleo.
- Partición por rangos: `RangePartition` (segmentos por hijo, `SampleGrid` y límites fusionables), `SampleGrid::merge` (sumas; más cercano con `<` estricto y el rango anterior primero), unión de segmentos (`join_segments` genérica sobre `SpoolStore`, borrando cada segmento tras copiarlo), orden de ids, errores (A8) y casos raíz con puntos iguales y no divisible.
- Cómo se activa en nativo (`ConvertOptions` y `real_dataset_bench`, sin cambiar el defecto) para que R7 no se vea afectado; memoria de W × d³ búferes solo con rangos (B11).
- `PointSource` por rangos de chunks en el núcleo, genérico en `Read + Seek`, con cortes de `fill` en múltiplos globales y la regla de errores de B3; provisional hasta S2 (B4).
- Bucle host serie del núcleo (B2): `inspect_source`, metadatos, plan y fase de bloques serie sobre `Write + Seek`, con la función por bloque compartida con la fase nativa paralela.
- Cancelación en M3a (B9): API por pasos o `terminate()` más limpieza de huérfanos.
- Lista exacta de `disallowed-methods` y `disallowed-types` (B10) y funciones que llevan `#[inline]` (B6).

- [ ] **Paso 1:** firmas anotadas aquí; revisión del coordinador antes de la Tarea 4.

### Tarea 4: M2 paso 2 — crate `viewer_convert_core`

**Archivos:** los fija la Tarea 3. Crear `crates/viewer_convert_core/` (`Cargo.toml` con
`[lints.clippy] disallowed_methods = "deny"`, `clippy.toml`); modificar `crates/viewer_convert/`,
`Cargo.toml` del workspace, `scripts/check.sh`, `.cargo/config.toml` (`check-web`).

- [ ] **Paso 1:** mover los módulos puros sin cambiar su lógica; `viewer_convert` reexporta; `#[inline]` en las funciones calientes que pasan a ser entre crates (B6).
- [ ] **Paso 1b (B2):** bucle host serie en el núcleo; la fase de bloques nativa usa la misma función por bloque. Test: la ruta serie del núcleo produce los mismos bytes que el CLI en las fixtures de `lib_tests.rs`.
- [ ] **Paso 2:** `check.sh` añade `cargo check -p viewer_convert_core --target wasm32-unknown-unknown` y clippy con el lint (B10); un uso de prueba de `std::fs::read` en el núcleo debe hacerlo fallar (comprobarlo en el worktree propio y revertirlo).
- [ ] **Paso 3:** tests existentes en verde; SHA de M0 en LAS pequeño, autzen y Belvedere (defecto y divisiones 8) iguales.
- [ ] **Paso 3b (B6, sin otros agentes activos):** instrucciones (callgrind) del LAS pequeño, divisiones 8, 2048 puntos por bloque, ≤ +2% frente a `fb9a3b7`, y R7 de 3 rondas en divisiones 8. Criterio de parada arriba.
- [ ] **Paso 4:** commit.

### Tarea 5: M2 paso 3 — partición raíz por rangos de chunks en nativo

**Archivos:** los fija la Tarea 3; en el núcleo, la partición, la unión de segmentos y el
`PointSource` por rangos de chunks LAZ (receta de la Tarea 1, B3); en el shell, la ejecución de W
rangos con `thread::scope`.

- [ ] **Paso 1:** tests que fallan: matriz W ∈ {1, …, 8} × LAS/LAZ multi-chunk variable × divisiones {2, 4, 8} igual byte a byte a la serie, con W = 1 también por la unión de segmentos (B11); Review Focus 1–4 (fronteras, empates, hijos vacíos, errores en el rango k > 0 con el mismo `Display` e índice global, B3); raíz con todos los puntos iguales y no divisible con W > 1.
- [ ] **Paso 2:** implementar hasta verde; `MemSpoolStore` no deja ids vivos tras la fase de bloques.
- [ ] **Paso 3:** SHA de Belvedere (defecto y divisiones 8) con W = 1, 2, 4, 8 rangos iguales a M0. Criterio de parada (spec): el núcleo necesita `dyn` o asignación por punto.
- [ ] **Paso 4:** commit.

### Tarea 6: M2 paso 4 — puerta R7, registro y estado

- [ ] **Paso 1:** R7 frente a `fb9a3b7` (base de la etapa 3) en la misma sesión; registro `docs/records/benchmarks/M2_R7_<aaaa-mm-dd>.md` con SHA por rangos de la Tarea 5. Criterio de parada: regresión > 3% tras dos intentos → revertir la extracción.
- [ ] **Paso 2:** actualizar estado en README (fila 7), este plan, la spec (líneas citadas que M1 dejó obsoletas), el inventario (crate nuevo) y `CONTRIBUTING.md` (paso nuevo de `check.sh`). Commit.

### Tarea 7: M3a — conversión web con un worker sobre el núcleo

**Archivos:**
- Modificar: `crates/viewer_wasm/` (un conversor wasm-bindgen nuevo sobre la API serie del núcleo: `PointSource` sobre `Blob` con `BrowserBlobReader`, `Clock` con `performance.now()`, `MemSpoolStore`, salida a handles OPFS como `OpfsWriter`), `examples/web-host/converter-worker.js` y `converter-api.mjs` (opción `engine: "core"`; el conversor actual sigue por defecto hasta M3b, que lo elimina)
- Crear: export sin imports en `tools/format_parity` (o un tool hermano) que convierte un LAS en memoria con el núcleo y devuelve los SHA de la pareja; paso en `check.sh`
- Crear: driver Playwright `docs/records/benchmarks/data/m3a-parity-<aaaa-mm-dd>-driver.mjs` y registro `docs/records/benchmarks/M3A_PARITY_<aaaa-mm-dd>.md`

Se ejecuta en su propio worktree (B5) y solo toca `viewer_wasm`, el host web y `tools/`: el bucle
serie ya está en el núcleo (Tarea 4, B2).

- [ ] **Paso 1 (B1):** test Node que falla: SHA de la pareja convertida en wasm32 con el núcleo = SHA del CLI para el LAS pequeño de M0 (`413858a9…`), el LAS pequeño con 4096 puntos por bloque y divisiones 2 y 8, autzen y la fixture multi-chunk variable (SHA de la Tarea 1); cada caso salvo el primero comprueba más de un nodo.
- [ ] **Paso 2:** implementar hasta verde; tests Node de `converter-api`/`converter-worker` para `engine: "core"` (progreso, error y la cancelación fijada en la Tarea 3, B9).
- [ ] **Paso 3:** navegador: Chrome y Firefox por HTTP convierten el LAS pequeño con `engine: "core"`; SHA = CLI; registro. Criterio de parada (spec): SHA distinto sin explicación tras una semana.
- [ ] **Paso 4:** estado en README y este plan; commit.

---

## Después de la etapa 3

M3b (un worker con spools híbridos a escala Belvedere según S10, eliminar `browser_convert.rs`,
Chrome `file://` solo RAM), M4 (pool de preparación y escritor ordenado, S3/S4/S7), M5 (pool de
partición y trabajos por nodo, validación paralela) y M6 (Majadas), en el orden de la spec.
