# 0003 V6 final y pre5 permanecen rechazados

Estado: Aceptada · Fecha: 14-10-2026 (según los registros de v6)

## Contexto
Pre1–pre4 exploraron deltas, ocupación por profundidad, constantes/diccionarios y planos de bytes. Los candidatos siguientes (RGB agrupado, bitpacking por rango, índice compacto) no se evaluaron y siguen sin evidencia (fila «Siguientes candidatos v6» del README, [entregas](../records/archive/DELIVERIES_2026-10-14.md), [plan V7](../plans/format-v7-plan.md)); el desarrollo se trasladó a V7.

## Decisión
La selección admite exactamente v5 y `v6.1-pre`–`v6.4-pre`. V6 final, pre5 y otras identidades se reconocen por cabecera pero se rechazan: no se infiere compatibilidad por orden.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Congelar pre4 como v6 final | Sin evidencia de que sea el diseño final; V7 absorbe la evolución. |
| Aceptar identidades desconocidas por orden | Una revisión incompatible se leería con un decoder erróneo. |

## Consecuencias
Los archivos pre1–pre4 siguen legibles para comparación; nuevas ideas de formato se evalúan solo en V7.

## Revisar si
Se decide retener V6 más allá de la transición (contradice ADR 0004).

## Evidencia
[Preparación v6](../records/benchmarks/STORAGE_V6_PREPARATION.md) (:218); [entregas 14-10](../records/archive/DELIVERIES_2026-10-14.md); [pre4 vs pre3](../records/benchmarks/SENSOFOREST_V6_BENCH.md)
