# 0020 La pirámide LOD es densa: un nivel por cada ×2 de densidad (R4)

Estado: Aceptada · Fecha: 30-10-2026

## Contexto
- Los nodos internos guardan 8³ representantes frente a hojas de hasta 262 144 puntos: ~4 niveles de densidad colapsados (B2 de la [evaluación](../records/archive/RENDER_ASSESSMENT_2026-10-30.md)).
- Se leen 1 100–2 700 puntos por vóxel dibujado; Belvedere no completó 4 px en 600 s.

## Decisión
- Un nivel por cada ×2 de densidad, hojas de ≤ 16–64 k puntos y espaciado por nodo en el índice.
- Los datasets se reconvierten.
- Se entrega dentro de V7 ([ADR 0022](0022-alcance-y-congelacion-de-v7-final.md)).

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Mantener 8³ representantes | Mantiene la lectura por vóxel y el incumplimiento de RQ-03 a escala. |
| Spike medido antes de decidir | El propietario decidió ahora ([P3](../requirements.md#resoluciones-del-30-10-2026)); la medición real de disco y conversión queda en el criterio de revisión. |

## Consecuencias
- Disco +15–35 % (estimación, sin medir; RQ-07).
- Más nodos: exige la jerarquía paginada (R5) y supera el límite por defecto de 2 000 000 nodos del lector ([ADR 0007](0007-limites-por-defecto-del-lector.md)) en datasets grandes.
- R2b (caché de celdas agregadas) queda absorbida por R4.
- **Pendiente:** especificación del contrato de densidad y del formato; nada implementado.

## Revisar si
El disco o el coste de conversión medidos incumplen RQ-07.

## Evidencia
[Pregunta P3](../requirements.md#resoluciones-del-30-10-2026) · [Plan de render, R4](../records/archive/render-y-carga-escala.md) · [Requisitos RQ-07](../requirements.md)
