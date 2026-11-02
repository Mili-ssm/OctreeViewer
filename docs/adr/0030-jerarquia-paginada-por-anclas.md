# 0030 Jerarquía paginada por niveles desde anclas de prefijo

Estado: Aceptada · Fecha: 02-11-2026 · Decide: propietario · Depende de [ADR 0029](0029-octree-global-por-bits-de-enteros.md)

## Contexto
- Hoy el índice se carga entero. Mide ~560–610 B por nodo: 13,5 MB en Majadas (24 091 nodos) y 2,4 MB en Belvedere ([registro T6](../records/benchmarks/R4_T6_MEASUREMENT_02-11-2026.md)). Abrir cuesta 17–35 µs por nodo.
- Extrapolación lineal (*inferida*, ramificación ~4):
  - RQ-04 (primer frame remoto) falla hacia 70 GB y RQ-03 (local) hacia 90 GB.
  - El límite del lector de 256 MiB (`index_codec.rs:384`) se alcanza hacia 150 GB.
  - La memoria wasm32 revienta antes de 1 TB: el índice está en memoria al menos tres veces (bytes en bruto, estructuras validadas y descriptor JS por nodo).
- Propuestas del propietario: OQ-05, OQ-06 y OQ-07 ([requisitos](../requirements.md#preguntas-abiertas)).

## Decisión
- **Corte por niveles del octree, no por número de hijos ni por bytes.** Cada página cubre *k* niveles bajo su ancla, con *k* entre 1 y 5 guardado en la cabecera de la página (`u8`). El conversor elige el mayor *k* ≤ 5 que deja la página en 512 KB o menos (propietario, 02-11-2026, tras el [spike P0](../records/benchmarks/R5_P0_PAGES_02-11-2026.md)). Las páginas se encuentran bajando por los punteros de las páginas padre, sin contar nodos.
- **Cada página tiene un ancla:** su cabecera guarda el nivel (`u8`) y el prefijo completo (3 × `u64`), más un CRC32 de la página.
- **Nodos de la página:**
  - Se identifican por (profundidad relativa `u8`, camino `u16`), con 5 niveles × 3 bits de octante (OQ-06), en orden determinista.
  - Llevan sus bloques en línea.
  - Los nodos de profundidad *k* apuntan a su página hija (offset `u64`, longitud `u32`).
- **El registro completo del ancla vive en la página padre.** Así el ancla se puede seleccionar y dibujar antes de pedir su página.
- **Lo derivable no se guarda:**
  - límites en `f64`;
  - offsets de columnas contiguas;
  - longitudes en bruto;
  - espaciado de los nodos internos;
  - ids de nodo y de padre.
- **Apertura:** el índice es un bloque de metadatos más la página raíz, junto al pie. Las páginas hijas se escriben en anchura tras los datos, de modo que una sola petición de rango trae varias hermanas.
- **Validación:** al abrir, cada página se valida sola (CRC, claves, rangos dentro de la sección de datos, sin solapes dentro de la página); una página inválida se rechaza. Las comprobaciones globales (sin huecos entre bloques, propiedad de bloques) pasan a `viewer_convert verify` (propietario, 02-11-2026; precisa ADR 0023).
- **Lector:** pide las páginas bajo demanda, con la prioridad de error proyectado de los bloques. Las guarda en una caché LRU, con las páginas que tienen nodos seleccionados fijadas.
- **Selección de LOD:** un ancla sin página cargada cuenta como «hijos desconocidos».
- **Revisión `v7.5-pre`:** incluye también los flags LAS y los Extra Bytes (RF-24), decisión del propietario del 02-11-2026, para que haya una sola regeneración (ADR 0022).
- **OQ-05 (compresión de cadenas): se rechaza.** Majadas tiene 6 de 24 091 nodos con un solo hijo y Belvedere 9 de 3 941, con cadenas de 1 y 3 nodos como máximo: ahorraría menos de 6 KB. Además rompe la regla «hijo = padre + 1» y la profundidad fija de las anclas.
- **OQ-07 queda fuera:** se decide con R3. El origen `i64` del ancla es el candidato natural para las posiciones relativas en la GPU.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Páginas COPC (claves `i32` por eje, tamaño variable) | Hay que bajar la página hija antes de poder usar el nodo; las claves no son el prefijo global |
| Bloques de jerarquía de Potree 2 (paso 4, nodos proxy) | El proxy obliga a una petición previa y el orden condiciona la decodificación; ahorra < 1 % |
| EPT (JSON por subárbol) | Coste de parseo y tamaño |
| Páginas limitadas por bytes | No encaja con un camino `u16` fijo; más lógica en el escritor |
| Índice entero con registros reducidos | Solo lleva el límite hasta ~250 GB |

## Consecuencias
- El coste de apertura deja de crecer con el número total de nodos, y la lectura remota pasa a ser posible.
- `SpatialHierarchy` tiene que poder crecer.
- Desaparece el descriptor JS por nodo (antigua R1-T6).
- Las comprobaciones globales pasan al `verify` del CLI.
- Hay que reconvertir todos los datasets; `v7.4-pre` sigue siendo legible.

## Revisar si
Una página con *k* = 1 supera 1 MiB, más del 1 % de los nodos tienen un solo hijo, o hace falta una profundidad mayor de 21 (OQ-07).
