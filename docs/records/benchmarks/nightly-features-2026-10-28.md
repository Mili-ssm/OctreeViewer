# Evaluación de características nightly (2026-10-28)

Resultado: **no se adopta ninguna**. Las dos candidatas medidas empeoran el rendimiento; el código de producción queda sin cambios.

## Entorno
- CPU: AMD Ryzen AI 9 365 w/ Radeon 880M (20 hilos); proceso fijado con `taskset -c 4`.
- `rustc 1.101.0-nightly (41089d07a 2026-10-27)`, `nightly-2026-10-28`, perfil `perf` (opt-level 3, LTO thin, 16 CGU), host nativo sin `target-cpu=native`.
- Criterio: adoptar solo si la mediana mejora más que el rango mín–máx de la línea base y la salida es idéntica byte a byte.

## Candidatas

| # | Característica | Ruta caliente | Harness | Base (mediana, mín–máx) | Con cambio | Veredicto |
|---|---|---|---|---|---|---|
| 1 | `portable_simd` (`Simd<u32,8>::gather_or_default` + `reduce_xor`) | `crates/viewer_format/src/crc32.rs` `Crc32::update` | `examples/checksum_bench.rs 64 9` | 37,12 ms / 64 MiB (37,09–37,16; ~1,68 GiB/s) | 122,68 ms (122,63–122,74) | Rechazada: 3,3x más lenta |
| 2 | `portable_simd` (`gather_or_default` / `scatter` de 16 filas) | transposición de planos de bytes, `attribute_codec.rs` `encode_byte_planes` y `decode` | `examples/byte_plane_bench.rs 9` (copia aislada del bucle de producción, estrategia `column`) | ver abajo | ver abajo | Rechazada: más lenta en todos los anchos |

Detalle de la candidata 2 (1 048 576 filas, ms, mediana de 6 procesos x 9 repeticiones; `column` = bucle actual):

| Dirección | Ancho | column | simd16 |
|---|---|---|---|
| encode | 3 | 1,369 (1,365–1,376) | 3,730 |
| encode | 6 | 2,788 (2,780–2,820) | 7,507 |
| encode | 8 | 3,817 (3,805–3,897) | 10,058 |
| encode | 16 | 9,691 (9,455–10,101) | 20,837 |
| decode | 3 | 1,365 (1,361–1,369) | 1,878 |
| decode | 6 | 2,825 (2,791–3,118) | 3,799 |
| decode | 8 | 3,876 (3,817–4,118) | 5,136 |
| decode | 16 | 10,404 (10,340–10,476) | 10,992 |

Datos completos (también 262 144 filas y las estrategias `row`/`tile256`): [data/nightly-features-2026-10-28-byte-planes-simd16.txt](data/nightly-features-2026-10-28-byte-planes-simd16.txt).

## Notas y causas
- CRC32 con tablas (slicing-8) está limitado por la cadena de dependencia `self.0` entre bloques y por las cargas de tabla; el gather SIMD no la acorta y añade coste de gather más reducción. Un CRC realmente vectorizado requiere multiplicación sin acarreo (PCLMULQDQ), que no es parte de `portable_simd` ni es portable a wasm32.
- Los gather/scatter de `u8` no tienen instrucción nativa; se emulan por lanes y pierden frente al bucle escalar que LLVM ya optimiza.
- Línea base de CRC: los primeros procesos tras compilar dieron 51,9 ms (CPU fría; 2 de 15) y se trataron como calentamiento; los otros 13 dieron 37,09–37,16 ms.
- Las pruebas de la candidata 1 compilaron con `#![feature(portable_simd)]`; ninguna se llegó a ejecutar con la suite porque se descartó por rendimiento. Todo revertido (`git checkout`).

## No evaluadas (sin ruta caliente con ganancia plausible)
- `slice_as_chunks`/`as_chunks`: ya estable y usada en `crc32.rs`.
- Ordenación radix de `occupied_cell_index.rs` y `occupancy_codec.rs`: sin bucles de escaneo de bits ni arrays contiguos que SIMD acelere; el cuello es el scatter por cubo (acceso aleatorio). Sin harness dedicado; no se midió.
- Visibilidad: ordenación/top-k, no aritmética de datos paralelos.

## Comandos
```
taskset -c 4 target/perf/examples/checksum_bench 64 9
taskset -c 4 target/perf/examples/byte_plane_bench 9
cargo build --offline --profile perf -p viewer_format --example <bench>
```
