# 0009 El transporte web es Blob local, sin HTTP Range

Estado: Sustituida por [0023](0023-fuentes-locales-y-remotas.md) · Fecha: 25-10-2026 (decisión confirmada en el plan V7)

## Contexto
El navegador abre archivos locales; la lectura asíncrona usa `Blob.slice(...).arrayBuffer()` con planes de rangos, cuota y prioridad LOD.

## Decisión
El viewer lee dataset local por Blob (cabecera, footer, índice acotado y rangos exactos). Las interfaces por rangos no acreditan transporte HTTP Range/remoto; este queda fuera de alcance hasta contar solicitudes, latencia y datos en vuelo.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| HTTP Range/remoto ahora | Exige cancelación, fallos y coalescing sin medidas; extrapolar desde NVMe local no es evidencia. |

## Consecuencias
Sin fuentes remotas; se evita introducir async/concurrencia por extrapolación.

## Revisar si
Requisito de producto de servir datasets remotos.

## Evidencia
[Plan V7](../explanation/format-v7-plan.md); [inventario](../reference/inventory.md#límites)
