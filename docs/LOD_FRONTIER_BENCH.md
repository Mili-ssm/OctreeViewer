# Selección LOD: base de frontera — 11-10-2026

Benchmark específico de `SpatialHierarchy::select_lod` sobre `279ec98`, sin cambios del algoritmo.
`visibility_bench` mide consulta de vóxeles y no cubre esta ruta. Esta base separa el coste de
seleccionar metadatos de lectura, voxelización, subida GPU y presentación en navegador.

## Método

Entorno de [las mediciones v5](V5_MEMORY_BENCH.md#entorno-y-método), perfil `perf`, tres procesos
secuenciales sin compilaciones/checks concurrentes. No se fijaron frecuencia CPU ni carga externa.
Cada caso hace una selección de calentamiento y nueve muestras; la tabla muestra la mediana de
las tres medianas y su mínimo–máximo. La creación/validación de la jerarquía queda fuera del tiempo.

El ejemplo genera seis jerarquías: raíz estructural con 64/256/1.024 ramas de dos hojas, árboles
binarios de profundidad 8/10 y una cadena de 1.024 nodos. Cada nodo con payload representa un
punto; el conteo es de representantes del índice, **no de vóxeles renderizados**. La ramificación
1.024 es un estrés de la API de jerarquías arbitrarias, no una salida del conversor 2³/4³/8³.

Combina `Replacement`/`Additive`, presupuesto completo/parcial, frustum completo/mitad y dos
callbacks: error declarado y un modelo sintético de distancia con raíz/división. El segundo no
reproduce la proyección conservadora de la cámara real. Tolerancia cero solicita todo el detalle
disponible; las hojas quedan limitadas por resolución. La cadena no usa frustum parcial y su caso
`Replacement` tiene un único presupuesto: un representante en cada profundidad. Son **86 casos**.

El tiempo incluye el contador de callbacks y la construcción del resultado. Los nodos únicos
proyectados se cuentan en el calentamiento, fuera del cronómetro. Cada muestra compara el
`LodSelection` completo con el calentamiento, incluidos orden, errores y diagnósticos. Los tres
procesos dieron iguales contadores y checksums de IDs/diagnósticos/conteos/error máximo. Estos
checksums facilitan detectar diferencias futuras, pero no sustituyen pruebas de equivalencia.

## Resultados seleccionados

Frustum completo y callback de error declarado. Presupuesto completo salvo donde se indica.

| Jerarquía / semántica | Presupuesto | Nodos únicos proyectados | Llamadas al callback | Selección (ms) |
| --- | ---: | ---: | ---: | ---: |
| 64 ramas / Replacement | 128 | 192 | 6.368 | 0,564 [0,266–0,565] |
| 256 ramas / Replacement | 512 | 768 | 99.200 | 4,258 [4,206–5,995] |
| 1.024 ramas / Replacement | 2.048 | 3.072 | 1.576.448 | 68,041 [67,973–69,474] |
| 1.024 ramas / Additive | 3.072 | 3.072 | 1.576.448 | 68,572 [68,569–68,715] |
| 1.024 ramas / Additive, parcial | 2.048 | 2.048 | 658.176 | 45,968 [45,915–46,036] |
| Binario profundidad 10 / Replacement | 1.024 | 2.047 | 525.824 | 31,248 [31,247–31,914] |
| Cadena / Replacement | 1 | 1.024 | 1.025 | 0,087 [0,086–0,089] |
| Cadena / Additive | 1.024 | 1.024 | 1.025 | 0,241 [0,241–0,245] |

El algoritmo recorre la frontera completa tras cada refinamiento y vuelve a construir/contar
los hijos visibles de candidatos. La frontera amplia repite mucho trabajo; la profundidad sola
no predice ese coste. Los casos pequeños tienen dispersión apreciable: no atribuir las diferencias
entre semánticas a un mecanismo concreto sin aislarlo. No se midió RSS, cámara real, datos LiDAR,
tiempo de frame ni FPS. Una cola de prioridad todavía no se ha comparado con esta base.

## Contrato que debe conservar la siguiente mejora

- Mayor error primero; empate por menor `NodeId`, incluyendo infinito positivo. El orden final
  del corte sigue el orden de los hijos en la jerarquía de entrada: no se ordena por ID.
- Refinamiento atómico: todos los hijos visibles o ninguno. Rechazar una base que excede presupuesto.
- `Replacement` puede reducir el conteo; un candidato antes bloqueado debe reconsiderarse después.
  `Additive` conserva ancestros y suma hijos. Los nodos estructurales se atraviesan sin payload.
- Mismos conteos, máximo error, diagnósticos y su orden, errores inválidos/overflow y cobertura.
  La nueva regresión prueba empate finito/infinito para ambas semánticas y entradas invertidas.
- La API actual no declara estabilidad/pureza del callback y lo reevalúa. Memorizar prioridades
  necesita resolver ese contrato; cambiar el número de callbacks puede ser observable. No añadir
  una caché sin límite ni sacrificar la memoria para acelerar solo el caso sintético.

La prioridad incremental sigue pendiente: requiere memoria contabilizada y comparación contra
esta base y cortes reales. La primera mejora implementada reutiliza temporales, descrita debajo.
El benchmark por sí solo no justifica una nueva API o un índice nuevo.

```sh
cargo run --profile perf -p viewer_core --example lod_frontier_bench --offline
cargo test -p viewer_core --test lod --profile fast --offline
```

Logs locales excluidos de Git: `target/audit-lod-frontier/run-{1,2,3}.txt` y `summary.json`.

## Reutilización de temporales — 11-10-2026

Comparación contra `ffd4af9`, con los mismos ejemplos en ambos binarios. `select_lod` reutiliza
los vectores del recorrido, hijos candidatos/ganadores y diagnósticos durante una llamada;
inserta los hijos en la selección sin clonar un vector intermedio. Mantiene los recorridos y
las reevaluaciones del callback: no añade caché, cola, índice persistente ni cambios de API.
Las capacidades crecen según las fronteras encontradas y se liberan al volver; esto no establece
un presupuesto en bytes ni reduce la complejidad cuadrática del escaneo.

Mismo entorno y perfil anteriores. Tres pares de procesos, alternando antes/después,
después/antes, antes/después, sin compilaciones/checks concurrentes. Cada caso usa calentamiento
y nueve muestras. Tablas: mediana de las tres medianas, con mínimo–máximo entre corchetes.
Los 86 casos sintéticos conservaron contadores y checksums entre versiones; cada muestra
comprueba el resultado completo contra su calentamiento. No hubo regresiones de mediana
superiores al 2 % en esta matriz; no es una garantía para otras jerarquías.

Frustum completo y callback declarado; presupuesto completo salvo indicación:

| Jerarquía / semántica | Antes (ms) | Después (ms) |
| --- | ---: | ---: |
| 1.024 ramas / Replacement | 68,422 [68,411–68,819] | 61,118 [61,074–61,666] |
| 1.024 ramas / Additive | 69,270 [68,496–69,300] | 61,479 [61,438–61,987] |
| 1.024 ramas / Replacement, presupuesto 1.536 | 46,179 [45,885–46,196] | 41,269 [41,230–41,445] |
| Binario profundidad 10 / Replacement | 31,407 [31,269–31,486] | 28,193 [28,091–28,306] |
| Cadena / Replacement | 0,087 [0,086–0,090] | 0,080 [0,079–0,081] |

El nuevo `viewer_render_wgpu/examples/lod_selection_bench.rs` abre solo metadatos de Belvedere
v5 (1.343 nodos), ajusta `OrbitCamera` al dataset y usa su proyección/frustum a 640×480, sin crear
dispositivo GPU. Combina objetivos de 1/4/16 px con presupuestos de 1 M/4 M/`u64::MAX`
representantes del índice. Son nueve selecciones diagnósticas; no cambian límites del renderer.
El cronómetro excluye apertura, payloads, voxelización y renderizado. Los IDs completos de
selección y diagnósticos, conteos, bits del error máximo y llamadas coinciden entre todas las
ejecuciones de ambas versiones.

| Objetivo / presupuesto | Nodos seleccionados | Callbacks | Antes (ms) | Después (ms) |
| --- | ---: | ---: | ---: | ---: |
| 1 px / sin límite | 1.083 | 133.766 | 10,252 [10,220–10,298] | 9,417 [9,374–9,438] |
| 4 px / sin límite | 320 | 15.116 | 1,180 [1,179–1,183] | 1,079 [1,078–1,103] |
| 4 px / 4 M | 143 | 3.358 | 0,544 [0,542–0,550] | 0,487 [0,483–0,496] |
| 16 px / sin límite | 19 | 69 | 0,0051 [0,0051–0,0052] | 0,0050 [0,0049–0,0050] |

Los casos reales de 1 px con presupuestos 1 M/4 M tuvieron dispersión amplia y rangos
solapados; no permiten atribuir una mejora estable. La reducción observada del corte completo
de Belvedere a 1 px es del 8,1 %; la frontera sintética amplia `Replacement` mejora un 10,7 %.

RSS pico del proceso completo (`/usr/bin/time -v`), mediana [mínimo–máximo], en KiB:

| Ejemplo | Antes | Después |
| --- | ---: | ---: |
| Sintético | 4.412 [4.280–4.476] | 4.412 [4.276–4.480] |
| Belvedere | 6.672 [6.668–6.744] | 6.672 [6.552–6.792] |

No se observa reducción de RSS; estos valores incluyen índice y benchmark, no aíslan los
temporales. No se midieron FPS ni Chrome/Firefox. Como regresión nativa, `render_dataset` con
`fs_e430_n4008_18.oct`, 4 px y 640×480 produjo el mismo PNG byte a byte que la referencia:
1.849 vóxeles, 111.190 puntos fuente, 9.790 píxeles no vacíos, sin vóxeles sobredimensionados.

Cinco tests nuevos comparan con una copia independiente del algoritmo anterior: 192 combinaciones
de semántica, frustum, presupuesto, tolerancia, entrada y callbacks estables/con estado;
errores inválidos en candidatos y validación final, overflow de base/candidato, y buffers sin
residuos con capacidad reutilizada. Pasan 231 tests del workspace (dos GPU ignorados), formato,
Clippy nativo/WASM y checks nativos/WASM.

```sh
cargo run --profile perf -p viewer_render_wgpu --example lod_selection_bench --offline -- INPUT.oct
cargo run --profile perf -p viewer_core --example lod_frontier_bench --offline
cargo test-fast --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo clippy -p viewer_wasm --target wasm32-unknown-unknown --offline -- -D warnings
cargo check-all --offline
cargo check-web --offline
```

Logs locales excluidos de Git: `target/audit-lod-scratch/{synthetic,real}-{before,after}-{0,1,2}.txt`,
sus `.time.txt`, `summary.json` y `render/voxel-4px.png`.

## Prioridad incremental opt-in — 13-10-2026

`select_lod_stable_projector` exige error estable por nodo durante la llamada; no garantiza
orden/número de callbacks. La API original conserva los callbacks con estado. Heap y lista de
bloqueados evitan reproyectar toda la frontera; Replacement reconsidera candidatos al liberar
presupuesto. Orden visible, empates, diagnósticos y errores siguen el selector original.

El límite configurable (por defecto 4 MiB) cubre capacidades heap/bloqueados, no temporales
comunes, RSS ni picos internos del allocator. Reservas falibles o validaciones ambiguas liberan
el intento y reinician el selector original. `try_reserve_exact` puede reservar más de lo pedido;
se comprueba también la capacidad obtenida. No hay caché persistente. Las búsquedas, splices y
reintentos todavía pueden ser lineales.

Comparación contra `867c62c`, perfil perf, tres pares alternados antes/después, después/antes,
antes/después, sin checks/compilaciones concurrentes. Cada proceso calienta y toma nueve muestras.
Los 86 casos sintéticos y nueve cortes Belvedere conservan resultados completos/checksums; los
callbacks cambian deliberadamente. Mediana de las tres medianas, en microsegundos:

| Caso | Antes | Después | Callbacks antes → después | Prioridades (bytes) |
| --- | ---: | ---: | ---: | ---: |
| 1.024 ramas Replacement, completo | 61.643,364 | 600,046 | 1.576.448 → 5.120 | 24.576 |
| 1.024 ramas Replacement, presupuesto 1.536 | 41.459,222 | 400,301 | 658.176 → 3.584 | 36.864 |
| Binario profundidad 10 Replacement, completo | 28.358,674 | 374,503 | 525.824 → 3.071 | 12.288 |
| Cadena Replacement | 79,639 | 80,431 | 1.025 → 1.025 | 0 |
| Belvedere 1 px, sin límite | 9.447,612 | 228,619 | 133.766 → 2.426 | 6.144 |
| Belvedere 4 px, sin límite | 1.083,935 | 61,144 | 15.116 → 732 | 3.072 |
| Belvedere 16 px, sin límite | 5,009 | 3,507 | 69 → 43 | 96 |

Rangos Belvedere: 1 px 9.380,275–9.598,465 → 226,776–239,118 µs;
4 px 1.076,230–1.096,558 → 60,714–64,831 µs. Las nueve cámaras y los 80 casos
sintéticos ramificados mejoraron en esta matriz. No es garantía para otras jerarquías.

La primera prueba empeoró las cadenas aproximadamente un 79 % (Replacement 79,218 →
141,516 µs). Se conserva ese contraejemplo: ahora una jerarquía validada cuyo número de nodos
es profundidad máxima + 1 usa directamente la ruta original. Las seis cadenas finales quedan
por debajo de un 2 % de diferencia y sin almacenamiento de prioridades. Máximo observado de
prioridades en toda la matriz: 36.864 bytes; no es una medición de RSS.

Nueve regresiones nuevas cubren equivalencia generada, presupuesto, empates/infinito, orden de
entrada, descendientes inválidos inactivos, overflow y fallback. Workspace: 264 tests pasan,
tres ignorados; formato, Clippy nativo/WASM, check-web y diff pasan. Esto mide selección CPU
de metadatos; no mide lecturas, GPU, FPS ni latencia del navegador.

```sh
cargo run --profile perf -p viewer_core --example lod_frontier_bench --offline -- --stable
cargo run --profile perf -p viewer_render_wgpu --example lod_selection_bench --offline -- INPUT.oct --stable
```

Artefactos locales excluidos: `artifacts/lod-priority/`, binarios congelados `before-*`/`after-*`,
`run_pairs.py`, logs de tres pares y `summary.json`; `first-trial/` conserva la regresión inicial.

### Adopción en WASM

Las dos selecciones de cámara en `LodDataset` usan el proyector estable y 4 MiB de prioridades.
La cámara/viewport son inmutables durante la llamada síncrona; los consumidores con callbacks
arbitrarios conservan `select_lod`. El fallback del selector mantiene la funcionalidad al agotar
la memoria de prioridades. Se repitieron 264 tests (tres ignorados), Clippy nativo/WASM y build
release WASM; paquete local regenerado.

Playwright MCP, Chrome 155.0.8059.39 con SwiftShader: pasan `lod-budget-fallback`, `lod-session`,
`lod-attributes`, `lod-reuse`, sin errores JS no capturados. El probe de atributos también pasa
con `artifacts/dispatch-refactor/after-run-1/bunny.oct` y su sidecar, en RGB/intensidad/clasificación/
`attribute:0:0`: cobertura completa de 35 bloques, 40.256 puntos fuente, 21.969 vóxeles, cero
sobredimensionados, geometría conservada. Estas pruebas funcionales no resuelven la presentación
visual pendiente en Chrome ni demuestran mejoras de FPS o memoria total.
