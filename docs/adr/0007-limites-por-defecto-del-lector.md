# 0007 El lector aplica límites por defecto configurables

Estado: Propuesta · Fecha: fecha no registrada

## Contexto
Las cabeceras y el índice declaran tamaños controlados por el archivo; sin tope, un archivo corrupto o hostil reserva memoria arbitraria.

## Decisión
`FormatReaderLimits` (`crates/viewer_format/src/reader_limits.rs`): 256 atributos, 65 535 componentes, 1 MiB por cadena, 256 MiB de índice, 256 MiB por bloque, 2 000 000 nodos y 2 000 000 bloques. Esquemas mayores se abren con `FormatReader::with_limits`. Pregunta abierta: ¿debe el writer rechazar esquemas que el lector por defecto no puede abrir?

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Sin límites | Reserva ilimitada ante archivos hostiles. |
| Writer rechaza esquemas >256 | Aún sin decidir: evita archivos que el lector por defecto no abre, pero impone un tope al formato, que sí admite esquemas anchos. |

## Consecuencias
Los archivos válidos grandes necesitan opt-in. No hay todavía test de regresión para esquemas >256 atributos (pendiente).

## Revisar si
Se decide la política de compatibilidad por defecto, o un corpus real supera algún límite.

## Evidencia
[reader_limits.rs](../../crates/viewer_format/src/reader_limits.rs); [inventario](../reference/inventory.md#límites)
