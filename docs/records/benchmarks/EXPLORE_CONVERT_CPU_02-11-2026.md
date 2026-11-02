# Exploración: CPU por nivel del conversor

Fecha: 02-11-2026. Rama: `explore/convert-cpu`. Estado: **una mejora mantenida (salida idéntica); RQ-07 sigue sin cumplirse; pendiente del propietario**.

## Cambio mantenido

Los nodos copian el registro del spool como bytes al hijo y solo decodifican la posición (`decode_position`); cada celda de muestreo guarda una ranura `u32` en un arena (`NearestRecords`) en lugar de un `SpoolPoint` completo. El acumulador baja de ~250 B a ~72 B. Afecta a V5 y a `v7.x`.

## Resultados (`opt-level` 3, salida SHA-256 idéntica antes y después)

| | Antes | Después |
| --- | --- | --- |
| Belvedere V5 (mediana de 3) | 24,5 s, 66 MB | 20,9 s, 69 MB |
| Belvedere v7.5 (mediana de 3) | 46,7 s, 133 MB | 41,7 s, 89 MB |
| Majadas V5 (1) | 255 s | 167 s |
| Majadas v7.5 (1) | 329 s, 281 MB | 354 s, 222 MB |

Carga 5–8 durante Majadas: sus tiempos no son comparables. Belvedere v7.5/V5 queda en ~1,9–2,0 (RQ-07 pide ≤ 1,3).

## Sin probar

Rejilla dispersa por ordenación, espaciado de hoja reutilizando Morton, fase de bloques sin `i128`, y `GlobalCellFrame::cell` recuantiza una posición ya cuantizada.
