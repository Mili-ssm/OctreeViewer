# 0005 Cada candidato de writer V7 tiene identidad exacta `v7.N-pre`

Estado: Aceptada (revisar) · Fecha: 25-10-2026 (según el plan V7)

## Contexto
V7.2-pre restringe la ocupación a compact24 y V7.3-pre limita diccionarios a tuplas de 1/2/4 B: cambian la heurística del writer, no el decoder.

## Decisión
Cada revisión tiene revisión de cabecera propia y magics `OCTVI7xP`/`OCTVE7xP` (`crates/viewer_format/src/layout.rs`), válidos para archivos, acompañantes, bloques preparados y checkpoints. Los checkpoints de otra identidad se rechazan; V7.1/7.2/7.3 comparten checkpoint EPT por bloques.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Un perfil de encoder con una única identidad V7 | Menos identidades y lectura uniforme, pero un checkpoint o una comparación no sabría qué heurística produjo los bytes; permitiría mezclar candidatos al reanudar. |

## Consecuencias
Más identidades (cada una con especificación y rama en el despacho); comparaciones A/B exactas y reanudación segura. No se reduce disco por la identidad.

## Revisar si
V7.2/V7.3 solo cambian heurísticas del writer y el coste de identidades supera su valor de trazabilidad; o al promover V7 final (se colapsan).

## Evidencia
[V7.1](../reference/format/FORMAT_V7_PRE1.md), [V7.2](../reference/format/FORMAT_V7_PRE2.md), [V7.3](../reference/format/FORMAT_V7_PRE3.md); [evidencia](../records/benchmarks/format-v7-evidence.md)
