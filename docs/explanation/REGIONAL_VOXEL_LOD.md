# Resolución regional de representantes Replacement — 14-10-2026

Entrega parcial de la prioridad 1 del [estado](../estado.md#prioridades-pendientes).
Una rama con representantes gruesos ya no impone automáticamente su resolución a todo el
corte. Se conserva una grilla diádica compartida, con celdas enteras y una única regla terminal
para todos los bloques. No cambia v5/pre1–pre4, la política de caché ni Additive.

## Problema y contrato

Antes, `build_voxel_scene_with_limits` elegía una profundidad máxima global: el mínimo entre
los límites de todos los nodos Replacement seleccionados que tenían bloques y no eran hojas.
Cada límite usa `floor(log2(root_side / (node_side / 8)))`, acotado a 0–24, porque los
representantes nativos se construyen sobre una subdivisión 8³. Un padre todavía residente
podía impedir detalle en otra rama que ya disponía de sus puntos finos.

Ahora cada nodo grueso marca las regiones que contienen sus **puntos cargados**, a profundidad
`D−3`, donde D es su límite anterior. Cuando una celda del descenso llega a D, consulta si su
ancestro de profundidad `D−3` está marcado. Si lo está, termina ahí; también puede terminar
antes por proyección/culling o por otra restricción más gruesa. La misma regla se aplica a
puntos de hojas, padres y cualquier otro bloque, sin importar el orden de acumulación.

Esto conserva el tamaño propio de los representantes y permite detalle fuera de sus regiones.
No conserva el ensanchamiento artificial que les podía imponer otro padre mediante el suelo
global anterior. No se infiere ocupación de AABB vacías ni se generan hijos o puntos artificiales.
Una región que cruza el límite de un nodo puede limitar conservadoramente a su vecino.

La decisión terminal depende de la celda y de restricciones inmutables. Por eso, cuando una
celda es terminal, ningún punto puede producir descendientes dentro de ella: la frontera no
contiene simultáneamente un ancestro y sus hijos. El hint de última celda sigue siendo válido.
Los máximos inclusivos, negativos y puntos exteriores usan el mismo mapeo/clamping que el resto
del builder; no hay una segunda regla de coordenadas con tolerancias epsilon.

## Espaciado de referencia por nube (RF-05)

La continuidad entre nubes usa un único espaciado por nube: la **mediana ponderada por puntos del
espaciado de todas sus hojas** (hojas ordenadas por espaciado creciente; gana la primera cuyo
recuento acumulado alcanza la mitad del total, y en un empate exacto el valor menor; sin puntos,
todas pesan igual). Lo calcula `viewer_core::reference_spacing`. `v7.5-pre` lo guarda en el bloque
de metadatos ([formato](../reference/format/FORMAT_V7_PRE5.md)), de modo que no cambia al injertar
o expulsar páginas; los demás formatos, con la jerarquía completa, lo calculan al abrir.

## Coste acotado y fallback

- Se admiten como máximo ocho regiones distintas por nodo. Es una guardia de coste, no una
  suposición de validez del proveedor. Un payload externo más disperso conserva el suelo global
  antiguo para toda la escena. D menor que 3 también usa ese fallback; D=24 no añade restricciones.
- Un primer pase usa arrays de ocho claves en stack, valida el límite y suma las longitudes con
  aritmética comprobada **antes** de asignar memoria. El segundo llena un vector reservado de
  forma falible, después ordenado/deduplicado. Reserva como máximo ocho claves por nodo grueso.
- La capacidad efectiva se carga al presupuesto estimado junto a la reserva de proyección.
  Ambas se liberan antes de reservar instancias; la estimación registra el máximo entre fases.
  Es contabilidad de componentes, no RSS ni pico simultáneo de navegador/GPU.
- Las actualizaciones siguen construyendo y subiendo una escena candidata antes del commit del
  corte. Un error de construcción conserva la escena aceptada mediante el contrato existente.
  No se añade acumulación persistente ni un índice espacial general.

Se descartó un primer candidato que exigía igualdad exacta entre bounds `f64` y celdas de la
grilla: dejó el piloto real en el fallback global y añadió coste sin detalle. Derivar regiones
de los representantes reales elimina esa restricción sin mantener dos mecanismos regionales.

## Pruebas y navegador

La [regresión nativa](../../crates/viewer_render_wgpu/tests/voxel_regions.rs) falla antes del cambio:
un padre y una hoja vecina generan dos vóxeles en lugar de tres. Pasa después conservando el
padre de tamaño 1 y dos celdas finas independientes. Once pruebas comprueban también:
orden de nodos, origen geográfico grande, negativos locales, frontera compartida, puntos de
otro bloque dentro de la región gruesa, padres a distintas resoluciones, bounds no cúbicos,
culling, RGB/intensidad propios de las celdas finas, caché de proyección pequeña, presupuesto
y fallback de más de ocho regiones. Se
comprueban tamaños, conteos y ausencia de intersecciones volumétricas entre instancias.

Pasan 354 tests del workspace; cuatro quedan ignorados en esa suite. Los dos tests GPU
ignorados del renderer pasan por separado con Vulkan/lavapipe. También pasan fmt, Clippy
workspace/all-targets y Clippy WASM con `-D warnings`, compilación WASM release, generación de
bindings y paquete local. Los tests del host sin cambios conservan la validación de la
[migración anterior](../records/archive/RENDERER_DEPENDENCIES.md); esta entrega añade comprobación de sintaxis del
nuevo probe, sin atribuirle una nueva pasada de todas las suites JS/Python.

[Probe WebGPU](../../examples/web-host/tests/lod-regions.browser.mjs), ejecutado con Playwright MCP:
parent fallback y tres vóxeles en el corte mixto, reconstrucción tras resize, llegada del hijo,
cuatro vóxeles y cobertura completa. La cámara explícita permite comprobar la rama fina incluso
cuando el escalonado sigue en 16 px esperando bloques. Tras llegar el hijo se invalida la cámara
para reconsiderar el objetivo ya residente, sin cambiar la política de escalonado del visor.

[Resultado de los probes y contraejemplos](../records/benchmarks/data/voxel-regions-browser-2026-10-14.json).
Se activó WebGPU con `--enable-unsafe-webgpu --use-webgpu-adapter=swiftshader
--enable-unsafe-swiftshader --use-angle=swiftshader`; `requestAdapter` confirmó arquitectura
`swiftshader`. La prueba de atributos conserva sus 4.096 vóxeles. La prueba separada de fallback
por presupuesto no completa la solicitud de adapter. El clear JavaScript independiente vuelve
a perder el dispositivo con `A valid external Instance reference no longer exists.`
La consola de página registra además un 404 de recurso al abrir la raíz del servidor.
Esta evidencia funcional por software no cierra presentación visual, GPU física, móvil ni FPS.

## Comparación nativa reproducible

[Datos completos, comandos, hashes y muestras](../records/benchmarks/data/voxel-regions-belvedere-2026-10-14.json).
Fuente Belvedere completa en pareja `v6.4-pre`; se lee solo XYZ y se construyen cortes fijos de
su índice real. No son cargas completas de los 81,7 millones de puntos ni recorridos de cámara.
Perfil `perf`, viewport 1280×720, objetivo 4 px, caché decodificada de 256 MiB. El corte mixto
contiene 13 bloques y 39.414 filas seleccionadas, incluidos representantes LOD; retiene 631.352 B
de caché estimada. No confundir esas filas con el total de puntos fuente.

Tres pares de procesos alternados antes/después por modo, tres construcciones en cada proceso:
nueve muestras por variante, correlacionadas dentro del proceso. El cronómetro excluye apertura,
I/O, decodificación y checksum; incluye configuración regional y construcción/finish. La última
serie no coincide con compilación/tests de esta tarea; la carga del host y la caché no se aíslan.
El binario anterior usa el código base `d190121` con el mismo diagnóstico añadido; se conservan
los ejecutables y comandos locales bajo `target/regional-voxel-20261014/`.

| Corte fijo | Vóxeles antes → después | CPU mediana antes → después (ms) | Rango antes / después (ms) |
| --- | ---: | ---: | --- |
| `held`: hijos de raíz sin refinar | 81 → 81 | 0,126 → 0,116 | 0,106–0,167 / 0,054–0,176 |
| `mixed`: retener una rama, refinar las demás un nivel | 325 → 472 | 2,989 → 5,807 | 1,442–3,369 / 5,719–9,564 |
| `refined`: refinar todas las ramas un nivel | 325 → 828 | 1,481 → 6,408 | 1,455–2,002 / 6,314–8,759 |

`held` conserva el checksum geométrico `1623410fa92167a3`; no demuestra aceleración por sus
rangos solapados. `mixed`/`refined` cambian de checksum deliberadamente: construyen más detalle,
no un resultado equivalente más rápido. El tamaño mínimo del corte mixto baja de ~378,970 a
~47,371 unidades locales; el máximo se mantiene ~378,971 por los representantes gruesos.

La estimación de pico del builder mixto sube 33.616.800 → 33.645.216 B y la capacidad de instancias
13.000 → 18.880 B. Son modelos de capacidad, no RSS. Sigue habiendo vóxeles sobre el objetivo de
4 px porque la fuente gruesa aún limita sus regiones; el incremento de detalle tampoco acredita
que todas las celdas finales cumplan ese objetivo. No se mide ahorro de I/O ni rendimiento GPU.

```sh
cargo test -p viewer_render_wgpu --profile fast --offline --test voxel_regions
cargo run --profile perf --offline -p viewer_render_wgpu --example regional_voxel_bench -- \
  INPUT.oct mixed 3
# held/refined repiten exactamente el corte correspondiente, sin selección dependiente de caché.
```

## Cierre pendiente

### Construcción de corte mixto — 24-10-2026

`build_voxel_scene_with_limits` acepta ahora un corte Replacement que contiene un padre y
un descendiente ya residente. Solo aplica la exclusión si todos los bloques del descendiente
están disponibles; un hijo sin payload mantiene el fallback del padre. Excluye del payload
del padre los puntos dentro de los bounds del descendiente seleccionado, con frontera mínima
inclusiva y máxima exclusiva salvo el
máximo exterior de la raíz. Usa coordenadas locales `f64` respecto al origen del dataset.
Los puntos restantes del padre mantienen su profundidad terminal de representantes; el
hijo no hereda ese suelo. Ambos llegan al menos a la profundidad de la frontera común
antes de terminar por proyección, de modo que una cámara lejana tampoco mezcla los colores
en una sola celda. Las exclusiones se cargan al presupuesto estimado del builder; sigue
siendo un modelo, no una medida RSS.
Un corte mixto cuya frontera no coincide con la grilla de representantes, o cuyo hijo está
más profundo que esa grilla, se rechaza con `MixedCutNotAligned` antes de construir la escena.

La prueba nativa construye explícitamente el corte padre+hijo durante una secuencia de
carga Replacement/Additive. Comprueba celdas disjuntas, color padre/hijo y el borde exacto.
Otra prueba usa un origen de 10¹² unidades y verifica RGB e intensidad propios del hijo;
una tercera comprueba el rechazo de una frontera desalineada. Pasan las 14 pruebas
`voxel_regions` y las pruebas del crate renderer. El modo `partial` del
diagnóstico nativo acepta ese corte en la pareja v5 Belvedere regenerada; a 4 px y con la
cámara ajustada a toda la nube produjo 81 vóxeles y el mismo checksum que `held`:
esa vista no distingue visualmente el detalle del hijo. Una sola muestra de CPU no mide
coste relativo.

El planificador publica ahora un hijo directo residente junto al padre cuando la frontera
del hijo coincide con la grilla diádica de la raíz. Si el corte tiene padre e hijo, protege
solo el payload del padre: una caché que admite padre y un hijo puede expulsar el hijo
parcial para intentar cargar su hermano. La escena GPU anterior sigue activa hasta que
el builder y el upload aceptan el siguiente corte; si el hijo ya no está residente, el
plan siguiente vuelve al padre. La prueba del ciclo de caché comprueba la llegada alterna
de hijos sin proteger ambos. Los descendientes profundos, desalineados o más finos que los
representantes del padre siguen esperando el corte completo. Una regresión con bounds
contenidos pero no canónicos para su nivel reprodujo un corte admitido por el planificador
que rechazaba el builder; ambos aplican ahora el mismo límite de profundidad.

Esto permite progreso regional para un caso concreto. Sigue faltando observación visual
de la transición entre frames, cargas con error, movimiento y zoom en navegador sobre un
dataset real multibloque. La GPU física y la presentación no quedan certificadas por las
pruebas nativas.

### Comprobación adicional — 24-10-2026

Se añadió `slow_and_unavailable_multiblock_loads_preserve_renderable_cuts` a
[`voxel_regions.rs`](../../crates/viewer_render_wgpu/tests/voxel_regions.rs). La prueba combina
`LodTransitionPlanner` y el builder para una jerarquía sintética de padre y dos hijos.
Simula ausencia de respuesta, llegada de un hijo, otro intento sin bloque y llegada del segundo.
En Replacement, conserva los dos vóxeles rojos del padre hasta que ambos hijos están
residentes; entonces publica los colores propios azul y verde. En Additive, admite cada
hijo disponible. En cada paso comprueba que las instancias no se solapan. Es evidencia
nativa de esos cortes, no una observación de presentación ni de continuidad entre frames.

Los `.oct` locales antiguos de Belvedere y Majadas tienen la magia provisional `OCTVWFRM`
y el lector vigente los rechaza con `ProvisionalFormat`. Para repetir el corte real se
reconvirtió el LAZ Belvedere completo a v5 bajo `target/lod-belvedere-20261024.oct`:
81.682.329 puntos, 1.343 bloques y 1.591.650.185 bytes para la pareja. Es una salida local
no versionada; se omitieron flags LAS y CRS tal como informa el conversor. El diagnóstico
`regional_voxel_bench` con una sola ejecución por corte dio 81/472/828 vóxeles y los
checksums `1623410fa92167a3` / `37ea64bec4787572` / `24f5190724d9cb92` para
`held` / `mixed` / `refined`, respectivamente. Los checksums coinciden con el registro
anterior. Las muestras de CPU únicas no justifican una conclusión de rendimiento.

```sh
cargo run --release --offline -p viewer_convert -- \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz \
  target/lod-belvedere-20261024.oct --format-version v5 --max-memory-mib 512
cargo test --offline -p viewer_render_wgpu --test voxel_regions
cargo run --profile perf --offline -p viewer_render_wgpu --example regional_voxel_bench -- \
  target/lod-belvedere-20261024.oct mixed 1
```

Se recompilaron los bindings WASM actuales con `cargo build --release --offline --target
wasm32-unknown-unknown -p viewer_wasm` y `wasm-bindgen --target web`. Playwright MCP abrió
el host, pero mostró `init failed: no GPU adapter supports this surface`; no se pudo
observar transición, cámara ni recuperación WebGPU en esta sesión.

Tras recompilar WASM y regenerar `pkg` con este ajuste, Playwright MCP abrió el host el
24-10-2026 y volvió a mostrar `init failed: no GPU adapter supports this surface`.
Los bindings y WASM respondieron 200; la consola solo registró un 404 de `favicon.ico`.
No se pudo repetir la interacción LOD en ese navegador.

### Firefox con archivo multibloque y lectura fallida — 24-10-2026

Se convirtió la muestra LAS versionada de Outer Banks a v5 con bloques de 10.000 puntos:
149.730 puntos, 55 bloques, pareja local ignorada bajo `target/`. En Firefox 158.0.1
headless con WebGPU habilitado, el host a 1280×800 cargó el `.oct` sin compañero de
atributos. La captura mostró la nube gris; durante el refinamiento se observaron 34
bloques visibles y 4 pendientes a 8 px. Tras llegar a 4 px mostró 40 bloques visibles,
149.730 puntos y cero pendientes. Un arrastre de cámara cambió la vista capturada sin
errores de consola. Es observación de presentación en ese entorno, no evidencia de GPU
física ni una medida de rendimiento repetida.

Para provocar una lectura fallida se copió el `.oct` y se cambió un byte del payload en
el offset 1.500.000, anterior al índice. El lector informó checksum incorrecto del bloque
47. El host mantuvo visible el corte previo en la captura: 20 bloques, 262 vóxeles y 20
bloques pendientes a 8 px; mostró el error sin vaciar el canvas. Al abrir después la copia
sana en la misma sesión, volvió a 4 px, 40 bloques visibles y cero pendientes, con la nube
presentada. La consola no registró errores. Esta prueba cubre un archivo real con una
corrupción inyectada, no fallos de red, pérdida de dispositivo ni todas las regiones.

```sh
cargo run --release --offline -p viewer_convert -- \
  data/samples/fs_e432_n4002_18.las target/lod-outerbanks-multiblock.oct \
  --format-version v5 --points-per-block 10000 --max-memory-mib 256
python3 - <<'PY'
from pathlib import Path
source = Path('target/lod-outerbanks-multiblock.oct')
data = bytearray(source.read_bytes())
data[1_500_000] ^= 0xff
Path('target/lod-outerbanks-corrupt.oct').write_bytes(data)
PY
```

Se sirvió la raíz con `python3 -m http.server 8767 --bind 127.0.0.1` y se abrió
`/examples/web-host/index.html` en un perfil Firefox nuevo con
`dom.webgpu.enabled=true` y `dom.webgpu.service.enabled=true`. Los dos archivos se
seleccionaron mediante el control **Open project dataset**. El Playwright MCP
predeterminado seguía sin adapter; esta observación procede de Firefox BiDi.

### Retención regional del corte anterior — 24-10-2026

`LodTransitionPlanner` conserva el último corte visible con cobertura completa como base
regional cuando el objetivo nuevo tiene bloques pendientes. El plan mezcla las regiones
anteriores con las nuevas que ya pueden publicarse, para Replacement y Additive. Al completarse el
objetivo, el plan deja fuera la base anterior y el commit la sustituye solo después de que el
builder y el upload aceptan el corte. Los payloads del corte completo quedan protegidos durante
esa transición. La caché no supera su presupuesto: si no cabe el siguiente bloque junto al
corte retenido, la inserción se rechaza y el corte visible permanece; en ese caso el refinamiento
se pausa hasta que otro objetivo ya residente cierre la transición o se reinicie el dataset.
Additive conserva sus
contribuciones anteriores; no interpreta un padre como reemplazo semántico de sus hijos.

Las regresiones de `crates/viewer_core/tests/lod_transition.rs` cubren el cambio de región con
carga pendiente, la retirada del corte anterior al completar el objetivo y el rechazo de una
inserción que excedería el presupuesto. Pasan las 13 pruebas de esa suite, las 14 pruebas
`voxel_regions` y 34 pruebas WASM (una microprueba queda ignorada). Clippy pasa para los tres
crates afectados, también para WASM. Esto valida los planes y la protección de la caché, no
una captura entre frames. La comprobación Playwright MCP con bindings recompilados abrió la demo,
pero WebGPU informó `no GPU adapter supports this surface`; a 390 px no hubo desbordamiento
horizontal. La consola registró el 404 preexistente de `favicon.ico`. La prioridad 1 sigue abierta
para zoom, más cámaras/cargas, secuencia de hijos ocupados con atributos, errores reales y
observación visual en las plataformas objetivo.
`sync_visible_blocks` solo vacía la escena durante el reset de datos o el rechazo de la selección
base en WASM. La publicación voxelizada completa usa `upload_voxel_cut`; no hay otra ruta de
consumo de bloques que requiera agregación.

## Selección por celda en la GPU (fase 2a, 16-11-2026)

Con `cellLod: cell` el pase de slots decide **por celda** qué nivel activo de un nodo dibuja cada punto, de modo que las
fronteras entre niveles siguen la distancia de cada celda y no los bordes de octante. Con `octant` (por defecto) todo
queda como antes. Diseño y decisiones E1–E8 en el [plan de la fase 2a](../records/archive/fase-2a-lod-por-celda-en-gpu.md#design-decisions-agent-16-11-2026);
resultados en el [registro](../records/benchmarks/R3_FASE2A_LOD_POR_CELDA_16-11-2026.md).

### Regla

| Id | Regla |
| --- | --- |
| E1 | Una celda refina si `side > min_side` y su arista proyectada **real** (`projected_voxel_edge_pixels`: 8 esquinas relativas al ojo, `+inf` si alguna queda detrás) supera el objetivo. Métrica elegida y medida en el [registro de la métrica](../records/benchmarks/R3_FASE2A_METRICA_16-11-2026.md) |
| E2 | Coordenadas enteras relativas a la esquina del nodo a cada profundidad (`rel = abs − (node_lo << (d − node_depth))`); solo se activan profundidades `node_depth ≤ d ≤ node_depth + 31` |
| E4 | Se dibuja la celda si no está enmascarada y, para **cada** profundidad activa más gruesa, su ancestro refina (o está enmascarado) y ella no refina. Se evalúan los ancestros del más grueso al más fino y se para en el primero que no refina: exactamente una celda por punto, sin suponer monotonía |
| E5 | `masked_any` (D5): a profundidad `≥ rep` (`node_depth + 3`) es el bit 8³; por debajo, un bit de un mip precalculado (4³, 2³, 1³) |
| E6 | Conjunto activo: el nivel residente más cercano (el más fino en empate) a cada profundidad que piden los octantes, con la misma métrica que la GPU |
| E7 | Puerta D5: las máscaras de un nodo solo se aplican mientras tiene una profundidad activa `≥ rep`; hasta entonces sus hijos enmascarantes se retienen y esa profundidad se pide primero |

### Opción

`cellLod` vale `octant` (por defecto) o `cell`. Se fija con `?cellLod=` en la URL al arrancar o con la API del visor
(`setCellLod`); no hay control en la interfaz. Ver [`examples/web-host/README.md`](../../examples/web-host/README.md).

`cellActive` (experimental) vale `window` (por defecto: la ventana de un nivel por fotograma del ADR 0053) o
`contiguous` (la tirada residente contigua del rango, de golpe). Se fija con `?cellActive=` o con
`setCellActiveRule`. Es una opción temporal para que el propietario compare el zoom; tras su decisión se borra la
regla perdedora (OQ-11, issue #66).

### Límites conocidos

- Los conjuntos activos de la 2a pueden saltarse niveles (por ejemplo `{6, 13}`) hasta la fase 2b, que aplica la regla de
  un nivel de la [ADR 0053](../adr/0053-transiciones-de-un-nivel.md).
- `probe-cells-parity` compara el resumen del corte, no los píxeles; la comparación de píxeles a la misma pose (octante frente a celda) está en la sección «Navegador» del registro.
