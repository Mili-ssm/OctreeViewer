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

Siguiente subtarea: prototipo de prioridad incremental con memoria contabilizada y comparación
contra esta base y cortes de datos reales; mantener la implementación actual hasta verificar
equivalencia y beneficio. El benchmark por sí solo no justifica una nueva API o un índice nuevo.

```sh
cargo run --profile perf -p viewer_core --example lod_frontier_bench --offline
cargo test -p viewer_core --test lod --profile fast --offline
```

Logs locales excluidos de Git: `target/audit-lod-frontier/run-{1,2,3}.txt` y `summary.json`.
