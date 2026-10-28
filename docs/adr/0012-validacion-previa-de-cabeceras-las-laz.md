# 0012 Validar cabeceras LAS/LAZ antes de que las crates las parseen

Estado: Aceptada · Fecha: 28-10-2026

## Contexto
Las crates `las`/`laz` reservan memoria según tamaños declarados en la cabecera; el changelog de `las` 0.11.2 (leído al actualizar dependencias, 28-10-2026) solo menciona una constante obsoleta y CI, así que esas reservas no están corregidas upstream; además un archivo hostil puede forzar reservas de GiB.

## Decisión
`viewer_core::las::preflight_las` acota los tamaños declarados (VLR/EVLR, tabla de chunks, conteos) por los bytes reales del fichero y por el mínimo de registro del formato, y rechaza antes del parseo. Se aplica en las rutas nativas (LAS, EPT) y WASM.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Confiar en las crates | Reservas ilimitadas por cabecera. |
| Límite fijo | Rechazaría datos reales grandes; la cota por tamaño real no. |

## Consecuencias
Comprobado con 228 archivos LAS/LAZ reales y 2100 generados sin falsos rechazos, y 3 PoC rechazados. Un archivo válido con chunks menores que un registro se rechazaría (ninguno visto). Pendiente reportar a upstream (el desbordamiento en debug de `raw/header.rs:359` se observó en `las` 0.11.1 y no se reverificó en 0.11.2).

## Revisar si
Upstream corrige las reservas, o aparece un falso rechazo en datos reales.

## Evidencia
[las.rs](../../crates/viewer_core/src/las.rs); [evaluación 28-10](../records/archive/CODE_ASSESSMENT_2026-10-28.md)
