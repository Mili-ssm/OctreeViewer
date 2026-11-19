# 0057 `release` optimiza velocidad (`opt-level = 3`)

Estado: Aceptada · Fecha: 19-11-2026 · Decide: propietario ([#20](https://github.com/Mili-ssm/OctreeViewer/issues/20)) · Sustituye al [ADR 0008](0008-perfiles-release-y-perf.md) en el `opt-level` de `release`; el resto del 0008 (perfiles `perf` y `fast`, registrar el perfil de cada medida) sigue vigente

## Contexto
- El ADR 0008 fijó `opt-level = "s"` en `release` para que el WASM publicado pesara poco.
- El 02-11-2026 el commit `9185a6f` pasó `release` a `opt-level = 3` sin ADR. Su medida: conversión nativa
  ~20 % más rápida, el visor web llega a su objetivo ~18 % antes con la misma salida, y la descarga con brotli
  crece ~8 %.
- El WASM de la demo pesa 3,57 MB sin comprimir (19-11-2026). El render y la carga dependen de la CPU del WASM.

## Decisión
`[profile.release]` usa `opt-level = 3`, LTO completo y `codegen-units = 1`.

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Volver a `opt-level = "s"` o `"z"` | Pierde la velocidad medida a cambio de ~8 % de descarga |
| Medir de nuevo antes de decidir | La medida del 02-11-2026 ya da el orden de magnitud; el propietario prefiere velocidad |

## Consecuencias
- Las cifras de `release` desde el 02-11-2026 son de `opt-level = 3`; las anteriores no se comparan con ellas.
- `perf` y `release` difieren ahora solo en LTO y unidades de código.

## Revisar si
El tamaño de la descarga pasa a ser un problema medido (p. ej. tiempo de primera carga en móvil) o `"s"` deja de
costar velocidad.
