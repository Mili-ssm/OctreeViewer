# Resolución regional de representantes Replacement — 14-10-2026

Entrega parcial de la prioridad 1 del [README](../README.md#prioridades-pendientes).
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

La [regresión nativa](../crates/viewer_render_wgpu/tests/voxel_regions.rs) falla antes del cambio:
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
[migración anterior](RENDERER_DEPENDENCIES.md); esta entrega añade comprobación de sintaxis del
nuevo probe, sin atribuirle una nueva pasada de todas las suites JS/Python.

[Probe WebGPU](../examples/web-host/tests/lod-regions.browser.mjs), ejecutado con Playwright MCP:
parent fallback y tres vóxeles en el corte mixto, reconstrucción tras resize, llegada del hijo,
cuatro vóxeles y cobertura completa. La cámara explícita permite comprobar la rama fina incluso
cuando el escalonado sigue en 16 px esperando bloques. Tras llegar el hijo se invalida la cámara
para reconsiderar el objetivo ya residente, sin cambiar la política de escalonado del visor.

[Resultado de los probes y contraejemplos](benchmarks/voxel-regions-browser-2026-10-14.json).
Se activó WebGPU con `--enable-unsafe-webgpu --use-webgpu-adapter=swiftshader
--enable-unsafe-swiftshader --use-angle=swiftshader`; `requestAdapter` confirmó arquitectura
`swiftshader`. La prueba de atributos conserva sus 4.096 vóxeles. La prueba separada de fallback
por presupuesto no completa la solicitud de adapter. El clear JavaScript independiente vuelve
a perder el dispositivo con `A valid external Instance reference no longer exists.`
La consola de página registra además un 404 de recurso al abrir la raíz del servidor.
Esta evidencia funcional por software no cierra presentación visual, GPU física, móvil ni FPS.

## Comparación nativa reproducible

[Datos completos, comandos, hashes y muestras](benchmarks/voxel-regions-belvedere-2026-10-14.json).
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

### Comprobación adicional — 24-10-2026

Se añadió `slow_and_unavailable_multiblock_loads_preserve_renderable_cuts` a
[`voxel_regions.rs`](../crates/viewer_render_wgpu/tests/voxel_regions.rs). La prueba combina
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

La prioridad 1 sigue abierta: retener la representación anterior **por región** entre generaciones,
la secuencia hijos ocupados con color heredado/atributos propios, cargas lentas/fallidas reales,
cámara/zoom y validación visual multibloque. `sync_visible_blocks` sigue siendo un consumidor
separado de esta agregación. La mejora regional actual no certifica esos contratos pendientes.
