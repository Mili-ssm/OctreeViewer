# 0039 Hijos en curso y bloqueo retenido en el ajuste por presupuesto de CPU

Estado: Aceptada · Fecha: 14-11-2026 · Decide: propietario (R3-Q30) · Enmienda el [ADR 0037](0037-corte-ajustado-al-presupuesto-de-cpu.md)

## Contexto
- Al corregir el bucle de olvido de Majadas a 1 px ([registro de B1c](../records/benchmarks/R3_B1C_CPU_FIT_14-11-2026.md)) hicieron falta tres reglas que el ADR 0037 no detalla.
- Un hijo solo sustituye a su padre en el plan cuando están listos todos sus hermanos. Los hijos ya construidos de un grupo incompleto quedaban fuera del corte y del soporte, y el driver los olvidaba: ~29 hijos, ~37 olvidos por segundo.
- Al soltar esos hijos con el presupuesto lleno, los bytes necesarios bajaban del 80 %, el bloqueo se levantaba y se volvían a construir: una oscilación que reproduce un test.

## Decisión
- **Hijos en curso:** los nodos seleccionados ya construidos que aún no están en el corte van al soporte y cuentan en los bytes necesarios del ADR 0037.
- **Bloqueo de todos los candidatos:** con el presupuesto lleno se bloquean todos los candidatos sin construir, no solo el primero que no cabe y los más lejanos: tras desbordar, los bytes necesarios rondan el 90 % y ninguno cabría. Así no se construye nada para tirarlo.
- **Bloqueo retenido:** mientras el presupuesto bloquea, los hijos en curso salen del soporte y no se reconstruyen. Los bytes que ocupaban siguen contando para levantar el bloqueo (umbral del 80 %) hasta que cambie la selección. Un cambio de vista real aplica el 90/80 normal.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Construir por grupo de hermanos | Quita las expulsiones de la caché de bloques, pero no evita el olvido con el presupuesto lleno |
| Levantar el bloqueo solo con los bytes actuales | Oscila: suelta, baja del 80 %, reconstruye y vuelve a bloquear |

## Consecuencias
- Con el presupuesto lleno, la zona bloqueada se queda en el padre (más grueso), sin olvidos en reposo.
- Un nodo cercano cuyo bloque llega tarde puede quedar bloqueado hasta el siguiente cambio de vista.

## Revisar si
- La Tarea 17 mide cortes bloqueados en navegación normal, fuera de las poses extremas a 1 px.
