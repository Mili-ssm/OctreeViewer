# Conversor web paralelo — plan de la etapa 1

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** dejar demostrado lo que la paridad necesita antes de tocar el núcleo: independencia
de workers ampliada en el CLI, igualdad de bytes nativo/wasm32 (S0), referencia del CLI (M0) y
modelo de arenas OPFS medido (S1).

**Arquitectura:** sin cambios de arquitectura todavía. Se añaden tests de caracterización, un
crate de herramienta `tools/format_parity` (digest de codificación ejecutable en nativo y en
wasm32 bajo node), un parche local de `lz4_flex`, contadores de spool en los diagnósticos del
planificador y una página de spike OPFS. La extracción de traits (M1) se planifica en la etapa 2
con los datos de S1.

**Tecnología:** Rust nightly fijado (`rust-toolchain.toml`), `lz4_flex` 0.14.0, node 22,
`wasm32-unknown-unknown` sin wasm-bindgen para la herramienta, OPFS `FileSystemSyncAccessHandle`.

**Spec:** [conversor-web-paralelo.md](conversor-web-paralelo.md).

## Restricciones globales

- `scripts/check.sh` en verde (todos los pasos) al cerrar cada tarea.
- No cambiar la fecha del nightly ni añadir `#![feature]`.
- `FormatVersion::V<n>` solo en `crates/viewer_format`: la herramienta itera `FormatVersion::WRITABLE`.
- Los SHA nativos de salida no cambian (R7): ninguna tarea altera bytes en x86_64.
- No editar registros existentes en `docs/records/`; los nuevos van fechados (`dd-mm-aaaa`), en español.
- No trasladar cifras entre revisiones, perfiles ni equipos.
- Commits convencionales en inglés, terminados en
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`; sin push.
- No enviar avisos upstream (backlog, prioridad 9 del README).

## Review Focus

1. **Raíz con todos los puntos iguales o no divisible**: la salida no debe depender de N → Tarea 1
   (`coincident_fixture`).
2. **LAZ en lugar de LAS** con varios workers y divisiones 8 → Tarea 1 (matriz con `compressed = true`).
3. **Bloques incompresibles frente a muy compresibles** (elección de codec y LZ4) → Tarea 2 (dos
   patrones de datos sintéticos).
4. **Esquemas con todos los atributos** (GPS, RGB16, NIR y extra bytes) → Tarea 2 (esquema completo).
5. **Disco insuficiente para Majadas en M0** → Tarea 4 comprueba el espacio libre y registra la omisión
   en vez de fallar a mitad.

---

### Tarea 1: independencia de workers ampliada (A5)

**Archivos:**
- Modificar: `crates/viewer_convert/src/lib_tests.rs` (junto a `bounded_spatial_workers_preserve_the_serial_output`, `:500`)

**Interfaces:**
- Consume: `fixture(path, compressed, colored, count)` (`:56`), `coincident_fixture(path)` (`:271`), `convert_file`, `ConvertOptions::new(points_per_block, max_error)`, `with_spatial_workers`, `with_spatial_divisions`, `output_attributes_path`.
- Produce: helper `fn output_pair_bytes(output: &Path) -> (Vec<u8>, Vec<u8>)` (lee `.oct` y `.octattrs`).

- [ ] **Paso 1: escribir los tests de caracterización**

```rust
#[test]
fn spatial_workers_preserve_output_across_counts_divisions_and_compression() {
    // for compressed in [false, true], divisions in [2, 4, 8]:
    //   fixture(&input, compressed, true, 4_000);
    //   options = ConvertOptions::new(64, 0.01)?.with_spatial_divisions(divisions)?;
    //   reference = output_pair_bytes(convert with with_spatial_workers(1));
    //   for workers in [2, 4, 8]: assert_eq!(output_pair_bytes(convert with workers), reference,
    //       "compressed={compressed} divisions={divisions} workers={workers}");
}

#[test]
fn coincident_root_output_is_independent_of_worker_count() {
    // coincident_fixture(&input); same comparison for workers in [1, 2, 4, 8], divisions 8.
}
```

- [ ] **Paso 2: ejecutarlos**

Run: `cargo test -p viewer_convert --profile fast spatial_workers_preserve_output_across coincident_root_output`
Esperado: PASS. Son de caracterización: si alguno falla, **parar** y reportar el caso (contradice la
premisa de R1); no cambiar el código de producción en esta tarea.

- [ ] **Paso 3: commit** — `test(convert): widen worker-count independence to divisions, LAZ and N=8`

---

### Tarea 2: digest de codificación nativo/wasm32 (S0, parte 1)

**Archivos:**
- Crear: `tools/format_parity/Cargo.toml` (`publish = false`, `crate-type = ["cdylib", "rlib"]`, dependencia `viewer_format` por ruta)
- Crear: `tools/format_parity/src/lib.rs`, `tools/format_parity/src/bin/native_digest.rs`, `tools/format_parity/run-wasm.mjs`
- Modificar: `Cargo.toml` (miembro `tools/format_parity`), `scripts/check.sh` (paso nuevo)

**Interfaces:**
- Produce: `pub fn parity_digest() -> u64` y `#[no_mangle] pub extern "C" fn format_parity_digest() -> u64` (mismo valor); `native_digest` imprime `{:016x}`; `run-wasm.mjs` carga `target/wasm32-unknown-unknown/release/format_parity.wasm`, llama a la exportación (BigInt) e imprime el mismo formato.

- [ ] **Paso 1: implementar `parity_digest`**

Para cada versión de `FormatVersion::WRITABLE`, escribir en memoria (`Cursor<Vec<u8>>`) una pareja
partida con la misma API que el CLI (`FormatWriter::new_split_for_version`, `OctreeWriter::write_block`,
`finish_pair`; límite de bloque 64 MiB como `las_source.rs:375`, error máximo 0,01). Entradas
deterministas generadas con un LCG fijo (semilla `0x5eed`): al menos 4 bloques; un patrón muy
compresible (rejilla regular, atributos constantes) y uno incompresible (ruido LCG); coordenadas
`f64` con decimales para ejercer la cuantización; esquema con GPS, RGB16, NIR y extra bytes.
Digest: FNV-1a 64 sobre `version || len || bytes` de ambos ficheros, en orden de `WRITABLE`. Si
`FormatWriter` exige ficheros y no `Write + Seek`, parar y reportar.

- [ ] **Paso 2: comprobar que nativo y wasm32 difieren hoy (RED esperado)**

Run:
```sh
cargo run -q --release -p format_parity --bin native_digest
cargo build -q --release -p format_parity --target wasm32-unknown-unknown --lib
node tools/format_parity/run-wasm.mjs
```
Esperado: dos digests **distintos** (diferencia LZ4 de 32/64 bits). Si coinciden, el patrón
compresible no ejerce LZ4: corregir las entradas hasta que difieran.

- [ ] **Paso 3: commit** — `test(format): add native/wasm32 encoding digest harness` (sin paso en `check.sh` todavía, para no dejarlo en rojo)

---

### Tarea 3: `lz4_flex` con hash de 64 bits también en wasm32 (S0, parte 2)

**Archivos:**
- Crear: `vendor/lz4_flex/` (copia de `lz4_flex-0.14.0` del registro de cargo, con su `LICENSE`)
- Modificar: `vendor/lz4_flex/src/block/hashtable.rs:26-50`, `Cargo.toml` (`[patch.crates-io] lz4_flex = { path = "vendor/lz4_flex" }`), `Cargo.lock`
- Documentar: `vendor/lz4_flex/PATCH.md` (qué cambia, por qué —R1 de la spec—, cómo volver al crate original)

**Interfaces:**
- Consume: el digest de la Tarea 2.

- [ ] **Paso 1: SHA nativo antes del parche**

Run: `cargo run -q --release -p viewer_convert -- data/samples/fs_e430_n4008_18.las target/s0/before.oct && sha256sum target/s0/before.oct target/s0/before.octattrs`
Guardar los dos SHA.

- [ ] **Paso 2: parche**

En `target_pointer_width = "32"`, seleccionar exactamente el mismo tipo de tabla hash y la misma
función hash que en `"64"`, con aritmética `u64` explícita donde la ruta de 64 bits lee `usize`. La
ruta de 64 bits no se toca. Comentario en el fichero que remita a `PATCH.md`.

- [ ] **Paso 3: paso en `scripts/check.sh`**

Junto a `clippy wasm32`: `step "format parity wasm32" bash -c 'cargo build -q --release -p format_parity --target wasm32-unknown-unknown --lib && test "$(cargo run -q --release -p format_parity --bin native_digest)" = "$(node tools/format_parity/run-wasm.mjs)"'`.

- [ ] **Paso 4: verificar**

Run: los comandos del Paso 2 de la Tarea 2 → digests **iguales**; el Paso 1 de nuevo → SHA
**idénticos** a los guardados; `cargo test -p viewer_format --profile fast`; `scripts/check.sh` →
todos los pasos en verde, incluido `format parity wasm32`.
Criterio de parada: sin igualdad tras 3 días, parar y consultar al propietario (choca con R7).

- [ ] **Paso 5: commit** — `fix(format): encode LZ4 identically on wasm32 and 64-bit targets`

---

### Tarea 4: referencia del CLI (M0)

**Archivos:**
- Crear: `docs/records/benchmarks/WEB_CONVERTER_BASELINE_2026-10-29.md` y `docs/records/benchmarks/data/web-converter-baseline-2026-10-29.json`
- Modificar: `docs/README.md` (fila en Registros)

**Interfaces:**
- Produce: SHA de referencia para R1 y la mediana Belvedere para R7, citados por las etapas siguientes.

- [ ] **Paso 1: comprobar el espacio libre**

`df -h target data`. Majadas necesita ≥ 40 GB libres (salida ~7,5 GB y spools observados ~26 GB). Si
no los hay, omitir Majadas y registrarlo como pendiente.

- [ ] **Paso 2: SHA con el CLI** (`cargo build --profile perf -p viewer_convert`, binario `target/perf/viewer_convert`, opciones por defecto salvo indicación)

| Caso | Entrada |
| --- | --- |
| LAS pequeño | `data/samples/fs_e430_n4008_18.las` |
| COPC pequeño | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/las-0.11.2/tests/data/autzen.copc.laz` |
| Belvedere | `data/datasets/belv_2009_digital-aerial_pcd_copc.laz` (COPC: chunks variables) |
| Belvedere divisiones 8 | ídem con `--spatial-divisions 8` |
| Majadas | `data/datasets/sensoforest/SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz` |

`sha256sum` de `.oct` y `.octattrs`; repetir Belvedere con `--spatial-workers 1` y `8` y comprobar
que el SHA coincide (confirma la Tarea 1 a escala real).

- [ ] **Paso 3: tiempos**

Belvedere por defecto con `cargo run --profile perf -p viewer_convert --example real_dataset_bench -- convert …`,
5 rondas seguidas sin otras cargas: total, `spatial_planning` y fases `internal_*` impresas (jerarquía
incluida), RSS pico (`/usr/bin/time -v`). Majadas: 1 ronda con las mismas métricas. Comprobar que
`real_dataset_bench` produce el mismo SHA que el CLI; si no, registrar la diferencia de opciones.

- [ ] **Paso 4: registro**

Equipo, kernel, `rustc -V`, commit, estado de caché; tabla de SHA; medianas con mín–máx; JSON con
los valores crudos. Enlace desde `docs/README.md`. `python3 scripts/check_docs_links.py` → ok.

- [ ] **Paso 5: commit** — `docs(bench): record CLI baseline for web converter parity`

---

### Tarea 5: modelo de arenas y medida OPFS (S1)

**Archivos:**
- Modificar: `crates/viewer_convert/src/spatial.rs` (diagnósticos de spool), `crates/viewer_convert/examples/real_dataset_bench.rs` (impresión)
- Crear: `docs/records/benchmarks/data/opfs-arena-spike-2026-10-29.html` y `.js` (página de spike, desechable, guardada como evidencia)
- Crear: `docs/records/benchmarks/OPFS_ARENA_SPIKE_2026-10-29.md`

**Interfaces:**
- Produce en `SpatialPhaseDiagnostics`: `spool_bytes_written: u64`, `peak_live_spool_bytes: u64`, `root_child_spool_bytes_written: Vec<u64>` (bytes escritos en el subárbol de cada hijo raíz, en orden de hijo). Solo diagnóstico: no cambian bytes de salida (verificar SHA de la Tarea 4).

- [ ] **Paso 1: test de los contadores**

```rust
#[test]
fn spool_diagnostics_account_written_and_live_bytes() {
    // fixture 4_000 points, divisions 4, workers 1: diagnostics.spool_bytes_written > 0;
    // peak_live_spool_bytes <= spool_bytes_written;
    // root_child_spool_bytes_written.iter().sum::<u64>() <= spool_bytes_written.
}
```

- [ ] **Paso 2: implementar los contadores** donde se añade y se borra cada spool (`spatial.rs:571-578`, `:1013`, `:1061`); comprobar SHA de LAS pequeño y Belvedere iguales a la Tarea 4.

- [ ] **Paso 3: modelo**

Con Belvedere (por defecto y divisiones 8) y Majadas: pico de arenas =
bytes de spools de nivel 1 + suma de los N mayores `root_child_spool_bytes_written` en curso, para
N = 4 y 8, frente a `peak_live_spool_bytes` nativo. Criterio de parada: pico modelado > 1,5 × el
conjunto vivo nativo → replantear `SpoolStore` con el propietario antes de la etapa 2.

- [ ] **Paso 4: medida en navegador**

Página de spike con N workers (1, 4, 8); cada uno crea una arena OPFS, añade 1 GiB en bloques de
32 KiB y de 1 MiB con un `FileSystemSyncAccessHandle`, la relee y la borra. Medir MB/s de escritura y
lectura, tiempo de `createSyncAccessHandle` y `removeEntry`. Chrome y Firefox, servida con
`python3 -m http.server` y desde `file://`; registrar si OPFS está disponible en `file://`. Usar el
mecanismo de navegador de los benchmarks web existentes (`examples/web-host/README.md`, sección de
probes). Criterio de parada: OPFS no disponible en `file://` en Firefox → volver a R8 con el
propietario.

- [ ] **Paso 5: registro** — modelo, medidas y conclusión (tamaño de añadido recomendado, handles por
trabajo, si las arenas caben en cuota), enlazado desde `docs/README.md`; `scripts/check.sh` en verde.

- [ ] **Paso 6: commit** — `feat(convert): report spool write and live-byte diagnostics` + `docs(bench): record OPFS arena spike`

---

## Después de la etapa 1

La etapa 2 (M1: traits con arenas dentro de `viewer_convert`, R7 con ≥ 5 rondas A/B alternadas) se
planifica con los resultados de S0 y S1; los cambios de forma del `SpoolStore` que salgan de S1 se
incorporan antes a la spec.

S0 con bloques reales de Belvedere (la spec exige identidad de bytes «en fixtures y bloques Belvedere»; el arnés actual usa solo bloques sintéticos) queda **pendiente** para la etapa 2, antes de M3a.
