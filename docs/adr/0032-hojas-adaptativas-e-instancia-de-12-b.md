# 0032 Hojas hasta separar posiciones, clave regional más allá de 21 bits e instancia de 12 B

Estado: Aceptada · instancia enmendada por [0035](0035-instancia-de-16-b-con-w-16.md) (diseño; las cifras de GPU se confirman en las Tareas 5 y 6 del [plan de R3](../plans/render-r3-cubos-en-gpu.md)) · Fecha: 05-11-2026 · Decide: propietario · Enmienda a [ADR 0031](0031-celdas-por-nodo-en-v7-5.md) · Poda precisada por el [ADR 0046](0046-la-poda-de-niveles-no-limita-el-render.md)

## Contexto
- La Tarea 1 de R3 se detuvo por sus dos criterios de parada ([parada](../records/benchmarks/R3_T1_STOP_04-11-2026.md)):
  - en Majadas, 16 hojas tienen posiciones `f32` distintas en la misma celda al llegar a la profundidad fija de la cadena;
  - en Belvedere, 3 hojas (4 puntos) necesitan 22 bits relativos por eje ([rango de hojas](../records/benchmarks/R3_LEAF_SPAN_04-11-2026.md)), y 0031 fija 21 bits como error.
- Continuar la cadena solo en esas hojas las separa con profundidad relativa ≤ 18. Esas 16 hojas pasan de 17.373 a 18.553 celdas (+1.180) ([alternativas](../records/benchmarks/R3_T1_OPTIONS_04-11-2026.md)).
- 0031 fija una instancia de 16 B con 10 bits por eje relativos a la página. Con la posición en un `u32` (30 bits) y los flags en la página, la instancia cabe en 12 B. Un barrido de anchuras de 8 a 32 bits sobre Belvedere y Majadas completos ([layout por anchura](../records/benchmarks/R3_REGION_LAYOUT_05-11-2026.md)) da:

| Variante | Belvedere B/celda (cerca) | Majadas B/celda (cerca) |
| --- | ---: | ---: |
| **W = 10, flags en la página (12 B)** | **12,19** | **12,95** |
| W = 10, flags en la instancia (16 B, 0031) | 16,19 | 16,92 |
| W = 8 (12 B) | 12,95 | 24,59 |
| W = 16 (16 B) | 16,19 | 16,19 |

  La codificación cuesta el 1,4–2,5 % de la construcción en todas las anchuras: las diferencias de cálculo entre ellas son pequeñas frente a la construcción. El criterio «sobrecoste de páginas < 1 %» no se cumple con W = 10 en Majadas (7,0 % de bytes de página sin usar), pero aun así gana con margen.

## Decisión
- **Hojas:** la cadena empieza en la profundidad del espaciado y baja hasta que el número de celdas iguala el de posiciones finitas distintas (bits `f32`, con el cero con signo normalizado). Después se poda con `chain_keep_mask` (crecimiento mínimo 1,5).
  - Las posiciones idénticas se agregan en una celda.
  - Si se llega a la profundidad absoluta 63 sin separar, el error es `AbsoluteDepthExceeded`, nunca un recorte. Esto incluye puntos distintos fuera de los límites del nodo, que se ajustan al borde (`clamp` en `build_level_keyed` de `node_cells.rs`) y no llegan a separarse.
- **Claves de CPU:** Morton de 64 bits relativo al nodo mientras el rango quepa en 21 bits por eje. Si no cabe, la clave es (prefijo de región `u128` con los bits ≥ 21, Morton local `u64`), que conserva el orden Morton completo. Superar 21 bits deja de ser error; el límite pasa a ser la profundidad absoluta.
- **Instancia de 12 B:**
  - Posición: un `u32` con 10 bits por eje relativos al origen de la página; quedan 2 bits libres.
  - Payload: rgba8 (4 B), intensidad media `u16` (2 B) y clase `i16` (2 B, −1 si falta).
  - En WGSL se lee como `array<u32>` con zancada de 3 palabras.
- **Páginas:** 8 KiB, con capacidad para 682 instancias. Se reparten de forma voraz en orden Morton: se abre una página nueva cuando el rango supera 2¹⁰ en algún eje, la página se llena o cambian los flags. `PageEntry` (32 B nominales) lleva, además de lo que fija 0031, los flags `has_intensity` y `has_class`, iguales para todas sus celdas.

**Qué sustituye de 0031:** «máximo de 21 bits por eje; superarlo es un error» (Decisión, Claves), la «instancia de 16 B», el rechazo de «Morton `u128`» (Alternativas: ahora solo se usa como prefijo de región en los nodos que lo necesitan) y el disparador «algún nodo necesita más de 21 bits» (Revisar si). El resto de 0031 sigue vigente: pirámide por nodo, máscara de 8³ por nodo, residencia todo o nada y etapas.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Límite fijo de la cadena | Deja fusionadas posiciones distintas (16 hojas en Majadas) |
| Subir el límite fijo | No garantiza la separación y añade celdas a todas las hojas |
| Instancia de 16 B con flags por instancia (0031) | +31–33 % de bytes de GPU frente a 12 B, sin ganar cálculo |
| W = 8 | Depende del dataset: en Majadas deja sin usar el 51 % de los bytes de página |
| W = 12, 16 o 21 | 16 B por instancia y el mismo cálculo; W = 16 queda como control en la Tarea 5 |
| W = 24 o 32 (clave ancha sin regiones) | 20–24 B por instancia |
| Subregiones en la clave de CPU | Mismos conteos con más tiempo en Majadas: 365,06–634,75 s frente a 307,79 s ([prueba](../records/benchmarks/R3_SUBREGIONS_04-11-2026.md), una ejecución por variante) |

## Consecuencias
- La Tarea 1 continúa con el código de `experiment/r3-layout-narrow` (continuación adaptativa, clave regional y registro compacto de 48 B cuando el nodo cabe en 21 bits). Sus dorados de `node_cells_digest` cambian.
- `RelativeDepthExceeded` desaparece o se queda sin uso; `AbsoluteDepthExceeded` cubre el límite.
- La Tarea 5 mide la instancia de 12 B frente al control de 16 B (W = 16). La Tarea 6 comprueba la decodificación bit a bit de la instancia de 12 B en Rust y en WGSL.
- `single_point` no viaja a la GPU. Si un modo de color lo necesitara, irá en los 2 bits libres de la posición.

## Revisar si
- La Tarea 5 mide que la zancada de 12 B cuesta más del 10 % de tiempo de GPU frente a 16 B.
- Partir páginas por cambio de flags añade más del 1 % de páginas.
- Un dataset nuevo añade más del 10 % de celdas con la continuación adaptativa frente al límite fijo.
- Algún dataset deja sin usar más del 20 % de los bytes de página con W = 10. Con un 26 %, la instancia de 12 B igualaría a la de 16 B.
