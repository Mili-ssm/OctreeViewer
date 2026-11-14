# 0040 Pool de celdas de 512 MiB y corte de nodos ajustado al pool

Estado: Aceptada · Fecha: 14-11-2026 · Decide: propietario (R3-Q29) · Enmienda el [ADR 0033](0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md)

## Contexto
- El ADR 0033 ajusta el corte al pool de GPU bajando niveles **dentro** de cada nodo, con un pool de 256 MiB (32.768 páginas) que la Tarea 17 debía revisar.
- En Belvedere a 1 px tras un salto, el corte de 624 nodos no cabe ni con los niveles más gruesos de cada nodo: el pool desborda en cada frame y el driver retiene el corte anterior durante 282 s, sin avanzar ([registro de B1c](../records/benchmarks/R3_B1C_CPU_FIT_14-11-2026.md#belvedere-a-1-px-tras-el-salto)). El ADR 0033 no tiene regla para ese caso.

## Decisión
- **Pool de 512 MiB** (el doble de páginas), dentro de la GPU mínima de 4 GB del ADR 0033. Si el adaptador no admite los tamaños de buffer y de binding que pide, el pool se queda en lo que admita (con 256 MiB como mínimo de la ruta de celdas) y se informa en las estadísticas.
- **Corte de nodos ajustado al pool:** el planificador admite nodos mientras los niveles más gruesos del corte quepan en el 90 % del pool, con el mismo orden por cercanía, histéresis 90/80 y bloqueo que el ajuste de CPU (ADR 0037 y 0039). Lo lejano se queda en el padre y se cuenta.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Solo pool de 512 MiB | Otra nube o pose podría volver a desbordar |
| Solo corte ajustado al pool | Más engrosamiento en la práctica con la GPU mínima ya garantizada |

## Consecuencias
- La memoria de GPU de la ruta de celdas sube a ~512 MiB más los buffers del render.
- El planificador necesita el tamaño en páginas del nivel más grueso de cada nodo, que conoce al empaquetarlo.
- «Objetivo alcanzado» (R3-Q3) exige también que no haya nodos bloqueados por el pool.

## Revisar si
- La Tarea 17 mide coste de frame o de memoria por el pool mayor en el equipo de referencia.
- Algún adaptador objetivo no admite el pool de 512 MiB.
