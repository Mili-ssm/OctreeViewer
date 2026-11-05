# 0031 Celdas por nodo en v7.5: pirámide por nodo, coordenadas por página del pool y claves de 64 bits

Estado: Aceptada · enmendada por [0032](0032-hojas-adaptativas-e-instancia-de-12-b.md) (diseño; las cifras se confirman en las Tareas 4, 5 y 17 del [plan de R3](../plans/render-r3-cubos-en-gpu.md)) · Fecha: 03-11-2026 · Decide: propietario · Sustituye en parte a [ADR 0027](0027-celdas-por-nodo-en-pool-paginado.md)

## Contexto
- En `v7.5-pre` cada nodo interno muestrea una rejilla de 256³ (`options.rs:17`). En Belvedere eso da unas 67 000 celdas por nodo y unos 51 M en total, alrededor de 780 MiB con 16 B por celda: más que el pool de 256 MiB (pre-mortem del 03-11-2026; *inferido*).
- En el octree global ([ADR 0029](0029-octree-global-por-bits-de-enteros.md)), una nube con CRS que cruza un bit alto tiene la raíz en el nivel 0, y entonces hace falta una profundidad mayor de 24 (OQ-07). WGSL no tiene `u64`.
- La jerarquía es paginada ([ADR 0030](0030-jerarquia-paginada-por-anclas.md)):
  - los `NodeId` se repiten entre ficheros;
  - las páginas se expulsan;
  - un ancla sin cargar parece una hoja (`spatial.rs:723`).
- Tras la poda, la cadena de cada hoja queda en ~1 nivel. Con eso, el «mejor nivel residente» de 0027 vuelve a ser todo o nada.
- El error proyectado se calcula con la esquina más cercana (`camera.rs:414`). Por eso las celdas de 256³ salen mucho más finas de lo necesario en los nodos lejanos y en las poses rasantes.

## Decisión
- **Cada nodo es un conjunto disperso de niveles guardados.**
  - Nodo interno: su pirámide, de 8³ a 256³ (profundidades +3 a +8).
  - Hoja: su cadena.
  - Ambos se podan con `chain_keep_mask` (`CHAIN_MIN_GROWTH` = 1,5).
  - Para cada nodo se dibuja el nivel guardado más grueso cuyo error proyectado cumple el objetivo. Si una profundidad no está guardada, se usa la menor guardada que sea ≥ la deseada (RQ-09).
- **Claves:** coordenadas `u64` en la CPU, y Morton de 64 bits sobre coordenadas relativas al nodo, con un máximo de 21 bits por eje. Superarlo es un error, nunca un recorte.
- **Instancia de 16 B:** 10 bits por eje, relativos al origen de su **página del pool**.
  - Una página se parte, siguiendo el orden Morton, cuando su rango supera 2¹⁰ en algún eje.
  - Cada `PageEntry` lleva el origen relativo a la cámara (`i64` → f64 → f32, por frame).
  - Clase `i16`, con −1 si falta. Intensidad media con `intensity_count`.
- **Máscara espacial de 8³ por nodo**, no por página. El shader la evalúa con `coord_nodo >> (bits − 3)`, y se construye solo con descendientes que tienen un nivel residente. Un descendiente más de 3 niveles por debajo devuelve `DescendantTooFine`.
- **Residencia:** todo o nada por (nodo, nivel), con la clave (sesión, nodo, profundidad, generación de color).
  - Mientras falta el nivel deseado, **se dibuja el nivel residente más grueso como etapa**, y se cuenta en las métricas.
  - «Objetivo alcanzado» exige que ningún nodo quede dibujado con un nivel más grueso que el deseado.
- **Expulsión de páginas de jerarquía:** al expulsar una página se emite `evict` de sus nodos. Las páginas que contienen el corte confirmado por `poolAck` quedan fijadas.
- **Hojas:** se reconocen por el bit `has_children` de la página.
- **Continuidad entre nubes (RF-05):** el conversor escribe en los metadatos de `v7.5-pre` el espaciado de referencia de cada nube. Hay que regenerar los ficheros v7.5.
- **Ruta CPU:** Additive, v5 y `v7.1–3` siguen por la ruta CPU (`CellPathUnsupported`) hasta que se retiren V5/V6 ([ADR 0004](0004-retirar-v5-v6-tras-v7.md)).

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Solo 256³ por nodo interno | Desborda el pool, dibuja lo lejano con detalle de más y no tiene nivel de respaldo |
| Residencia parcial (algunas páginas de un nivel) | Deja huecos dentro del nodo (RQ-02) o vuelve a necesitar máscaras por página |
| Retener el corte anterior mientras falta un nivel | La vista se congela hasta completar |
| Pirámide escrita por el conversor | Cambio de formato; reabre R4 y ADR 0022 |
| Origen por nodo con 10 bits | Hay hojas que necesitan 14 bits por eje |
| Morton `u128` | Más memoria y ancho de banda; 21 bits relativos bastan hoy |

## Consecuencias
- Se reabre la Tarea 1: firma `&[PointBlock]`, tipos nuevos y dorados de `node_cells_digest`.
- La pirámide añade como mucho 3 × las celdas del nivel más fino, y ~1,33 × en superficies (*inferido*). Se construye en el worker.
- La ruta de celdas lee v7.4 y v7.5. El resto va por CPU.
- 0027 sigue vigente en lo que esta decisión no toca:
  - celdas por (nodo, profundidad) independientes de la cámara;
  - pool de páginas de 8 KiB, con presupuesto por frame y acuses;
  - `draw` por nodo;
  - HZB condicional.

## Revisar si
- La Tarea 4 mide que la pirámide ahorra menos del 25 % en instancias y bytes del corte frente a 256³ solo.
- `DescendantTooFine` aparece en más del 5 % de las actualizaciones de corte.
- Algún nodo necesita más de 21 bits relativos.
- Los bytes del corte superan 256 MiB.
