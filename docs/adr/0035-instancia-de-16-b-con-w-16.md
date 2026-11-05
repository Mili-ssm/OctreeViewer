# 0035 Instancia de 16 B con W = 16 y compactación con Hi-Z antes de la Tarea 6

Estado: Aceptada · Fecha: 05-11-2026 · Decide: propietario · Enmienda a [ADR 0032](0032-hojas-adaptativas-e-instancia-de-12-b.md) (instancia) y al plan de R3 (Tarea 16)

## Contexto
- El [spike de GPU](../records/benchmarks/R3_GPU_SPIKE_05-11-2026.md) dibujó en la Radeon 880M los cortes reales empaquetados como el pool.
- **Con W = 10, las páginas de Majadas quedan vacías.** Las hojas dispersas, separadas muy abajo por la continuación adaptativa (0032), dejan pocas celdas en cada región de 2¹⁰. Dentro de una hoja a 1 px, W = 10 usa 34,1 M de huecos (38,7 % vacíos) frente a 21,1 M con W = 16 (1,1 % vacíos). En la rasante a 1 px los vacíos llegan al 84 %. Esto supera el disparador de 0032 (más del 20 % de bytes de página sin usar).
- **Coste en GPU.** En Majadas, dentro de una hoja a 4 px, W = 10 tarda 27,96 ms y W = 16 tarda 20,84 ms (+34 %); a 1 px, +53 %. En Belvedere empatan (7,66 frente a 7,74 ms). Las dos variantes superan los 16,7 ms del criterio de parada de la Tarea 5 en Majadas.
- **Dónde se va el tiempo.** Dentro de una hoja solo se ve entre el 0,75 % y el 4 % de las celdas dibujadas. Dibujar solo esas celdas, sin huecos ni descartes (control de 40 B), baja Majadas a 4 px a 14,57 ms.

## Decisión
- **Instancia de 16 B con W = 16:**
  - posición de 16 bits por eje relativa al origen de la página;
  - 16 bits de flags (libres por ahora);
  - rgba8;
  - intensidad `u16` y clase `i16`.
  - Páginas de 8 KiB con 512 huecos. Se parten en orden Morton si el rango supera 2¹⁶ en algún eje, si se llenan o si cambian los flags de página.
  - Sustituye a la instancia de 12 B con W = 10 de 0032; el resto de 0032 sigue vigente.
- **Tarea 16 obligatoria antes de la Tarea 6:** un pase de cómputo compacta los huecos y las celdas descartadas (máscara 8³ y octantes que dibujan otro nivel) y recorta por oclusión con Hi-Z por página. Se mantienen los cubos rasterizados del [ADR 0019](0019-cubos-por-nodo-residentes-en-gpu.md).
- **Raymarching** (OQ-08): solo si la compactación con Hi-Z no baja Majadas dentro de una hoja a 4 px de 16,7 ms.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Mantener 12 B y cambiar el reparto de páginas | Necesita otra ronda de spike, y W = 16 ya gana o empata en los dos datasets |
| 12 B en niveles densos y 16 B en los dispersos | Dos decodificaciones en el shader y en el pool |
| Prototipo de raymarching ahora | Reescribe el render y obliga a sustituir el ADR 0019. Dibujar solo las celdas útiles da una cota inferior de 14,57 ms, todavía sin medir la compactación real |
| Relajar el criterio a RQ-01 (33 ms) | Sin margen para la iGPU: Majadas a 1 px tarda 62 ms con 16 B |

## Consecuencias
- La Tarea 6 implementa la instancia de 16 B. Su decodificación en Rust y en WGSL debe coincidir bit a bit.
- La Tarea 16 se especifica y pasa por pre-mortem antes de la 6. Su referencia de rendimiento es el spike de GPU (`examples/web-host/spike-cube-pool.*`).
- El presupuesto del pool (ADR 0033) se recalcula con 16 B por hueco. *Inferido*: el corte de Majadas dentro de una hoja a 1 px, que con 12 B ocupaba 269,4 MiB, pasaría a ~359 MiB, así que ahí actúa el ajuste del corte al pool.

## Revisar si
- La Tarea 16 no baja Majadas dentro de una hoja a 4 px de 16,7 ms → raymarching (OQ-08).
- Un dataset con hojas densas muestra que W = 16 cuesta más de un 10 % de GPU frente a W = 10.
