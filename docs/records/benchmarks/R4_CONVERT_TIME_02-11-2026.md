# R4 T1: tiempo del conversor con hoja 64 k y `sample_grid` = 256

Fecha: 02-11-2026. Estado: **+32 % en Majadas frente al conversor anterior; RQ-07 pide ≤ +30 %**.

Sigue a la [corrección de RSS](R4_SPARSE_GRID_RSS_02-11-2026.md). El propietario pidió perfilar y optimizar el tiempo (02-11-2026).

## Diagnóstico (Belvedere, 20 núcleos, 4 workers)

| Fase | 262 k / g = 8 | 64 k / g = 256 | Núcleos usados (g = 256) |
| --- | --- | --- | --- |
| Escaneo raíz | 16 s | 22,7 s | 1,2–1,8 (en serie) |
| Jerarquía | 7,4 s | 17 s | 4 |
| Codificar y escribir bloques | 15 s | 32 s | 1,6–2,5 (barrera por lote) |

- El trabajo extra es propio de la pirámide densa: unos 3,6 GB más de LOD interno y 5 239 bloques frente a 1 343.
- La rejilla dispersa se lleva unos 25 % de las muestras de CPU; no es la causa principal.

## Cambios

- `root_partition_ranges` pasa a 4 por defecto, limitado por `admitted_root_ranges`.
- `process_batches` usa una ventana deslizante de hasta 2 × workers elementos preparados en lugar de una barrera por lote. Escribe en el mismo orden.
- `block_stream_max_prepared_blocks` ya no mide los bloques en vuelo; la memoria de la ventana la acota 2 × workers, no `memory_limited_workers`.

## Resultados (una ejecución por fila)

| | Antes | Después | RSS después | Bytes de salida |
| --- | --- | --- | --- | --- |
| Belvedere 64 k / g = 256 | 73,3 s | **57,3 s** | 113 MB | iguales (SHA-256 iguales) |
| Belvedere 262 k / g = 8 | 40,6 s | 33,1 s | 93 MB | iguales (SHA-256 iguales) |
| Majadas 64 k / g = 256 | 499–506 s | **379,6 s** | 183 MB | mismo tamaño |
| Majadas 262 k / g = 8 | 287 s | 246,3 s | 126 MB | mismo tamaño |

Frente al conversor anterior (262 k / g = 8 antes del cambio): Majadas **+32 %**, Belvedere **+41 %**.

## Límites

- Una ejecución por fila, sin varianza; la diferencia con el +30 % de Majadas es del orden del ruido.
- Majadas no tiene hash previo: solo se compara el tamaño.
- Sin probar: subir los workers de 4 a 8 (unos −7 s en Belvedere; decisión de producto).
