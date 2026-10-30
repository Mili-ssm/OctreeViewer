# 0004 Retirar V5/V6 al completar V7

Estado: Aceptada · Fecha: 24-10-2026

## Contexto
Mantener tres familias de formato multiplica backends, opciones y suites. V7 es la única línea de evolución.

## Decisión
Tras completar V7 (contratos, comparaciones y validación nativa/WASM/navegador), promover V7 final/default, regenerar fixtures y datasets activos y eliminar backends y opciones V5/V6. Mientras tanto V5/V6 se conservan como compatibilidad y referencia de comparación.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Mantener V5 indefinidamente | Coste de mantenimiento y riesgo de divergencia de contratos. |
| Migración automática de V5/V6 | Sin lector legacy ni migrador; regenerar desde la fuente es más simple y verificable. |

## Consecuencias
Los datasets V5/V6 no se migran: se regeneran desde fuente. El inventario operativo de retirada está en el plan.

## Revisar si
V7 no alcanza criterios de cierre o aparece un consumidor externo de V5/V6.

## Evidencia
[Plan V7](../explanation/format-v7-plan.md#6-validación-promoción-y-retirada); [inventario de retirada](../explanation/format-v7-plan.md#inventario-operativo-para-la-promoción-y-retirada)

## Revisión 30-10-2026
El alcance y el congelado de V7 final se definen en la [ADR 0022](0022-alcance-y-congelacion-de-v7-final.md). V5 sigue siendo el valor por defecto hasta ese congelado.
