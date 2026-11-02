# R4 T1: memoria del conversor con `sample_grid` = 256

Fecha: 02-11-2026. Estado: **RSS dentro del presupuesto; el tiempo sigue por encima de RQ-07**.

Sigue al [spike de densidad](R4_DENSITY_SPIKE_02-11-2026.md). El propietario fijó hoja 64 k y `sample_grid` = 256 y pidió bajar la RSS (02-11-2026).

## Causa

El heap vivo en Belvedere era de unos 150 MB y la RSS de 410 MB. glibc sube su umbral de `mmap` tras el primer `free` grande, y desde ese momento las rejillas y los buffers de LOD de los 20 workers se quedan en arenas fragmentadas que no devuelven páginas.

## Cambios

- `viewer_convert/src/allocator.rs`: `mallopt(M_MMAP_THRESHOLD, 128 KiB)` una vez al convertir; no hace nada fuera de Linux/glibc.
- La rejilla dispersa pasa de `BTreeMap` a un `Vec` compacto con índice de direccionamiento abierto y orden por clave.
- Las sumas de `SampleAccumulator` pasan de `u128` a `u64`: 144 B por celda en lugar de unos 216.

## Resultados (hoja 64 k, g = 256, una ejecución)

| | RSS antes → después | Tiempo | Bytes de salida |
| --- | --- | --- | --- |
| Belvedere | 401 → **155 MB** | 75 → 76 s | 2 125 683 799, iguales (SHA-256 iguales) |
| Majadas | 653 → **178 MB** | 499 → 506 s | 8 891 071 692, iguales |

Datos: [r4-rss-02-11-2026/](data/r4-rss-02-11-2026/).

## Límites

- El tiempo no mejora: Majadas tarda un 76 % más que el control (287 s) e incumple el +30 % de RQ-07. *Inferido*, sin perfilar: el sobrecoste vendría de los LOD internos, mucho mayores (unos 6 GB más de salida).
- El hash de Majadas no se compara antes y después; solo el tamaño.
