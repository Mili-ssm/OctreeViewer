# Evaluación de características nightly (2026-10-28)

Resultado: **no se adopta ninguna**. Las dos formulaciones medidas empeoran el rendimiento; el código de producción queda sin cambios.

Alcance: el rechazo de la candidata 1 se aplica solo a la formulación gather + `reduce_xor` sobre las tablas slicing-8; el de la candidata 2, solo a la transposición con gather/scatter de `u8` en bloques de 16 filas. **No** se probó el plegado con multiplicación sin acarreo (PCLMULQDQ) mediante `core::arch` estable, que además no requiere ninguna característica nightly. La candidata 2 se midió únicamente en la copia del harness (`byte_plane_bench`), no se integró en `attribute_codec.rs`.

## Entorno
- CPU: AMD Ryzen AI 9 365 w/ Radeon 880M (20 hilos); proceso fijado con `taskset -c 4`.
- `rustc 1.101.0-nightly (41089d07a 2026-10-27)`, `nightly-2026-10-28`, perfil `perf` (opt-level 3, LTO thin, 16 CGU), host nativo sin `target-cpu=native`.
- Criterio: adoptar solo si la mediana mejora más que el rango mín–máx de la línea base y la salida es idéntica byte a byte.

## Candidatas

| # | Característica | Ruta caliente | Harness | Base (mediana, mín–máx) | Con cambio | Veredicto |
|---|---|---|---|---|---|---|
| 1 | `portable_simd` (`Simd<u32,8>::gather_or_default` + `reduce_xor`) | `crates/viewer_format/src/crc32.rs` `Crc32::update` | `examples/checksum_bench.rs 64 9` | 37,12 ms / 64 MiB (37,09–37,16; ~1,68 GiB/s) | 122,68 ms (122,63–122,74) | Rechazada (formulación gather + `reduce_xor`): 3,3x más lenta |
| 2 | `portable_simd` (`gather_or_default` / `scatter` de 16 filas) | transposición de planos de bytes, `attribute_codec.rs` `encode_byte_planes` y `decode` | `examples/byte_plane_bench.rs 9` (copia aislada del bucle de producción, estrategia `column`) | ver abajo | ver abajo | Rechazada (gather/scatter `u8` de 16 filas, solo en el harness): más lenta en todos los anchos |

Detalle de la candidata 2 (1 048 576 filas, ms; `column` = bucle actual). La cifra es la mediana de 6 procesos; cada proceso aporta la mediana de sus 9 repeticiones, y el rango mín–máx entre paréntesis se extiende sobre esas 6 medianas por proceso (no sobre repeticiones individuales). No se descartó calentamiento: las 9 repeticiones cuentan, y el orden de las estrategias rota entre repeticiones. La columna simd16 solo muestra la mediana; su rango está en el fichero de datos.

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
- Línea base de CRC: los primeros procesos tras compilar dieron 51,9 ms (2 de 15; causa no determinada) y se descartaron; los otros 13 dieron 37,09–37,16 ms. Valores por proceso en [data](data/nightly-features-2026-10-28-crc32-runs.txt) (solo medianas por proceso; las repeticiones individuales no se conservaron).
- No se tomó ningún perfil de muestreo (`perf record`): ambas formulaciones se rechazaron por tiempo de pared con gran margen (3,3x y 1,1–2,2x más lentas), por lo que un perfil no habría cambiado la decisión.
- Las pruebas de la candidata 1 compilaron con `#![feature(portable_simd)]`; ninguna se llegó a ejecutar con la suite porque se descartó por rendimiento. Todo revertido (`git checkout`).

## No evaluadas (sin ruta caliente con ganancia plausible)
- `slice_as_chunks`/`as_chunks`: ya estable y usada en `crc32.rs`.
- Ordenación radix de `occupied_cell_index.rs` y `occupancy_codec.rs`: sin bucles de escaneo de bits ni arrays contiguos que SIMD acelere; el cuello es el scatter por cubo (acceso aleatorio). Sin harness dedicado; no se midió.
- Visibilidad: ordenación/top-k, no aritmética de datos paralelos.

## Comandos
Toolchain: `rust-toolchain.toml` fija `nightly-2026-10-28`. Las formulaciones usan `#![feature(portable_simd)]` en la raíz del crate (`crates/viewer_format/src/lib.rs` para la candidata 1; primera línea de `crates/viewer_format/examples/byte_plane_bench.rs` para la 2). El perfil `perf` está definido en `Cargo.toml` (`[profile.perf]`).

Formulaciones probadas, aplicables con `git apply` sobre `d2c47f5^` (el código de producción es idéntico en `d2c47f5`):
- [crc32-simd.patch](data/nightly-features-2026-10-28-crc32-simd.patch)
- [byte-planes-simd16.patch](data/nightly-features-2026-10-28-byte-planes-simd16.patch)

Bucle de medición (cada proceso fijado al núcleo 4; el argumento `9` de `byte_plane_bench` y el segundo de `checksum_bench` son las repeticiones por proceso, `64` son MiB):
```
cargo build --offline --profile perf -p viewer_format --example checksum_bench
# CRC32: 5 procesos por lote, 3 lotes de línea base y 2 con cambio; se toma la mediana de las 9 repeticiones slicing8 de cada proceso
for i in 1 2 3 4 5; do taskset -c 4 target/perf/examples/checksum_bench 64 9; done
cargo build --offline --profile perf -p viewer_format --example byte_plane_bench
# planos de bytes: 6 procesos, 9 repeticiones cada uno
for i in 1 2 3 4 5 6; do taskset -c 4 target/perf/examples/byte_plane_bench 9 > run$i.txt; done
```
