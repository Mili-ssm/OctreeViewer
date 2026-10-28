# 0013 Los módulos experimentales van tras la feature `experimental`

Estado: Aceptada · Fecha: 28-10-2026

## Contexto
Octree ocupado, resúmenes por nodo y diccionarios de tuplas no están integrados en el formato vigente, pero compilaban y se exportaban siempre.

## Decisión
`viewer_core` (`node_attributes`, `categorical_attribute`, `occupied_point_block`, índices) y `viewer_format` (`experimental_tuple_dictionary`) quedan tras `experimental`, apagada por defecto. `scripts/check.sh` prueba con y sin `--all-features`.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Siempre compilados | Superficie pública y mantenimiento sin integración. |
| Eliminarlos | Se conservan como experimentos con contrato. |

## Consecuencias
Los consumidores deben activar la feature; la API estable es menor.

## Revisar si
Un experimento se integra en el formato (sale de la feature) o se descarta.

## Evidencia
[inventario](../reference/inventory.md#componentes-y-puntos-de-entrada); [resúmenes](../explanation/NODE_ATTRIBUTE_SUMMARIES.md)
