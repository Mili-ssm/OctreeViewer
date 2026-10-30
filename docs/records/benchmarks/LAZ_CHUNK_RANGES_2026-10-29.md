# S2: rangos de chunks LAZ y decodificador en wasm32 (29-10-2026)

> Registro fechado. Cifras válidas solo para este equipo (AMD Ryzen AI 9 365, 20 hilos; Fedora 44, Linux
> 7.2.11), `rustc 1.101.0-nightly (41089d07a 2026-10-27)`, Node v22.23.1 y `laz` 0.13.0 / `las` 0.11.2.
> Commit base: `1cde1a9`. Código: [`tools/laz_chunk_probe`](../../../tools/laz_chunk_probe/). Datos crudos:
> [JSON](data/laz-chunk-ranges-2026-10-29.json). Contexto: tarea 1 del plan de la
> [etapa 3](../../plans/conversor-web-paralelo-etapa3.md) y spec del
> [conversor web paralelo](../../plans/conversor-web-paralelo.md).

## Conclusión

| Pregunta | Respuesta |
| --- | --- |
| ¿Se puede decodificar un rango de chunks arrancando un decodificador nuevo en el offset del chunk, con los mismos puntos que la lectura secuencial, sin `seek`? | **Sí, sin parchear `laz`.** Igualdad byte a byte con la lectura secuencial para todas las particiones probadas (W = 1..8) en la fixture sintética de chunks variables, en una de chunks fijos, en autzen y en los primeros 256 chunks de Belvedere. |
| ¿Cuánto más lento es el decodificador secuencial en wasm32 (Node 22) que en nativo? | **1,74 ×** en tiempo (mediana de 5 rondas, rango 1,67–1,79 ×): 3,87 M puntos/s nativo frente a 2,21 M puntos/s en wasm32. **Bajo el umbral de 4 ×** de M3b. |

La tarea 5 **no queda bloqueada**: no hace falta parche vendorizado.

## Receta para la tarea 5

La tabla de chunks ya la valida ADR [0012](../../adr/0012-validacion-previa-de-cabeceras-las-laz.md); la receta la reutiliza tal cual. API de `laz` 0.13 usada, todo público:

1. `las::Header::laz_vlr()` (o `LazVlr::read_from`) da el `LazVlr`; `vlr.uses_variable_size_chunks()` decide si la tabla lleva `point_count`.
2. Leer 8 bytes en `offset_to_point_data` (`i64` LE = posición de la tabla), `seek` a ella y `laz::laszip::ChunkTable::read(&mut src, variable)`. Los chunks empiezan en `offset_to_point_data + 8`; el offset de un chunk es la suma de `byte_count` de los anteriores.
   Con chunks fijos `read` deja `point_count = 0`: se rellena con `chunk_size` y el resto de `header_points` en el último (`ChunkTable::with_capacity` + `push(ChunkTableEntry)`).
3. Para el rango `a..b`: montar en memoria un flujo LAZ mínimo `[i64 LE: 8 + bytes] [bytes comprimidos de los chunks a..b] [subtabla]`, donde la subtabla es una `ChunkTable` con solo las entradas `a..b` escrita con `write_to(&mut stream, &vlr)`.
   El prefijo de 8 bytes **sí es necesario**: `LasZipDecompressor::new`/`selective` lee un `i64` de offset de tabla en la posición actual y la tabla en ese offset (`sequential/decompression.rs`), también en chunks variables (sin tabla devuelve error). Por eso el `Cursor` de la receta es `[prefijo + bytes de chunk + subtabla]` y no una vista directa de los bytes del chunk.
4. `LasZipDecompressor::new(Cursor::new(stream), vlr.clone())` y `decompress_many(&mut buf)` con `buf` de `Σ point_count × vlr.items_size()` bytes. El decodificador arranca en el primer byte del primer chunk del flujo; **no se usa `seek`**.

Funciona igual con `PointWiseChunked` (formatos 0–5) y `LayeredChunked` (point14, COPC). El prototipo es `decode_chunk_range(src, &ChunkIndex, chunks, &mut Vec<u8>)` y `RangeDecoder::new` (construcción separada de la decodificación). La salida son registros LAS crudos de `items_size` bytes (para la tarea 5 hay que convertirlos a `las::PointData`/`PointSource`; no se midió esa conversión).

### Por qué no `seek`

`LasZipDecompressor::seek` (`laz-0.13.0/src/laszip/sequential/decompression.rs`) calcula `point_idx % chunk_table[chunk].point_count`, correcto solo con chunks de igual tamaño. Control negativo (`laz_seek_lands_on_the_wrong_point_with_variable_chunks`): con chunks `[7, 13, 5, 11, 50000, 3]`, `seek(9)` (tercer punto del chunk 1) usa `9 % 13 = 9` en vez de 2 y devuelve el punto 16, no el 9. `ParallelDecompressor::seek` parte del mismo cálculo de `chunk_of_point`; no se usó (ni `laz-parallel`).

## SHA de referencia para la paridad M2/M3a

Fixture [`synthetic_variable_chunks.laz`](../../../tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz) (408.284 B; LAS 1.2 formato 0, 50.039 puntos, chunks variables `[7, 13, 5, 11, 50000, 3]`, regenerable con `laz_chunk_probe make-synthetic`; un test comprueba que el fichero versionado es la salida del generador). Convertida con `target/perf/viewer_convert` (rama `feat/web-converter-stage3`, base `1cde1a9`), `--points-per-block 4096`, el resto por defecto. Dos ejecuciones dieron los mismos bytes.

| Opciones | Bloques | `.oct` SHA-256 | `.octattrs` SHA-256 |
| --- | --- | --- | --- |
| por defecto (2 divisiones) | 21 | `22ba7b4e6a63969c38adfa527798085f175c2b9cd3ec6a3a3278ffd7fe72790b` | `4a7b92dba322561b1ae79cfd783aae041bf4b658e9e995e5315125d56d0b62a9` |
| `--spatial-divisions 8` | 65 | `af0aaa75ebe724a32f3a3827d98c91be711691bfee244413f32a0732a4dde2fe` | `34009eb97e0a9d4036aaabb647e67f1cb29534ade42466edb9df09a3d0cf4d38` |

## Método

**Pruebas** (`cargo test -p laz_chunk_probe`): se compara la concatenación de los puntos decodificados por cada rango con una lectura secuencial (un `LasZipDecompressor::new` desde el inicio de los datos).

| Fixture | Chunks | Particiones |
| --- | --- | --- |
| Sintética Point10 variable `[7, 13, 5, 11, 50000, 3]`, valores no constantes (LCG, dependen del índice) y que además deben coincidir con los bytes originales | 6 | **Todas** las composiciones en 1..6 rangos (32) |
| Sintética de chunks fijos (tamaño 10, 45 puntos, último chunk de 5) | 5 | Todas las composiciones en 1..5 rangos |
| `autzen.copc.laz` y `autzen.laz` (de `las` 0.11.2) | **1 cada uno** (107 y 106 puntos) | Solo W = 1: **no ejercitan la partición**; solo prueban que la receta funciona con COPC real y con un fichero de laszip fijo |
| Belvedere COPC (`#[ignore]`, git-ignorado) | primeros **256** de 3.835 (2.377.747 de 81.682.329 puntos) | W = 1..8, con corte equilibrado en chunks y con corte pseudoaleatorio fijo |

Belvedere: chunks **variables** (46–88.207 puntos), point14 + RGB14 (36 B/punto), estructura en capas. Se limitó a 256 chunks porque la referencia secuencial y las 16 particiones de todo el fichero cuestan minutos y GB; la prueba tardó 13 s en `--release`.

**Medición** (`laz_chunk_probe bench` y `run-wasm.mjs`): mismos 256 chunks de Belvedere en una fixture (`make-fixture`, 17,4 MB: VLR + tabla + bytes de chunk), el mismo código de la biblioteca y los mismos bytes; los hash FNV-1a de la salida coinciden entre nativo y wasm32 (`ce846c90e94358ce`). Nativo con `--profile perf`; wasm32 con `--release --target wasm32-unknown-unknown`. En cada proceso: 1 iteración de calentamiento descartada y 5 medidas; se toma la mediana. 5 rondas alternando nativo y wasm32 (cada ronda = un proceso de cada). `Instant` no existe en `wasm32-unknown-unknown`, así que el runner Node cronometra cada llamada exportada con `performance.now()`. Tres cronometrajes por iteración:

- **batch**: un solo decodificador para los 256 chunks (rendimiento).
- **each**: un decodificador nuevo por chunk.
- **setup**: solo montar el flujo y construir el decodificador por chunk (sin decodificar ningún punto).

Carga: se registró `loadavg` y `/proc/pressure/cpu` antes y después de cada proceso (en el JSON). Todas las rondas con `loadavg` 1 min 4,3–5,6 sobre 20 hilos y presión `some avg10` ≤ 0,28 %: no hubo carga que obligase a repetir. Durante las rondas había otro agente activo (spike S10), contra la enmienda B7 del plan; el coordinador acepta las cifras sin repetirlas porque la presión de CPU fue despreciable y la decisión (1,74 × frente al umbral de 4 ×) no cambia con un error de medida de ese orden.

## Resultados

Mediana de 5 rondas (cada ronda ya es la mediana de 5 iteraciones) y rango mín–máx entre rondas.

| Magnitud | Nativo (`perf`) | wasm32 (Node 22) | wasm32 / nativo |
| --- | --- | --- | --- |
| Tiempo batch, 2.377.747 puntos (ms) | 615 (600–644) | 1.075 (1.050–1.117) | **1,74 ×** (1,67–1,79) |
| Puntos/s batch | 3.868.000 (3.691.000–3.966.000) | 2.211.000 (2.129.000–2.265.000) | 0,57 × |
| Puntos/s, un decodificador por chunk | 3.887.000 (3.639.000–3.956.000) | 2.209.000 (2.138.000–2.238.000) | 0,57 × |
| Montaje del decodificador por chunk, sin decodificar (µs) | 234 (231–242) | 371 (365–378) | 1,59 × |

**Coste de arranque por chunk.** Montar el flujo del rango y construir el decodificador cuesta ≈ 234 µs nativo y ≈ 371 µs en wasm32 por chunk (media de 68 KB comprimidos por chunk, que incluye copiar los bytes). Frente a decodificar un chunk medio (9.288 puntos ≈ 2,4 ms nativo, 4,2 ms wasm32), es ≈ 10 % y ≈ 9 %. La diferencia entre «un decodificador por chunk» y «uno para todo» (each − batch) queda dentro del ruido (mediana +36 µs nativo y −15 µs wasm32 por chunk, con signo variable entre rondas): no hay un coste de arranque separable más allá del montaje, porque el decodificador secuencial ya reinicia su estado en cada frontera de chunk.

## Límites

- Una sola máquina, un solo fichero de medida, un tramo de 256 chunks (los 256 primeros de COPC son nodos de poca profundidad del octree; los chunks más pequeños, de 46 puntos, tendrían mayor coste relativo de arranque). No se midió la parte del fichero con chunks de 88.000 puntos.
- La fixture está en memoria (`Cursor<&[u8]>`); no incluye leer del disco, `Blob` ni OPFS, que dependen de la tarea 5.
- Node 22 (V8, Liftoff + TurboFan), un solo hilo: no cubre Chrome/Firefox con workers, SIMD ni contención de memoria entre varios workers.
- Las pruebas de partición tienen poder real solo en la fixture sintética y en Belvedere; autzen tiene un único chunk.
- `laz` panica ante datos corruptos (ver `contain_decoder_panic` en `viewer_convert`); el prototipo no lo contiene porque parte de tablas ya validadas por ADR 0012 y no se ensayaron entradas corruptas.
- El módulo wasm32 tiene imports `__wbindgen_*` heredados de `las` → `chrono` → `js-sys`; el decodificador nunca los llama, y `run-wasm.mjs` los sustituye por funciones que lanzan. No es un módulo sin imports como `format_parity`.
- La salida son bytes crudos de `items_size` por punto, no `las::PointData`.

## Decisión

1. **Tarea 5 desbloqueada** con la receta de arriba: sin parche de `laz`, sin `Reader::seek`, sin `laz-parallel`. Cada worker construye un `RangeDecoder` por rango (o por cada trozo de rango que quiera) a partir de la tabla de chunks ya validada.
2. **Riesgo wasm32 (criterio de parada de M3b, «wasm con un worker > 4 × nativo»)**: el decodificador LAZ solo cuesta 1,74 × en nativo, bien por debajo de 4 ×. El resto del conversor (partición, codificación) no está medido aquí.
3. Si la tarea 5 usa la tabla ya leída por el preflight, basta reusarla; este prototipo la vuelve a leer solo para ser autónomo.
