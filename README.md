# Octree Viewer

Visor de nubes LiDAR en Rust, WebAssembly y WebGPU. Convierte LAS/LAZ o EPT local a una
jerarquía espacial persistente y carga los bloques necesarios para la vista. El objetivo a largo
plazo es explorar fuentes mayores de **500 GB decimales** con memoria acotada. La escala completa
y cualquier superioridad frente a Potree siguen sin demostrarse.

## Estado actual — 14-10-2026

- El formato de trabajo vigente es **v5 definitivo** (`OCTVFRM1`, revisión 0), separado en `.oct` (coordenadas y jerarquía) y
  `.octattrs` (RGB y atributos numéricos). Se lee y escribe v5; hay que regenerar archivos v1–v4 y v5 provisionales (`OCTVWFRM`) desde sus fuentes.
- Está implementado **`v6.1-pre` optativo**: coordenadas delta con fallback directo, sin pérdida
  adicional frente a v5, con lectores/escritores intercambiables por identidad exacta. V5 sigue
  siendo la salida predeterminada; `v6` final y otras revisiones se rechazan.
- La conversión admite LAS/LAZ y EPT local. La demo web admite carga progresiva por bloques,
  color por RGB o componente numérico, cámara orbital y modo pájaro.
- La ruta LOD voxelizada usa celdas diádicas enteras sin solapamiento entre generaciones. El
  objetivo en píxeles guía la selección; no cambia arbitrariamente el tamaño físico de las celdas.
  Los huecos de la fuente no se rellenan. Este invariante todavía necesita cobertura en todos los
  consumidores de bloques.
- Las métricas de conversión, memoria, LOD y navegador son resultados de ejecuciones concretas,
  con límites de interpretación. No equivalen a límites garantizados ni a objetivos ya cumplidos.

## Prioridades pendientes

El orden siguiente es el seguimiento activo del proyecto. Los resultados cerrados y sus protocolos
se conservan en los documentos enlazados, no como tareas completadas en esta lista.

1. **Cerrar la corrección visual y la transición LOD.** Ya se corrigió el recorte al desplazar y
   rotar, y se ajustó el foco al seleccionar el modo pájaro. La corrección conservadora de LOD
   mantiene el volumen de los representantes padres, pero usa un suelo de resolución común que
   puede retrasar detalle. Falta conservar por región las celdas visibles anteriores hasta disponer
   de hijos, sin solapamientos ni huecos añadidos. Cierre: cortes mixtos Replacement/Additive,
   carga lenta o fallida, zoom y cámara; verificar con un dataset multibloque real y confirmar el
   movimiento/velocidad/rueda del modo pájaro con teclado físico.
2. **Cerrar la validación visual en navegador.** Firefox tiene cobertura funcional y capturas de
   muestra y algunas rondas de datos reales. Chrome/AMD sigue sin presentar incluso un clear WebGPU
   independiente, aunque la lectura GPU y el picking funcionen; no atribuirlo al HZB sin nueva
   evidencia. Falta un pase visual reproducible en Chrome y una matriz de interacción que incluya
   cámara, picking, cambios de atributo, viewport móvil y recuperación de errores. La evidencia
   disponible y sus limitaciones están en el [benchmark web](docs/WEB_PERFORMANCE_BENCH.md).
3. **Medir el pico conjunto de memoria antes de retener estado incremental.** Ya hay modelos del
   builder, muestreo RSS nativo y mediciones parciales de PSS de Firefox. No describen todavía el
   pico simultáneo del caché decodificado, acumulador/proyección, instancias, escena anterior y
   recursos GPU/WASM. Extender esa observación a la carga progresiva y a un corpus Additive real;
   después decidir si la reutilización incremental aporta suficiente trabajo ahorrado para su coste
   de memoria y complejidad. La medición estricta de prefijos disponible no justifica por sí sola
   un acumulador persistente.
4. **Completar la base de conversión, lectura y escala v5.** Las conversiones nativas repetidas de
   Belvedere, Majadas y Albuera están medidas; la matriz de navegador sigue parcial y Belvedere a
   4 px no completa el objetivo bajo el presupuesto de caché probado. Separar apertura, conversión,
   primera vista, refinamiento, RSS/PSS, temporales, bytes de archivo e I/O; declarar equipo,
   perfil y estado de caché. El objetivo de más de 500 GB y la comparación con Potree requieren una
   prueba reproducible con hardware, calidad y tareas equivalentes.
5. **Resolver requisitos de producto y soporte que aún condicionan el alcance.** Acordar plataformas
   y mínimos de RAM/GPU; dimensiones LAS/LAZ que deben preservarse (incluidos CRS, flags y Extra
   Bytes); licencia y política pública de compatibilidad; y alcance de medición, clasificación,
   recorte y exportación. Hasta acordarlos, las omisiones deben seguir siendo explícitas y los
   umbrales de conversión/tamaño son referencias de evaluación, no promesas de producto.
6. **Continuar v6 por revisiones verificables.** La primera entrega `v6.1-pre` ya lee/escribe
   coordenadas delta o directas, conserva filas/atributos/precisión de v5 y funciona con LAS/LAZ,
   EPT, checkpoints y selección programática WASM. La comparación repetida en tres muestras
   pequeñas reduce tamaño conjunto un 10,4 %, 9,8 % y 49,0 %, con costes de CPU/lectura variables;
   no demuestra rendimiento a escala. [Especificación](crates/viewer_format/FORMAT_V6_PRE1.md),
   [contratos y pendientes](docs/STORAGE_V6_PREPARATION.md) y [evidencia](docs/STORAGE_BENCH.md).
   Un piloto completo de Belvedere ya convirtió ambas versiones, reconstruyó el árbol y leyó hijos
   tres veces: la pareja v6 sólo bajó 86 KB (0,005 %) y los tiempos fueron equivalentes. Esto
   descarta asumir que delta-varint ahorra en nubes de alta entropía. Falta repetir conversiones
   alternando el orden y extender a Majadas/Albuera. Ya existe un prototipo lógico aislado
   `OccupiedOctree` en `viewer_core`: arrays de celdas ocupadas por nivel, máscara e índice
   al primer hijo, conteo derivado y coordenadas reconstruidas desde una raíz entera explícita.
   No está integrado en conversión/render ni serializado en v6.1-pre. Su contrato local no
   resuelve grilla física ni CRS entre nubes; ver la entrega y tareas siguientes más abajo.
   La versión del formato en disco, la versión de esa representación lógica y la versión/algoritmos
   del renderer son contratos distintos. Caras expuestas/agrupadas y transiciones entre LOD son
   trabajo del renderer; deduplicar valores o streams es una optimización del almacenamiento.
   Medir picos de caché/render y validar WebGPU siguen pendientes. Cada cambio incompatible de
   almacenamiento requiere otra revisión pre; no habilitar `v6` final antes de cerrar compatibilidad
   y calidad.
7. **Definir el marco para varias nubes compatibles.** Es un requisito futuro que condiciona la
   topología y metadatos: CRS, unidades, ejes, origen/ancla y escala deben compartir una grilla
   diádica reproducible. Falta decidir si una escena exige una celda global fija o permite rejillas
   por nube con transformación explícita, y cómo rechazar CRS/unidades incompatibles. Resolver
   cuantización, negativos, límites enteros y escalas distintas sin desplazar silenciosamente puntos
   ni depender del orden de carga. Probar nubes convertidas por separado antes de serializar el
   contrato en otra revisión.

## Uso y formato

Conversión nativa:

```sh
cargo run --profile perf -p viewer_convert -- INPUT.laz OUTPUT.oct --format-version v5
# También admite un directorio EPT local laszip o su ept.json.
# --checkpoint conserva fases completas; --resume retoma la misma entrada/opciones.
```

La conversión produce una pareja v5 definitiva. Las muestras versionadas se han regenerado;
los datasets locales provisionales deben reconvertirse. Para ensayar la revisión experimental,
añadir `--format-version v6.1-pre` en lugar de `v5`. Se rechazan las versiones sin backend antes
de escribir y los manifiestos de checkpoint anteriores a V5. `v6.N-pre` identifica revisiones
exactas de desarrollo; por ahora sólo está implementada `v6.1-pre`, no `v6` final.

La pareja contiene:

| Archivo | Contenido | Uso |
| --- | --- | --- |
| `OUTPUT.oct` | XYZ cuantizado a `u32` (directo en v5; delta/fallback en v6.1-pre), marco local, jerarquía e índice común. | Se puede abrir sin atributos; se visualiza en gris. |
| `OUTPUT.octattrs` | RGB de presentación y columnas numéricas tipadas, alineadas por bloque/fila. | Opcional; habilita RGB, intensidad, clasificación y selección de componentes numéricos. |

En la demo, abrir `.oct` y seleccionar después su `.octattrs`, o asociar el acompañante antes de
abrir las coordenadas. Para desarrollo, servir el repositorio por HTTP:

```sh
python3 -m http.server 8000 --bind 127.0.0.1
# Abrir http://localhost:8000/examples/web-host/index.html
```

Para doble clic se usa el paquete autocontenido `artifacts/visor-web-local/index.html`. Contrato,
regeneración y pruebas de ambos modos: [README del host web](examples/web-host/README.md).
Especificación binaria: [formato v5](crates/viewer_format/FORMAT_V5.md). La publicación de los dos
archivos no es atómica; el conversor valida candidatos y publica atributos antes de coordenadas.

## Arquitectura y límites conocidos

| Área | Responsabilidad | Límite relevante |
| --- | --- | --- |
| `viewer_core` | Geometría, jerarquía, LOD, caché y contratos compartidos. | El índice residente y la memoria estimada no equivalen a RSS. |
| `viewer_format` | V5 y v6.1-pre, cuantización, codecs, checksums y lectura selectiva. | XYZ lógico ocupa 12 bytes/punto; v6 puede reducir el stream antes de LZ4; los enteros de 64 bits conservan bits en disco, pero su proyección de color puede perder precisión. |
| `viewer_convert` | Conversión LAS/LAZ con spools/checkpoints y EPT local. | EPT remoto, binary/zstandard y checkpoints EPT no están soportados. |
| `viewer_render_wgpu` | Cámara, voxelización, buffers GPU, HZB y presentación. | Los presupuestos propios no representan toda la VRAM; la GPU puede reservar recursos fuera del control del renderer. |
| `viewer_wasm` | Worker, carga LOD y conversión web en dos pasadas. | Lecturas por bloque síncronas; convertir fuentes de cientos de GB en navegador no es requisito. |
| `examples/web-host` | Demo, API embebible, carga y descarga de archivos. | OPFS/cuota, pérdida de GPU, picking y cancelación requieren validación real en navegadores objetivo. |

La conversión conserva intensidad, clasificación, retornos, ángulo, user data y point source ID; si
existen, GPS time, RGB16 y NIR. Flags LAS, Extra Bytes y CRS requieren trabajo adicional y las
omisiones se notifican. El `.oct` conserva marco local `f64` y error de cuantización acotado; el
render reconstruye posiciones locales `f32`.

## Documentación

| Documento | Contenido vigente |
| --- | --- |
| [Preparación v6](docs/STORAGE_V6_PREPARATION.md) y [v6.1-pre](crates/viewer_format/FORMAT_V6_PRE1.md) | Traits, formato experimental, compatibilidad y criterios pendientes. |
| [Benchmarks de almacenamiento](docs/STORAGE_BENCH.md) | Suite repetida: tamaños, escritura, reconstrucción y carga secuencial de hijos. |
| [Benchmark v5 de memoria y lectura](docs/V5_MEMORY_BENCH.md) | Conversión, acceso a bloques, memoria y perfiles nativos. |
| [Benchmark de construcción de vóxeles](docs/VOXEL_BUILD_BENCH.md) | Construcción, memoria conjunta, acumuladores y experimentos de reutilización. |
| [Benchmark web](docs/WEB_PERFORMANCE_BENCH.md) | Métricas de navegador, flujos, resultados y limitaciones visuales. |
| [Selección LOD](docs/LOD_FRONTIER_BENCH.md) | Contratos y mediciones de selección de frontera y prioridad incremental. |
| [Datos reales](docs/REAL_DATASET_BENCH.md) | Protocolo y resultados históricos; revisar fecha/versión antes de comparar con v5. |
| [Ramificación espacial](docs/SPATIAL_BRANCHING_BENCH.md) | Experimentos de subdivisión y cortes con diferentes ramas. |
| [Anchura de coordenadas](docs/COORDINATE_WIDTH_BENCH.md) | Evaluación del experimento `u16` y sus límites. |
| [Historial técnico](docs/DEVELOPMENT_HISTORY.md) | Snapshot anterior a v5; archivo de decisiones y evidencia, no hoja de ruta activa. |
| [Dataset de escala](data/datasets/README.md) y [muestras](data/samples/README.md) | Fuentes, procedencia y preparación reproducible. |

Los documentos de benchmark mantienen los comandos, versiones, dispersión y contraejemplos. Sus
cifras no deben trasladarse a otra versión, navegador, dataset o equipo sin una comparación válida.

## Desarrollo

Comprobaciones habituales del workspace:

```sh
cargo test-fast --offline
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo clippy -p viewer_wasm --target wasm32-unknown-unknown --offline -- -D warnings
node --test examples/web-host/tests/*.test.mjs
python3 -m unittest discover -s examples/web-host/tests -p 'test_*.py'
```

Las pruebas GPU ignoradas requieren un adaptador compatible o Vulkan por software. Para cambios en
el host web, además de las suites anteriores, comprobar con Playwright los flujos afectados, la
consola y escritorio/móvil cuando corresponda. No considerar tests nativos ni lecturas GPU como
evidencia de presentación visual en todos los navegadores.

## Entrega 14-10-2026: topología lógica candidata para v6

Assessment: `v6.1-pre` conserva puntos y atributos, pero delta-varint no aporta ahorro
material en el piloto Belvedere. Eliminar coordenadas de celdas hijas requiere una topología
explícita; `SpatialHierarchy` no garantiza que cada bloque sea una celda uniforme.

Plan y criterios de cierre de esta entrega:

- [x] Implementar en `viewer_core` un octree ocupado experimental, independiente del formato,
  con raíz explícita en coordenadas enteras, profundidad 0–32 y celdas hoja unitarias.
- [x] Guardar arrays por nivel, hijos contiguos en orden de octante `x<<2 | y<<1 | z`, máscara
  de ocho bits e índice al primer hijo. Derivar el conteo con popcount, sin flags indefinidos
  ni coordenadas por nodo. Mantener privados los registros para preservar invariantes.
- [x] Construir de forma determinista, colapsar entradas duplicadas, rechazar celdas fuera de
  raíz y raíces desbordadas. Permitir consulta de ocupación y reconstrucción exacta de hojas.
- [x] Probar vacío, profundidad cero, ocho octantes, negativos, límites i32, orden de entrada,
  duplicados y equivalencia con un conjunto de celdas. Pasan `cargo test-fast --offline`,
  `cargo fmt --all -- --check`, Clippy workspace/all-targets y Clippy WASM con `-D warnings`.
- [x] Actualizar contratos/documentación y revisar el diff para el commit de esta entrega.

Pendiente después de esta entrega: integrar payloads/atributos y semánticas Replacement/Additive;
definir grilla física, CRS/unidades y compatibilidad entre nubes; decidir conteo/flags y ancho de
índices en disco; medir tamaño total, construcción/apertura, consultas y pico de memoria frente
a v5/pre1 con datasets reales. Sólo entonces considerar otra revisión de almacenamiento.
Sugerencia: evaluar primero el conteo derivado y posponer flags de deduplicación hasta disponer
de patrones medidos. Este prototipo no cambia conversión, renderer ni compatibilidad en disco,
y no demuestra ahorro, velocidad o menor RSS.
