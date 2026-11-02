# R4 T1: segunda ronda de tiempo del conversor (hoja 64 k, g = 256)

Fecha: 02-11-2026. Estado: **dentro de RQ-07 en Majadas; Belvedere depende de la referencia elegida**.

Sigue a la [primera ronda](R4_CONVERT_TIME_02-11-2026.md). El propietario pidió seguir optimizando (02-11-2026).

## Cambios

- `sample_cell()` calcula índice y centro del vóxel en una pasada (antes, tres divisiones enteras por punto para deshacer el índice). Misma aritmética de coma flotante.
- La tabla de `SparseCells` guarda `clave << 32 | índice` en `u64`, se dimensiona desde el número de puntos del nodo (tope 65 536 celdas) y ordena palabras `u64`.

## Resultados (release, 4 workers)

| | Antes | Después | RSS | SHA-256 de la salida |
| --- | --- | --- | --- | --- |
| Belvedere 64 k / g = 256 (mediana de 3) | 53,0 s | **51,4 s** | ~110 MB | igual |
| Belvedere 262 k / g = 8 (1) | 32,0 s | 29,7 s | 92 MB | igual |
| Majadas 64 k / g = 256 (1) | 366,9 s | **354,8 s** | 207 MB | igual |

Frente al conversor anterior (262 k / g = 8 antes de estas optimizaciones):
- Majadas: 354,8 / 287 s = **+24 %**.
- Belvedere: 51,4 s frente a 37,7–42,4 s medidos ese día = **+21 % a +36 %**, según la ejecución de referencia.

## Límites

- Ruido de ±2 s entre ejecuciones; la ganancia de esta ronda es de ~3 %. La primera ronda (380 s) estaba inflada por carga del equipo: la base de hoy daba 366,9 s.
- Con 4 workers la jerarquía no puede pasar de 4 hilos. Ganar más exige acumuladores más compactos (~170 B por celda).
- Sin `perf` en el equipo; perfil por muestreo con `eu-stack`.
