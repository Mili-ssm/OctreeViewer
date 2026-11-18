# 0023 El visor lee fuentes locales (Blob) y remotas (HTTP Range)

Estado: Aceptada · Fecha: 30-10-2026 · Sustituye a [0009](0009-transporte-web-por-blob-local.md)

## Contexto
- La [ADR 0009](0009-transporte-web-por-blob-local.md) fijó Blob local sin HTTP Range. Los usuarios incluyen una app web pública y una librería embebible, y un dataset de ~500 GB no cabe siempre en el equipo del usuario.
- Pregunta [P6](../requirements.md#resoluciones-del-30-10-2026): la recomendación era mantener local y preparar la paginación.

## Decisión
- El visor lee datasets locales (`Blob`) y remotos (HTTP Range).
- La jerarquía paginada de R5 se diseña para ambos.
- Puerta remota RQ-04: primer frame ≤ 10 s y objetivo de 4 px ≤ 120 s a 100 Mbit/s y 50 ms de RTT (RED-REF).

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Solo local | Excluye la app pública con datasets alojados y al integrador. |
| Remoto primero | Hoy solo el caso local está implementado y medido. |

## Consecuencias
- Los servidores necesitan CORS y soporte de Range.
- Hay que agrupar peticiones para amortiguar la latencia.
- Los bytes remotos son entrada no confiable: aplica la validación previa existente ([ADR 0012](0012-validacion-previa-de-cabeceras-las-laz.md)) y los límites del lector ([ADR 0007](0007-limites-por-defecto-del-lector.md)).
- **Pendiente:** no hay lector remoto; ASM-02 (decenas de MB por vista) se comprueba en R5.

## Revisar si
RQ-04 no es alcanzable con la jerarquía paginada en RED-REF.

## Evidencia
[Requisitos RF-09, RQ-04, RED-REF](../requirements.md) · [Plan de render, R5](../records/archive/render-y-carga-escala.md)
