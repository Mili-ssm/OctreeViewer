# 0037 Corte de celdas ajustado al presupuesto de CPU

Estado: Aceptada · Fecha: 13-11-2026 · Decide: propietario (R3-Q27) · Extiende el [ADR 0033](0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md) del pool de GPU a la memoria de CPU

## Contexto
- El driver de celdas guarda los niveles empaquetados de cada nodo hasta un presupuesto de CPU: 512 MiB en wasm (`WASM_CPU_BUDGET_BYTES`). Por encima de él olvida los nodos fuera del corte y del soporte, del más lejano al más cercano.
- En la [Tarea 11](../records/benchmarks/R3_T11_SMOKE_13-11-2026.md), Majadas a 1 px dentro de una hoja no se asienta: 12.046 olvidos en 300 s con las celdas en el tope. En nativo, la hoja +x a 1 px ocupa 824 MiB ([T9](../records/benchmarks/R3_T9_CELLS_06-11-2026.md)).
- El corte que recibe el driver es el de la etapa de px vigente. Lo que el planificador construye para refinar queda fuera de él, se olvida al llegar el corte siguiente y se construye otra vez. La histéresis en el olvido no basta: el conjunto de trabajo no cabe.

## Decisión
- **El planificador ajusta el corte al presupuesto de CPU.** Admite los nodos por construir del más cercano al más lejano mientras los bytes que necesita el corte (corte, soporte y nodos admitidos, con su tamaño empaquetado) quepan en el 90 % del presupuesto. El primero que no cabe y todos los más lejanos quedan bloqueados: esa zona se dibuja con el antepasado que ya tiene celdas.
- **Histéresis 90/80**, como el ajuste del pool: el bloqueo se levanta cuando los bytes necesarios bajan del 80 %.
- Los nodos bloqueados no cuentan como trabajo pendiente: la escalera de px termina y el corte se asienta.
- El driver no cambia: sigue sin olvidar el corte ni el soporte.
- Se cuenta como `cpuCoarsened`. «Objetivo alcanzado» (R3-Q3) exige `cpuCoarsened` = 0, como `poolCoarsened` = 0; un corte asentado con nodos bloqueados no es un fallo de la Tarea 11.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Subir el presupuesto a 1 GiB | Majadas cabría, pero otra nube o pose volvería al bucle, y la pestaña llegaría a ~1,2 GB de memoria wasm |
| Solo histéresis en el olvido | No converge cuando el conjunto de trabajo supera el presupuesto |
| Parar la escalera de px entera en la etapa anterior | Engrosa también lo cercano, que sí cabe |

## Consecuencias
- El planificador necesita el tamaño empaquetado de cada nodo construido, con la misma medida que el driver, y el presupuesto de CPU en su configuración.
- En poses extremas, lo lejano queda más grueso que lo pedido, y se ve en las estadísticas.
- Un nodo que desborda puede construirse una vez y descartarse; no se repite mientras dure el bloqueo.

## Revisar si
- La Tarea 17 muestra cortes bloqueados en navegación normal (fuera de los saltos dentro de una hoja a 1 px).
- Se mide que un presupuesto mayor no cuesta memoria en el equipo de referencia.
