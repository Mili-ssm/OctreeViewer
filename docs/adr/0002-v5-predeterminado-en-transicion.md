# 0002 V5 sigue siendo la salida predeterminada durante la transición a V7

Estado: Sustituida por el [ADR 0044](0044-v7-final-congelada-y-predeterminada.md) · Fecha: 24-10-2026 (decisión V7; fecha del valor por defecto no registrada)

## Contexto
V6 y V7 son revisiones optativas y experimentales; los datasets activos, fixtures y la muestra principal están en V5 y su migración precede a la retirada del backend.

## Decisión
`FormatVersion::DEFAULT_OUTPUT` es `V5` (`crates/viewer_format/src/version.rs`). V6/V7 se eligen con `--format-version`. No se desarrollan nuevas revisiones V5/V6.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Promover `v7.N-pre` a default | Son identidades de desarrollo sin medidas de pareja completa ni de corpus grande; regenerar datasets antes de tiempo. |
| Cambiar el default a la última revisión | El default cambia sin evidencia y rompe comparaciones con la muestra V5. |

## Consecuencias
Quien no pase `--format-version` obtiene V5. La promoción de V7 requiere regenerar fixtures y datasets activos (fase 6 del README).

## Revisar si
V7 final se promueve (fase 6) o aparece evidencia de pareja completa que justifique otro default.

## Evidencia
[Plan V7](../records/archive/format-v7-plan.md); [estado](../estado.md#prioridades-pendientes)

## Revisión 30-10-2026
El alcance y el congelado de V7 final se definen en la [ADR 0022](0022-alcance-y-congelacion-de-v7-final.md). V5 sigue siendo el valor por defecto hasta ese congelado.
