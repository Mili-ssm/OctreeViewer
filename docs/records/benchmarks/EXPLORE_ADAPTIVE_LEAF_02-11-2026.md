# Exploración: hojas en el tope de R3-T1 sobre `v7.4-pre`

Fecha: 02-11-2026. Rama: `explore/adaptive-leaf` (`7e00e3a`). Estado: **< 1 % en ambos datasets; pendiente del propietario**.

## Diagnóstico

- Partir hojas por densidad no ayuda: las hojas en el tope son pequeñas (Majadas: mediana 296 puntos, máximo 30 k; Belvedere: 76–26 k), ninguna cerca de 64 000.
- Causa 1 (bench): 219 de las 382 de Majadas y 4 de las 45 de Belvedere son hojas de 1–2 puntos; su espaciado es el cuanto, la profundidad se recorta a 24 (`MAX_CELL_DEPTH`) y cuentan como «en el tope» sin celdas que crecer.
- Causa 2 (conversor): `measure_leaf_spacing` paraba en la primera meseta (crecimiento < ×1,5 con más de una celda); dos o tres grupos separados (2, 3, 3 celdas) la cortaban pronto, con un espaciado demasiado grueso.

## Cambios

- Conversor: una meseta solo cuenta si el nivel anterior tiene al menos `min(8, puntos)` celdas ocupadas (`LEAF_PLATEAU_MIN_CELLS`). Cambia el valor de `spacing` de hoja en `v7.4-pre`; no el layout. Spec actualizada.
- Bench: no cuenta como «en el tope» una cadena cuyo último nivel ya tiene una celda por punto. Es el criterio escrito en [ADR 0028](../../adr/0028-r4-antes-que-r3.md) («con celdas aún creciendo»), que el bench no aplicaba.

## Resultados

| Variante | Belvedere | Majadas |
| --- | --- | --- |
| Base, bench anterior | 45 (1,42 %) | 382 (1,96 %) |
| Base, bench corregido | 36 (1,13 %) | 102 (0,52 %) |
| Mínimo 8 celdas, bench anterior | 5 (0,16 %) | 254 (1,30 %) |
| **Mínimo 8 celdas, bench corregido** | **2 (0,06 %)** | **28 (0,14 %)** |

Mínimos de 16 y 32 celdas fueron peores que 8 con el bench anterior. Disco, nodos y hojas idénticos en todas las variantes (+7,6 % y +2,1 % frente a V5). Conversión (`opt-level` 3, carga distinta entre rondas): Belvedere 42,7 → 41,0 s, Majadas 343 → 284 s; RSS 100 → 103 MB y 232 → 250 MB. Datos: [explore-adaptive-leaf-02-11-2026/](data/explore-adaptive-leaf-02-11-2026/).

## Límites

- Una conversión por variante; el tiempo no es concluyente.
- Sin cambios: bytes de cadena / bytes de puntos (1,81 × y 1,96 ×) y 564–578 hojas de Majadas con cadena > 4 × sus puntos (peor 6,74 ×), el otro criterio de parada de R3.
