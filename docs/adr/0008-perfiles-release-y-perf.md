# 0008 `release` optimiza tamaño; `perf` optimiza velocidad para benchmarks

Estado: Sustituida en parte por el [ADR 0057](0057-release-optimiza-velocidad.md) el 19-11-2026 (`release` usa `opt-level = 3`); el resto sigue aceptado · Fecha: fecha no registrada

## Contexto
El WASM publicado prioriza tamaño y LTO completo; los benchmarks nativos necesitan iterar sin el coste de LTO completo.

## Decisión
`[profile.release]`: `opt-level = "s"`, LTO completo, `codegen-units = 1`. `[profile.perf]` hereda release con `opt-level = 3`, LTO thin y 16 codegen units. `fast` hereda dev sin debuginfo para tests. Las medidas registran el perfil; no son intercambiables.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Un único perfil release | Benchmarks lentos de compilar o con optimización de tamaño que distorsiona el rendimiento nativo. |

## Consecuencias
Hay que registrar el perfil en cada medida y no comparar cifras entre perfiles.

## Revisar si
Se mide que `perf` y `release` difieren poco, o el WASM necesita `opt-level` 3.

## Evidencia
[Cargo.toml](../../Cargo.toml) (`[profile.*]`); [CONTRIBUTING](../../CONTRIBUTING.md#convenciones)
