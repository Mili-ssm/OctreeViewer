# 0033 GPU mínima de 4 GB y corte ajustado al pool

Estado: Aceptada · Fecha: 05-11-2026 · Decide: propietario · Sustituye la resolución P2 del [30-10-2026](../requirements.md#resoluciones-del-30-10-2026) y el supuesto de ~2 GB de los ADR [0019](0019-cubos-por-nodo-residentes-en-gpu.md) y [0027](0027-celdas-por-nodo-en-pool-paginado.md) · Revisa el disparador de `DescendantTooFine` del [ADR 0031](0031-celdas-por-nodo-en-v7-5.md)

## Contexto
- P2 fijaba como equipo de referencia (EQ-REF) una iGPU con ~2 GB compartidos y 16 GB de RAM.
- En el mismo spike, `DescendantTooFine` aparece en 4 de 24 transiciones (16,7 %), todas en saltos de pose (de la rasante a dentro de una hoja). El ADR 0031 pide revisar la máscara por encima del 5 %. Con navegación continua (8→4→2→1 px dentro de una pose) no aparece.
- El pool de celdas de R3 es de 256 MiB (ADR 0027 y [ADR 0031](0031-celdas-por-nodo-en-v7-5.md)). En el [spike de cobertura](../records/benchmarks/R3_T4_COVERAGE_05-11-2026.md), el corte deseado de Majadas desde dentro de una hoja a 1 px ocupa 269,4 MiB con instancia de 12 B. La ruta CPU tampoco construye ese caso: supera su límite de 256 MiB.

## Decisión
- **GPU mínima:** GPU o iGPU con **≥ 4 GB** disponibles para WebGPU, más 16 GB de RAM. Una iGPU como la Radeon 880M cuenta si su memoria compartida se configura a ≥ 4 GB.
- **Saltos de pose (revisión del disparador de 0031):** la máscara de 8³ se mantiene. Una actualización de corte que daría `DescendantTooFine` se rechaza y se retiene el corte visible hasta que el nuevo esté completo, como ya fija el ADR 0027 para los descendientes más finos que las celdas del padre. No es la alternativa que rechazó 0031 («retener el corte anterior mientras falta un nivel»): las etapas siguen dibujando el residente más grueso. La congelación se mide en la Tarea 11.
- **Corte ajustado al pool:** si el corte deseado no cabe en el pool, los nodos más lejanos bajan a un nivel guardado más grueso, que se dibuja como etapa contada (R3-Q3), hasta que el corte quepa. El pool sigue en 256 MiB; su tamaño se revisa en la Tarea 17 del plan de R3.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Mantener 2 GB (P2) | Deja sin margen el pool de 256 MiB y los buffers del render |
| Subir el pool a 512 MiB sin ajustar el corte | No evita que otro dataset o pose lo supere |
| Aceptar el caso como extremo | El corte desbordaría el pool sin una regla de degradación |

## Consecuencias
- EQ-REF y los escenarios de las puertas pasan a ≥ 4 GB de GPU.
- El selector de corte de R3 necesita el tamaño del corte en bytes y un orden por distancia para bajar de nivel. Se implementa con el pool (Tarea 7).
- «Objetivo alcanzado» (R3-Q3) no se cumple mientras haya nodos bajados por el pool, y se cuenta.

## Revisar si
- La Tarea 17 mide que con 4 GB cabe un pool mayor sin coste de tiempo de frame.
- Más del 5 % de los cortes en las poses de T4 necesitan bajar nodos por el pool.
