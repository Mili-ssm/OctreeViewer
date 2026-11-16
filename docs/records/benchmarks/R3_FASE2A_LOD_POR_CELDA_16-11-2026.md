# R3 fase 2a: selección por celda en la GPU — 16-11-2026

**Estado: borrador del implementador; la sección «Navegador» la rellena el controlador.** La puerta de partición de la GPU pasa (0 huecos y 0 dobles en dos adaptadores), los tests de extremo a extremo con el controlador llegan a D5 sin errores y el modo `cell` dibuja igual o menos instancias que `octant` en los casos nativos medidos (hasta −39 % en la órbita a 1 px de Belvedere). El probe de navegador da el mismo resumen del corte en ambos modos en Belvedere; no compara píxeles.

Plan: [fase 2a](../../plans/fase-2a-lod-por-celda-en-gpu.md). Métrica: [R3_FASE2A_METRICA_16-11-2026.md](R3_FASE2A_METRICA_16-11-2026.md). Regla y opción: [REGIONAL_VOXEL_LOD](../../explanation/REGIONAL_VOXEL_LOD.md#selección-por-celda-en-la-gpu-fase-2a-16-11-2026).

## Identidad

- Rama `feat/cell-lod`, partiendo del commit `2f9852d`; las medidas nativas y del probe incluyen el cambio sin confirmar de esta tarea (bench y probe).
- Perfil `perf` para el bench nativo; navegador Chrome 157.0.8103.78 headless, WebGPU sobre Vulkan.
- Adaptadores: AMD Radeon 880M (RADV STRIX1, `rdna-3`) en bench y navegador; la puerta de partición (tarea 7b) corrió además en llvmpipe.
- Datos: `belvedere-v7.oct` (3.941 nodos, 530.962.031 B) y `majadas-v7.oct`; casos exportados por `coverage_spike --export-pool` el 06-11-2026. Salidas en [`data/r3-fase2a-16-11-2026/`](data/r3-fase2a-16-11-2026/).

## Hechos

### Puerta de partición de la GPU (tarea 7b)

Cada celda del conjunto de muestras cae exactamente en un nivel dibujado.

| Medida | Resultado |
| --- | --- |
| Huecos / dobles | 0 / 0 |
| Adaptadores | RADV y llvmpipe |
| Poses | 200 y 1.000 |
| Puntos | 11,6 M |
| Casos de umbral | 3.744 |
| Mutación: omitir el ancestro padre | 439.979 dobles |
| Mutación: quitar la guarda del nivel más fino | 244.278 huecos |

Las mutaciones comprueban que la prueba detecta los fallos que la regla E4 evita.

### Tests de extremo a extremo con el controlador (tarea 9)

Padre e hijos, 24 poses y 30 fotogramas: se alcanza D5, 0 huecos y 0 dobles.

### Defectos del modo `cell` hallados por las simulaciones en CPU (corregidos)

1. El ajuste memorizado conservaba claves D5 de ancestros olvidados: pánico.
2. Un nivel se activaba el fotograma en que terminaba su subida: se leía un slot mientras se escribía.
3. La retención de R3-Q21 comparaba con la profundidad nominal del octante y no con la más fina activa del ancestro: engrosamiento.

### Métrica (tarea 2)

`edge` pasa los criterios de parada; `bound` no. Detalle en el [registro de la métrica](R3_FASE2A_METRICA_16-11-2026.md).

### Contadores por profundidad en la GPU (bench nativo)

`cell_pool_bench --counters [--cell-lod]` converge la pose quieta, dibuja una vez con `set_counters(true)` y vuelca `read_lod_counters` más las instancias del pase simple (1280×720, RADV, `perf`). Solo estaban exportados los objetivos de 1 y 4 px (no 2 ni 8 px). Los niveles son los almacenados en el fichero (el cargador falso no sirve niveles a demanda). Todos los casos convergieron en ambos modos. `rel_origin` de las páginas se reconstruye en el bench porque el fixture es anterior al campo.

| Caso | px | Instancias `octant` | Instancias `cell` | `cell`/`octant` |
| --- | ---: | ---: | ---: | ---: |
| belvedere-grazing | 1 | 172.802 | 172.802 | 1,00 |
| belvedere-grazing | 4 | 10.636 | 8.208 | 0,77 |
| belvedere-inside-leaf | 1 | 3.550.817 | 3.550.817 | 1,00 |
| belvedere-inside-leaf | 4 | 429.054 | 420.314 | 0,98 |
| belvedere-orbit-0 | 1 | 73.999 | 47.825 | 0,65 |
| belvedere-orbit-0 | 4 | 4.796 | 2.921 | 0,61 |
| majadas-grazing | 1 | 54.495 | 54.495 | 1,00 |
| majadas-grazing | 4 | 3.883 | 3.872 | 1,00 |
| majadas-inside-leaf | 1 | 8.029.044 | 8.029.044 | 1,00 |
| majadas-inside-leaf | 4 | 1.841.335 | 1.672.890 | 0,91 |
| majadas-orbit-0 | 1 | 54.495 | 54.495 | 1,00 |
| majadas-orbit-0 | 4 | 3.921 | 3.921 | 1,00 |

Los recuentos por profundidad (`counter_per_depth`) están en `cell-pool-counters-{octant,cell}.json`. Ejemplos a 4 px: Belvedere órbita 0 pasa de `{7: 2.296, 8: 2.500}` a `{7: 2.921}`; Majadas dentro de una hoja mueve celdas entre las profundidades 12–18 sin tocar 19–21 y 26. Las instancias de `cell` en Belvedere órbita 0 a 4 px (2.921) coinciden con las de `edge` del [registro de la métrica](R3_FASE2A_METRICA_16-11-2026.md); a 1 px, con las 47.825 de `edge` en la misma pose. No se midieron tiempos de GPU en esta tarea.

### Probe de navegador (`probe-cells-parity`, Belvedere)

`node examples/web-host/probe-cells-parity.mjs belvedere-v7.oct OUT.json --cell-lod=octant|cell --rest-timeout-s=120`, servidor en el puerto 8791, cuatro brazos y dos poses.

| Modo | Pose | Resumen del corte en reposo (`cutDigestAtRest`) | Reposo (ms) | Paridad entre dos cargas | Puerta sin build worker |
| --- | --- | --- | ---: | --- | --- |
| `octant` | fitted | `e67592d26da97ab3` | 2.243 | igual | `needsBuildWorker`, ok |
| `octant` | orbit-step | `2058d40e998e328d` | 2.019 | igual | `needsBuildWorker`, ok |
| `cell` | fitted | `e67592d26da97ab3` | 2.225 | igual | `needsBuildWorker`, ok |
| `cell` | orbit-step | `2058d40e998e328d` | 2.017 | igual | `needsBuildWorker`, ok |

- Consola sin errores ni avisos en ningún brazo. El p95 del intervalo entre respuestas de fotograma de la pose `fitted` con build worker mientras llegan nodos fue 25 ms en `octant` y 24–61 ms en `cell` (una ejecución de 61 ms, otra de 24 ms); no son tiempos comparables (`functional`, sin aislar la máquina).
- Los dos modos dan el mismo resumen del corte; `cutEpochSent` difiere (24 frente a 23 en `orbit-step`), así que el modo llegó a cambiar el plan. El resumen no incluye el nivel dibujado por celda.
- `fs_e430_n4008_18.oct` es V5: el visor lo abre, pero con la ruta CPU (sin `cellPath`, 1 nodo), así que el probe no converge en el modo celdas; no se ejecutó. El sustituto es Belvedere V7.
- La muestra integrada de la demo no se ejecutó: el probe recibe un fichero. Majadas (2 GB) no se probó en navegador.
- Corrección de paso: el resumen final `gate`/`parity` del probe no se escribía nunca (buscaba `arm.name` en lugar de `arm.arm`); arreglado.

## Inferencias

- *Inferido:* con el mismo resumen del corte en los dos modos, la diferencia visible de `cell` está en el pase de slots, que el probe no captura; hace falta comparar píxeles (sección siguiente).
- *Inferido:* `cell` no dibuja más que `octant` en estos casos porque E6 elige menos niveles por nodo y E4 no dibuja la celda gruesa donde el hijo ya la sustituye; los 12 casos medidos no demuestran que valga siempre.
- *Inferido:* los casos con razón 1,00 a 1 px (rasante, dentro de una hoja) dibujan lo mismo porque a esa resolución casi todo se refina hasta el nivel almacenado más fino.

## Navegador

*Pendiente: el controlador rellena esta sección con los resultados del verificador de navegador (capturas en el mismo punto de zoom en `octant` y `cell`, nodos con ≥ 2 profundidades dibujadas, tiempo del pase de slots con consultas de marca de tiempo, Chrome WebGPU).*

## Límites

- Las medidas de bench son de una sola ejecución, sin repetición ni varianza; solo valen para los casos exportados, RADV y el perfil `perf`.
- Los conjuntos activos de la 2a pueden saltarse niveles hasta la fase 2b ([ADR 0053](../../adr/0053-transiciones-de-un-nivel.md)).
- Falta `scripts/check.sh` con la entrada real (a cargo del controlador).
