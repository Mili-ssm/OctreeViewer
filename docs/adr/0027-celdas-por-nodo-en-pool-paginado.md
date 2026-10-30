# 0027 Celdas por nodo en un pool de GPU paginado con máscaras de cobertura

Estado: Propuesta (se acepta al cerrar las Tareas 4, 5 y 17 del [plan de R3](../explanation/render-r3-cubos-en-gpu.md)) · Fecha: 30-10-2026

## Contexto
- [ADR 0019](0019-cubos-por-nodo-residentes-en-gpu.md) exige cubos por nodo, independientes de la vista y
  residentes en GPU. R3 va antes que R4: hay que trabajar con el formato actual, con 8³ representantes
  por nodo interno y hojas de hasta 262 144 puntos.
- Hoy cada punto desciende desde la raíz hasta la primera profundidad cuya arista proyectada cabe en el
  objetivo (`voxel_grid.rs:543-631`): el tamaño de celda depende de la cámara y se recalcula el corte
  entero en cada vista (B1). La instancia ocupa 40 B (`scene.rs:8-16`).
- El dispositivo se pide con `downlevel_defaults` (`gpu.rs:326`): 128 MiB por binding de almacenamiento.
- Equipo de referencia: iGPU con ~2 GB compartidos; PSS ≤ 8 GiB (RQ-05).

## Decisión
Decisión técnica del coordinador:
- **Celdas por (nodo, profundidad)** en la rejilla diádica anclada en la raíz: el nodo interno, a su
  profundidad de representantes, derivada del lado del nodo (`representative_depth`, válida con
  divisiones 2, 4 y 8); la hoja, una cadena desde esa profundidad hasta que el número de celdas deja de
  crecer o +7 niveles (10 bits por eje). Cada punto cae en una sola celda de su nodo (cara máxima
  inclusiva). Se construyen una vez por nodo y generación de color en el worker de construcción.
- **Selección LOD sin cambios** (`select_lod_stable_projector`); un corte puro elige la profundidad de
  cada hoja y las máscaras de cobertura.
- **Pool paginado:** instancias de 16 B (coordenada relativa al nodo con 10 bits por eje, color sRGB
  rgba8, intensidad, clase y banderas) en páginas de 512 (8 KiB), con una **máscara de 512 bits** por
  página que oculta las celdas del padre bajo descendientes seleccionados; un descendiente más fino que
  las celdas del padre se rechaza y se retiene el corte anterior, como hoy `MixedCutNotAligned`.
- Tabla de páginas con el origen de cada nodo relativo a la cámara, reescrito por frame (f64 → f32).
- El worker de construcción propone la residencia (LRU por página; las del corte, fijadas) y el de
  render la confirma con acuses (fuente de verdad), sube con un presupuesto por frame, dibuja el mejor
  nivel residente de cada hoja y usa un `draw` por nodo. Pool con techo de 256 MiB. La compactación con
  HZB solo entra si el spike de GPU lo exige.
- Additive y el color por atributo seleccionado mantienen la paridad: Additive sigue en la ruta CPU
  hasta R4, y el color por atributo reconstruye las celdas.

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Hoja solo a su profundidad más fina | Hasta 262 144 instancias por hoja; no cabe en el presupuesto. |
| Hoja a una sola profundidad (nivel+3) | Celdas por encima del objetivo sin que lo imponga la fuente (incumple RQ-09). |
| Buffers por nodo (patrón `block_buffers`) | Rotación de buffers y sin un único binding para HZB. |
| Reconstruir las páginas del padre en cada corte | Vuelve a subir datos durante la órbita. |
| Orden de dibujo o sesgo de profundidad | Los cubos del padre asoman en los huecos del hijo; rompe el contrato de celdas. |
| Colorear por atributo en el shader | Cambia los resultados frente a la ruta actual; pierde la paridad bit a bit. |

## Consecuencias
- Los 48 digests dorados y la ruta CPU quedan como oráculo de tests y como ruta Additive; la ruta nueva
  tiene sus propios dorados (celdas por nodo y corte) y una relación de refinamiento exacta con la CPU.
- Las celdas lejanas pueden salir más finas que hoy (más instancias). El spike de cobertura decide si
  basta o si el refinado por octante entra en R3.
- R4 solo sustituye la fuente de celdas (`NodeCellSource`) y el ancla de la rejilla
  ([ADR 0021](0021-rejilla-global-determinista-y-crs.md)); instancia, páginas y máscaras se mantienen.
- El dispositivo pedirá `min(adapter.limits, 256 MiB)`, con respaldo a `downlevel_defaults` y el pool
  limitado a 128 MiB.

## Revisar si
- El spike de GPU da más de 16,7 ms con 0,5 M instancias (≈ 4 px a 1080p) en el equipo de referencia.
- El spike de cobertura muestra más píxeles vacíos que la ruta CPU y el refinado por octante no lo
  corrige.
- Algún nodo real no es cúbico y diádico, o la memoria GPU/GTT supera 1 GiB en el equipo de referencia.

## Evidencia
[Plan de R3](../explanation/render-r3-cubos-en-gpu.md) · [ADR 0019](0019-cubos-por-nodo-residentes-en-gpu.md) ·
[Contrato de celdas](../explanation/REGIONAL_VOXEL_LOD.md) · [Requisitos](../requirements.md)
