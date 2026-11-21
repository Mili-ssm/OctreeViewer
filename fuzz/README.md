# Fuzzing (opcional)

Busca de forma mecánica entradas que nuestros parsers endurecidos aceptan y luego hacen entrar
en pánico o sobreasignar memoria. Crate aparte (`[workspace]` propio y `Cargo.lock` propio): no
forma parte del workspace ni de CI; `scripts/check.sh` solo comprueba que compila y está
formateado (`FUZZ_CHECK=0` lo omite).

Las compilaciones de `cargo fuzz` activan `--cfg fuzzing`: en `viewer_format` las comprobaciones
CRC-32 (`checksum_matches`) siempre aceptan, para que las mutaciones lleguen a los parsers que hay
tras los checksums. Es el único cambio de comportamiento bajo ese cfg; las compilaciones normales
verifican siempre y los tests de corrupción cubren la comprobación real.

| Objetivo | Entrada | Qué ejecuta |
| --- | --- | --- |
| `fuzz_format_reader` | bytes | `FormatReader::new` monolítico y, como segunda variante, `.oct` + `.octattrs` partidos (primeros 4 bytes LE = punto de corte); si abre, lee XYZ, colores y todas las columnas de todos los bloques |
| `fuzz_paged_reader` | bytes | `PagedReader::open` (V8, lectura de cola) y todas las páginas hijas encadenadas (máx. 256), leyendo los bytes de cada bloque; también liberando páginas. `fuzz_format_reader` ya cubre la vista ansiosa del mismo formato |
| `fuzz_las_preflight` | bytes | `preflight_las`; si pasa, `viewer_convert::convert_file` sobre archivos temporales (tmpfs, borrados en cada iteración): comprobación de la tabla de chunks LAZ (privada a `viewer_convert`, por eso no se llama directa), lector `las` con `laz-parallel` y conversión |

Un pánico, abort o asignación por encima de los límites de libFuzzer tras un preflight correcto
es un *bypass* del preflight.

Excepción: `convert_file` contiene los pánicos del decodificador LAZ de terceros (datos
aritméticos corruptos que ningún preflight de cabecera puede predecir) y los devuelve como
`ConvertError::LasDecoderPanicked`. Como libfuzzer-sys instala un hook que aborta antes de que
`catch_unwind` actúe, `fuzz_las_preflight` lo sustituye por uno silencioso; un pánico no
contenido sigue llegando al `catch_unwind` de libfuzzer-sys y se reporta como crash.

## Ejecutar

Requiere `cargo install cargo-fuzz` (usa el nightly fijado en `rust-toolchain.toml`).

```sh
cd fuzz
cargo run --bin gen_seeds   # regenera semillas en corpus/: solo V8 (muestra y cadenas de páginas, primario y partido) y LAS/LAZ 1.4 formatos 6-8 con y sin EVLR (solo las pequeñas se versionan)
cargo fuzz run fuzz_format_reader -- -max_total_time=900 -rss_limit_mb=2048 -malloc_limit_mb=1024
cargo fuzz run fuzz_paged_reader -- -max_total_time=900 -rss_limit_mb=2048 -malloc_limit_mb=1024
cargo fuzz run fuzz_las_preflight -- -max_total_time=900 -rss_limit_mb=2048 -malloc_limit_mb=1024
```

## Crashes

Se guardan en `fuzz/artifacts/<objetivo>/` (ignorado por git). Minimizar y reproducir:

```sh
cargo fuzz tmin <objetivo> artifacts/<objetivo>/crash-<hash>
cargo fuzz run <objetivo> artifacts/<objetivo>/minimized-from-<hash>
```

Para convertirlo en test de regresión: copiar la entrada mínima (o reconstruirla en el test si es
pequeña) a `crates/viewer_format/tests/` (formato) o `crates/viewer_convert/tests/las_preflight.rs`
(LAS/LAZ), afirmar un `Err` limpio sin pánico, arreglar la causa en el preflight o el lector y
comprobar que el test pasa solo después del arreglo.

`corpus/` solo versiona semillas pequeñas (`seed-*`); el resto se regenera y no se sube.
