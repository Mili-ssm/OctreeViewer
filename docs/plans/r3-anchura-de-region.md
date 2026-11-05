# R3: experimento de anchura de región, layout, máscaras y flags

Estado: completado · 05-11-2026 · decidido en [ADR 0032](../adr/0032-hojas-adaptativas-e-instancia-de-12-b.md); código en `experiment/r3-region-widths`

## Objetivo

Elegir, con datos del dataset completo, la anchura de página `W` (bits por eje) que **equilibra**
memoria de GPU, memoria de CPU y cálculo (criterio del propietario, 05-11-2026). Candidatas: 8, 10,
12, 16, 21, 24 y 32. El [ADR 0031](../adr/0031-celdas-por-nodo-en-v7-5.md) fija hoy 10 bits e
instancia de 16 B; el [ADR 0027](../adr/0027-celdas-por-nodo-en-pool-paginado.md) sigue fijando
páginas de 8 KiB. 32 bits equivale a «clave más ancha» sin subregiones.

El resultado entra en un ADR nuevo junto con la **continuación adaptativa de las hojas** (decisión del
propietario, 05-11-2026: la cadena sigue hasta separar las posiciones finitas distintas). Ese ADR
también resuelve que hoy se superan los 21 bits relativos (Belvedere llega a 22) sin error:
`RelativeDepthExceeded` está declarado pero no se emite (`node_cells.rs:93`, `:270-275`).

## Base y decisiones tomadas

| Decisión | Motivo |
| --- | --- |
| Base: `3eb78f7` + copia exacta del diff de crates de `experiment/r3-layout-narrow` (SHA-256 del parche `35a37a5a…`, el medido en `R3_LAYOUT_COMPACT_05-11-2026`) | Elección del propietario; los worktrees de Codex no se tocan |
| Celdas: `PointNodeCellSource::default()` sin cambios | Se mide el layout, no el constructor |
| Payload fijo de instancia: rgba8 + intensidad `u16` + clase `i16` = 8 B | ADR 0027/0031; aísla el efecto de `W` |
| Flags (`single_point`, `has_intensity`, `has_class`) con dos ubicaciones: por página (0 bits por instancia) o por instancia (3 bits) | Los atributos se buscan por bloque (`node_cells.rs:242-243`): pueden ir en la página |
| `PageEntry` nominal de 32 B | Supuesto declarado; se revisa al diseñar el struct WGSL |
| La memoria de CPU retenida (`NodeCell`, 80 B) no depende de `W`; solo dependen el staging codificado y el directorio de páginas | Se informa, pero no se cuenta dos veces junto con la GPU |

## Análisis previo (antes de medir)

Instancia = palabras de 32 bits para la posición (3W bits, más 3 si los flags van por instancia) + 8 B.

| W | Flags por página | Flags por instancia |
| --- | --- | --- |
| 8 | 12 B | 12 B |
| 10 | 12 B | 16 B |
| 12, 16 | 16 B | 16 B |
| 21, 24 | 20 B | 20 B |
| 32 | 20 B | 24 B |

- Con los flags por página, 12 y 16 quedan dominados por 10 (misma instancia o mayor, más páginas), y 21 queda dominado por 24.
- Los niveles internos caben en una página de cualquier `W` ≥ 8: las diferencias vienen de las hojas.
- Hipótesis: el resultado lo decide la clase de palabra de la instancia salvo que el sobrecoste de las páginas (cola vacía + `PageEntry`) supere el 1 %.

## Tarea 1: spike `region_width_spike`

**Hecha (05-11-2026):** [resultado](../records/benchmarks/R3_REGION_LAYOUT_05-11-2026.md). Gana W = 10 con flags por página (instancia de 12 B).

- [x] Ejemplo `crates/viewer_render_wgpu/examples/region_width_spike.rs`, que recorre **todos** los nodos y niveles guardados de un `.oct` v7.5.
- [x] Dos modelos de página, ambos con capacidad `8192 / instancia`:
  - (a) bloques alineados de 2^W relativos al origen del nodo;
  - (b) reparto voraz en orden Morton: página nueva si el rango supera 2^W en algún eje o se llena.
- [x] Por `W`, ubicación de flags, modelo y clase de nivel (interno, hoja no final, hoja final):
  - niveles, celdas, páginas y llenado medio;
  - bytes de instancias, cola vacía y `PageEntry`, y bytes por celda;
  - histograma de celdas por página (cubetas log2).
- [x] Indicadores de residencia: nivel guardado más grueso por nodo (vista lejana) y más fino (vista cercana).
- [x] Histograma de bits relativos máximos por clase; hay alarma si se superan 24.
- [x] Hipótesis fuera de la puntuación: ocupación de una máscara de 4³ (64 bits) por página.
- [x] Tiempo de codificación: 3 repeticiones por nivel en orden rotado, sumadas por `W` y repetición; se informa la dispersión y la parte que supone frente a la construcción.
- [x] Comprobaciones (un fallo aborta):
  - decodificador independiente solo con `u32`, como en WGSL, que recupera coordenada y flags exactos;
  - cada celda está en una sola página;
  - páginas contiguas en orden Morton;
  - `local < 2^W`;
  - `f32(local) + 0,5` a menos de 1/8 de celda del valor exacto.
- [x] Antes, con un límite de nodos sobre Belvedere; después, los datasets completos.
- [x] **Pasa si** completa Belvedere y Majadas con SHA de datasets iguales a los de `R3_LAYOUT_COMPACT_05-11-2026` y el mismo parche de base; la salida queda en `R3_REGION_LAYOUT_dd-mm-aaaa.md`. Los totales de ese registro (232.396.472 y 1.384.873.569) son de la cadena **sin podar** (`node_cells_bench.rs:153`, `min_growth: 1.0`) y no se comparan con los del spike, que usa la poda de producción.
- Los flags por página solo son válidos si cada página es homogénea: `has_intensity` y `has_class` dependen del esquema de atributos del bloque (*inferido*: homogéneos por dataset); `single_point` es por celda y por página no cabe, así que esa variante supone que la GPU no lo necesita.

## Tarea 2: coste de construcción por `W`

- [ ] ~~Ampliar el laboratorio de `R3_REGION_WIDTH_05-11-2026` con 24 y 32 sobre los mismos 17 nodos. Solo hace falta si la Tarea 1 no deja la decisión en el tamaño de palabra.~~ Descartada: la codificación es el 1,4–2,5 % de la construcción y la memoria decide.

## Tarea 3: finalistas

- [x] Puntuación fijada antes de medir: media de bytes de GPU (modelo b, vista cercana y vista lejana) y de cálculo (construcción + codificación), normalizada frente a W = 10 con flags por página.
- [x] Una diferencia menor que la dispersión es empate; el desempate prefiere W = 10 y después menos palabras en WGSL.
- [x] Dos finalistas, con el visto bueno del propietario antes de medir GPU o navegador: W = 10 con flags por página y W = 16 como control (05-11-2026); se miden en las Tareas 5 y 6 del plan de R3.

## Criterios de parada

- SHA de los datasets o parche de base distintos del registro previo → parar (el constructor o los datos han cambiado).
- Sobrecoste de páginas < 1 % para `W` ≥ 10 → decidir analíticamente por clase de palabra, sin más medidas.
- Una `W` que falla la comprobación `f32` → se descarta.
- Bits relativos > 24 → replantear antes de la Tarea 3.
- Majadas tarda más de 30 min o supera ~4 GB de RSS → muestrear el tiempo.

## Fuera de alcance

GPU real, navegador, cambios de formato en disco y cambios del constructor de celdas.
