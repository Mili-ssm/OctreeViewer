# 0021 Rejilla de cuantización global determinista anclada en el CRS

Estado: Aceptada · Fecha: 30-10-2026

## Contexto
- Varias nubes se dibujan a la vez (RF-01, hasta ~100 nubes) y se fusionan (RF-21). Con una rejilla por nube, los cubos de las zonas solapadas no coinciden.
- El CRS debe constar en el formato (RF-24).

## Decisión
- La rejilla de cuantización se ancla de forma determinista en el CRS, con tamaños de celda diádicos, independiente de la extensión, del lugar y del momento de la conversión: dos nubes con el mismo CRS producen celdas coincidentes en todos los niveles.
- El CRS se guarda en el formato.
- El visor rechaza abrir una nube con CRS distinto al de las abiertas, con un mensaje que nombra ambos (RF-02).
- El CLI nativo reproyecta a un CRS de destino (RF-22). PROJ solo en nativo, nunca en wasm.
- Ancla, unidad base, ejes y datum vertical se fijan en la especificación del formato de R4 (**pendiente**).

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Rejilla por nube | Cubos desalineados en los solapes. |
| Reproyectar en el visor | Exige PROJ en wasm y los cubos reproyectados quedan desalineados. |
| Rechazar sin ofrecer reproyección | No resuelve el uso con CRS distintos (RF-22). |

## Consecuencias
- Nueva dependencia nativa para reproyectar.
- Cambia el marco de cuantización: regeneración de datasets dentro de V7 ([ADR 0022](0022-alcance-y-congelacion-de-v7-final.md)).
- La [ADR 0016](0016-norma-portatil-en-el-error-de-cuantizacion.md) y la paridad web/CLI (RQ-08) siguen vigentes.
- Habilita carga multinube (RF-01) y fusión (RF-21). **Pendiente:** todo.

## Revisar si
Un caso real exige mezclar CRS en el visor, o la rejilla diádica degrada la precisión de algún CRS.

## Evidencia
[Requisitos RF-02, RF-22, RF-23](../requirements.md) · [Plan V7](../plans/format-v7-plan.md)
