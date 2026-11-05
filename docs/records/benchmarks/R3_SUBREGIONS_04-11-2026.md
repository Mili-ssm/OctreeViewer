# R3: subregiones experimentales — 04-11-2026

**Observado:** las variantes con subregiones completaron los dos datasets y coincidieron en los conteos registrados; una sola ejecución por variante no demuestra una mejora general ni su causa. Es un experimento nativo, no una decisión de producto ni un cambio del ADR aceptado. La clave de 21 bits por eje **por región** es experimental.

## Identidad y método

- Equipo `pc-tatil`, Linux x86_64 7.2.15-300.fc45; revisión base `3eb78f70f2a90d5a8b7b68d39efc5fd406d2983d`, con cambios locales experimentales. Rust `1.101.0-nightly (41089d07a 2026-10-27)`, Cargo `1.101.0-nightly`, toolchain `nightly-2026-10-28`. El perfil exacto de compilación no consta en las identidades capturadas.
- `node_cells_bench` nativo sobre archivos v7.5 locales. Las identidades contienen argumentos, hashes de binario y de ambos archivos de cada dataset, tiempos UTC y estado de salida; `/usr/bin/time` registra tiempo y RSS. No se capturó una serie controlada ni condiciones de aislamiento.
- Orden: Belvedere subregiones, Majadas subregiones, Majadas adaptativo, Belvedere rápido, Majadas rápido. Una muestra de cada combinación; no comparar como ensayo causal ni extrapolar a otros equipos o datasets.
- [Datos y procedencia](data/r3-subregions-04-11-2026/): `*.identity`, `*.stdout` y `*.stderr` son copias de las capturas de ejecución; los tres parches comprimidos sin alterar su contenido son snapshots de cambios locales sobre la revisión base. Los `*sha256.txt` originales registran hashes de fuentes, binarios y datasets según su captura; `SHA256SUMS` se generó después de copiar y comprimir los parches y permite comprobar estas copias; `PATCH-SHA256SUMS` registra los hashes de los parches descomprimidos. Los binarios y datasets no están incluidos. Los snapshots identifican el código capturado; por sí solos no prueban cómo se compiló cada binario. Se revisaron las capturas y parches buscando nombres habituales de secretos antes de copiar; los archivos de identidad solo contienen las variables de entorno explícitamente mostradas.

## Resultados

| Dataset | Variante | Tiempo real (s) | RSS máximo (KiB) | Nodos | Celdas totales |
| --- | --- | ---: | ---: | ---: | ---: |
| Belvedere | subregiones | 93,68 | 71.148 | 3.941 | 232.396.472 |
| Belvedere | subregiones rápida | 54,21 | 70.832 | 3.941 | 232.396.472 |
| Majadas | subregiones | 634,75 | 230.368 | 24.091 | 1.384.873.569 |
| Majadas | adaptativa sin subregiones | 307,79 | 218.392 | 24.091 | 1.384.873.569 |
| Majadas | subregiones rápida | 365,06 | 230.376 | 24.091 | 1.384.873.569 |

Las dos variantes de subregiones tienen también idénticos conteos de celdas y regiones ocupadas por profundidad en cada dataset; esto no prueba igualdad de todo el contenido. Las cinco ejecuciones salieron con estado cero y comprobaron todos los IDs de jerarquía frente al recorrido paginado: 3.941 para Belvedere y 24.091 para Majadas. La variante adaptativa utiliza poda de la cadena de hojas; su salida conserva el detalle por factor de crecimiento.

Los totales suman nodos y niveles: no representan celdas simultáneamente residentes. La salida proyecta 16 bytes por instancia GPU: 3.718.343.552 bytes para Belvedere y 22.157.977.104 para Majadas; son sumas teóricas, **no memoria GPU medida**. Igualmente, los payloads de `NodeCell` de 80 bytes suman 18.591.717.760 y 110.789.885.520 bytes respectivamente, sin significar residencia simultánea. El RSS es el máximo del proceso durante cada ejecución; no equivale a esos payloads. Las salidas detallan regiones y profundidad.

## Corrección y límites

Según la ejecución de pruebas enfocadas del experimento, pasaron 24 pruebas de `node_cells` y una del ejemplo. Una revisión independiente del código examinó la tupla completa alta/baja, el límite de 21 bits y el origen no nulo; la revisión de pruebas no identificó un problema concreto. Estos resultados son distintos de una comparación exhaustiva de los datasets o de una prueba visual. No se verificaron GPU ni navegador; no hay evidencia de presentación visual ni de rendimiento en producción. La comprobación completa configurada no forma parte de este registro de medición.
