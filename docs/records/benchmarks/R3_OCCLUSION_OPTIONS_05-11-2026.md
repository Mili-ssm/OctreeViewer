# R3: opciones para no dibujar celdas ocultas — 05-11-2026

**Evaluación, sin decisión.** El propietario propuso explorar el raymarching, o algo parecido, para no gastar trabajo en celdas tapadas por otras (05-11-2026). Hoy solo existe un HZB conservador para la ruta CPU, y únicamente con la cámara quieta. La ruta de celdas de R3 aún no tiene oclusión: la Tarea 16 es condicional y no tiene cifras.

**Propuesta:** decidir después de la Tarea 5, con dos datos que todavía no existen:
- la sobrecarga de fragmentos (*overdraw*);
- la fracción de instancias completamente ocultas en las poses de la Tarea 4.

## Estado en el repositorio

| Pieza | Estado |
| --- | --- |
| `hzb.rs` | Tres pases de cómputo:<br>- copia de profundidad a una pirámide R32F con máximo 2×2;<br>- `cull_instances` (`hzb.rs:890`), que proyecta las 8 esquinas de cada vóxel y lee como mucho 4×4 texeles;<br>- compactación con contador atómico y `draw_indexed_indirect`.<br>Lee la instancia antigua de 40 B, no la de 12 B (ADR 0032) |
| Uso | Activo por defecto en `upload_scene`/`upload_voxel_cut` (`gpu.rs:618`), solo con escena opaca, ≥ 4.096 instancias y presupuesto suficiente (`gpu.rs:631-640`). Se usa solo si el frame siguiente tiene la misma vista: cualquier movimiento lo invalida (`gpu.rs:1174`) |
| Evidencia | Recuentos de «vóxeles visibles por HZB» y bytes ([WEB_PERFORMANCE_BENCH](WEB_PERFORMANCE_BENCH.md)). Nunca se midió el ahorro de tiempo de GPU ([historia](../archive/DEVELOPMENT_HISTORY.md)) |
| Plan R3 | Tarea 16, condicional a la Tarea 5 o a RQ-01: generalizar `cull_instances` a un pase de compactación (corte, máscara, frustum y HZB) sobre el pool |
| Raymarching | No aparece en el código ni en la documentación. El ADR 0019 eligió cubos rasterizados y rechazó la rasterización por cómputo a 1 px |

## Opciones

| Opción | Cómo evita el trabajo oculto | Encaje con R3 | Coste y riesgo |
| --- | --- | --- | --- |
| **A. HZB en dos pasadas, por página o por nodo** (Tarea 16) | Dibuja lo visible del frame anterior, reconstruye la pirámide y prueba el resto contra ella antes de emitir argumentos indirectos | Reutiliza `hzb.rs` y las páginas de 8 KiB; no cambia el formato, pero hay que portar la lectura de `cull_instances` a la instancia de 12 B | Varios pases de cómputo con sincronía (cuello de botella a 1280×720 según Kitware). La oclusión sola puede ir **más lenta** que no recortar; compensa junto con el frustum. *Inferido*: apenas ayuda en vistas aéreas, con poca oclusión. WebGPU no tiene *multi-draw indirect* en el núcleo |
| **B. Raymarching por píxel** en cómputo | Cada rayo se detiene en la primera celda opaca: el coste crece con *píxeles × pasos*, no con las celdas | Hace falta una estructura recorrible en GPU: árbol de nodos, la máscara 8³ por nodo y ocupación por ladrillos o mapa de bits en el nivel dibujado. Las páginas actuales son listas ordenadas por Morton, no rejillas | Reescribe el render y obliga a un ADR que sustituya al 0019. No se encontró ningún raymarcher de vóxeles en WebGPU con tiempos publicados. Riesgo de divergencia SIMD con rayos incoherentes. A favor: dentro de una hoja de Majadas a 1 px hay 20,4 M celdas en pantalla ([cobertura T4](R3_T4_COVERAGE_05-11-2026.md)) frente a 0,92 M píxeles a 1280×720 |
| **C. Híbrido** | Rasteriza las cajas de nodo o página y hace marcha dentro de ellas (máscara 8³ y después ladrillos de 8³) | Aprovecha la máscara 8³ por nodo, que ya existe | Las dos complejidades a la vez; sin evidencia publicada del punto de cruce |
| **D. Rasterización de puntos por cómputo** (Schütz y col., 2021) | No evita la oclusión, pero quita el coste fijo por primitiva y resuelve la profundidad con atómicos | Choca con la decisión de cubos 3D (P1, ADR 0019) | Hasta un orden de magnitud frente al pipeline de puntos en escritorio (RTX 3090); no hay cifras en WebGPU |

## Qué medir antes de decidir

1. En la Tarea 5 (GPU): tiempo de frame, fragmentos sometidos a la prueba de profundidad por píxel (*overdraw*) y su reparto por pose.
2. En el spike de cobertura (CPU, barato): con un Z-buffer de baja resolución, la fracción de celdas en pantalla tapadas por completo en cada pose de la Tarea 4. Si es baja (vistas aéreas), A y B ganan poco.
3. Para B: un prototipo acotado de un nodo y una página, en WGSL, con tiempo por píxel en la 880M. Solo si 1 y 2 muestran *overdraw* alto y la Tarea 5 supera su presupuesto de 8 ms.

## Fuentes

| Tema | Fuente | Confianza |
| --- | --- | --- |
| HZB en dos pasadas | Haar y Aaltonen, «GPU-Driven Rendering Pipelines», SIGGRAPH 2015 (advances.realtimerendering.com/s2015/) | media (diapositivas no extraíbles) |
| HZB en WebGPU, costes medidos | Kitware, «WebGPU occlusion culling in VTK», 29-08-2024 (kitware.com/webgpu-occlusion-culling-in-vtk/) | baja-media (los factores de aceleración no cuadran con sus medias) |
| Cajas proyectadas | Kapoulkine, «Approximate projected bounds», 12-01-2023 (zeux.io) | media |
| Límites de WebGPU | W3C, especificación de WebGPU, «limits» (w3.org/TR/webgpu/#limits), consultada el 05-11-2026 | alta |
| Octrees dispersos | Laine y Karras, «Efficient Sparse Voxel Octrees», I3D 2010 | media (PDF no leído) |
| Ladrillos en un pool con caché | Crassin y col., GigaVoxels, SI3D 2009 | media |
| Recorrido DDA | Amanatides y Woo, Eurographics 1987 | alta |
| Mapas de ladrillos y 64-árboles | dubiousconst282, «Voxel ray tracing», 03-10-2024 | baja-media |
| Puntos por cómputo | Schütz, Kerbl y Wimmer, CGF 40(4), EGSR 2021 | media |

Investigación hecha por dos agentes con búsqueda web. Las afirmaciones sin fuente primaria leída llevan confianza baja o media.
