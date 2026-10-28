# Fuzzing (opcional)

Busca de forma mecánica entradas que nuestros parsers endurecidos aceptan y luego hacen entrar
en pánico o sobreasignar memoria. Crate aparte (`[workspace]` propio y `Cargo.lock` propio): no
forma parte del workspace, de `scripts/check.sh` ni de CI.

| Objetivo | Entrada | Qué ejecuta |
| --- | --- | --- |
| `fuzz_format_reader` | bytes | `FormatReader::new` monolítico y, como segunda variante, `.oct` + `.octattrs` partidos (primeros 4 bytes LE = punto de corte); si abre, lee XYZ, colores y todas las columnas de todos los bloques |
| `fuzz_las_preflight` | bytes | `preflight_las`; si pasa, `viewer_convert::convert_file` sobre un archivo temporal (tmpfs): comprobación de la tabla de chunks LAZ (privada a `viewer_convert`, por eso no se llama directa), lector `las` con `laz-parallel` y conversión |

Un pánico, abort o asignación por encima de los límites de libFuzzer tras un preflight correcto
es un *bypass* del preflight.

## Ejecutar

Requiere `cargo install cargo-fuzz` (usa el nightly fijado en `rust-toolchain.toml`).

```sh
cd fuzz
cargo run --bin gen_seeds   # regenera semillas en corpus/ (solo las pequeñas se versionan)
cargo fuzz run fuzz_format_reader -- -max_total_time=900 -rss_limit_mb=2048 -malloc_limit_mb=1024
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
