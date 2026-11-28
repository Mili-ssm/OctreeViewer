# 0012 Validar cabeceras LAS/LAZ antes de que las crates las parseen

Estado: Aceptada · Fecha: 28-10-2026 · Ubicación precisada por [0062](0062-el-preflight-las-vive-en-el-nucleo-de-conversion.md)

## Contexto
Las crates `las`/`laz` reservan memoria según tamaños declarados en la cabecera; la diferencia de fuentes de `las` 0.11.1 a 0.11.2 (28-10-2026) es formato y un `use std::f64` eliminado, así que esas reservas no están corregidas upstream y el desbordamiento en debug de `raw/header.rs:359` no cambia en 0.11.2; además un archivo hostil puede forzar reservas de GiB. El descompresor paralelo de `laz` 0.13.0 (feature `laz-parallel` de `viewer_convert`) reserva según `point_count` y `byte_count` de la tabla, decodificados como i32 con signo extendido a u64: un conteo de chunks válido no basta.

## Decisión
`viewer_core::las::preflight_las` acota los tamaños declarados (VLR/EVLR, tabla de chunks, conteos) por los bytes reales del fichero y por el mínimo de registro del formato, y rechaza antes del parseo. Alcance: el preflight de cabecera (`viewer_core`) acota EVLR y el número de chunks y se aplica en las rutas nativas (LAS, EPT) y WASM. La ruta nativa añade `viewer_convert::las_source`, que decodifica la tabla con `laz::laszip::ChunkTable` y rechaza entradas `byte_count`/`point_count` negativas o fuera de la cota del fichero. Además se aplica un tope absoluto de 256 MiB descomprimidos por chunk (`MAX_LAZ_CHUNK_BYTES`): `chunk_size` fijo × tamaño de punto en el preflight de cabecera y `point_count` variable × tamaño de punto en `viewer_convert`, con tamaño de punto = máx(longitud de registro, suma de ítems del VLR laszip), porque la cabecera la controla el atacante y `laz` reserva según esos valores sin acotarlos. WASM usa el descompresor secuencial (solo feature `laz`) y no queda afectada por las entradas, pero el tope de `chunk_size` fijo vive en el preflight compartido y también se aplica allí: un LAZ legal con chunk fijo mayor de 256 MiB descomprimidos se rechaza en el navegador (los escritores habituales usan 50 000 puntos y COPC usa chunks variables).

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Confiar en las crates | Reservas ilimitadas por cabecera. |
| Límite fijo | Rechazaría datos reales grandes; la cota por tamaño real no. |

## Consecuencias
Comprobado con 228 archivos LAS/LAZ reales y 2100 generados sin falsos rechazos, y 8 PoC rechazados (3 de cabecera y 5 de tabla o tamaño de chunk). Un archivo válido con chunks menores que un registro se rechazaría (ninguno visto). Pendiente reportar a upstream: reserva de EVLR en `las`; capacidad y entradas de la tabla de chunks en `laz`; desbordamiento en debug de `raw/header.rs:359`.

## Revisar si
Upstream corrige las reservas, o aparece un falso rechazo en datos reales.

## Evidencia
[las.rs](../../crates/viewer_core/src/las.rs); [evaluación 28-10](../records/archive/CODE_ASSESSMENT_2026-10-28.md)
