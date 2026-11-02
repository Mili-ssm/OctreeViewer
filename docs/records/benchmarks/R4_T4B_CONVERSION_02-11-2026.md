# R4 T4b: conversión `v7.4-pre` frente a V5

Fecha: 02-11-2026. Revisión: `1775104`. Estado: **criterio de parada de la Tarea 4b superado (tiempo); vuelve al propietario**.

## Método

`target/release/viewer_convert <LAZ> <salida> --format-version v5|v7.4-pre` con las opciones por defecto (4 workers, 512 MiB), medido con `/usr/bin/time -v`; una ejecución por fila, en serie, en el mismo equipo. Carga previa (1/5/15 min) en cada `.pre.txt`: 2,6–5,3, es decir, el equipo no estaba en reposo. Datos: [r4-t4b-02-11-2026/](data/r4-t4b-02-11-2026/).

## Resultados

| | V5 | `v7.4-pre` | Diferencia | Límite (plan, RQ-07) |
| --- | --- | --- | --- | --- |
| Majadas: tiempo | 222,7 s | 428,0 s | **+92 %** | +30 % |
| Majadas: RSS pico | 92 MB | 226 MB | +146 % | 512 MiB |
| Majadas: disco | 7 533 781 717 B | 7 656 270 032 B | +1,6 % | +35 % |
| Belvedere: tiempo | 27,8 s | 59,7 s | **+115 %** | +30 % |
| Belvedere: RSS pico | 61 MB | 98 MB | +60 % | 512 MiB |
| Belvedere: disco | 1 591 650 185 B | 1 712 809 917 B | +7,6 % | +35 % |

## Lectura

- El disco y la RSS cumplen con mucho margen; el tiempo no.
- El spike de la Tarea 1 comparaba con el conversor `v7.3-pre` (287 s en Majadas). V5 es más rápido (223 s), así que el margen frente a V5 es menor de lo que indicaba aquel registro. *Inferido*.
- La `v7.4-pre` añade una lectura más por hoja para medir el espaciado; su coste no está aislado.
- RQ-07 aplica al V7 final; este es un criterio de parada intermedio del plan.

## Límites

- Una ejecución por fila con el equipo cargado: sin varianza.
