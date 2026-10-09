# Experimento de ramificación espacial

Estado: comparativa sintética completada; informa la selección futura de partición y LOD, pero no
fija la política de producción.

Repetir con:

```sh
cargo run -p viewer_format --release --example spatial_branching_bench
```

## Método

El benchmark genera 100.000 puntos deterministas en cinco distribuciones: uniforme aleatoria,
90 % concentrada en un cubo pequeño, plano, línea y rejilla regular de 100 × 100 × 10. En el modo
`point_cap`, particiona con 2, 4 y 8 divisiones por eje, crea solo celdas ocupadas y subdivide hasta
que cada hoja tenga como máximo 2.048 puntos. Conserva cada punto exactamente una vez.

También compara resoluciones espaciales equivalentes: dos niveles de 2 divisiones frente a un nivel
de 4 (`same_extent_4`), y tres niveles de 2 frente a un nivel de 8 (`same_extent_8`). En estas dos
comparaciones se fija la profundidad y no se aplica el límite de puntos por hoja. Así se aísla el
coste de jerarquía; algunas hojas densas superan 2.048 puntos.

Cada payload de hoja contiene tres coordenadas `u32` cuantizadas respecto a la celda y una intensidad
`u16` (14 bytes por punto); se usa el codec LZ4 de `viewer_format`. No incluye colores, otros
atributos, índice, cabeceras ni las muestras de LOD de nodos internos. Las consultas son 100 cajas
AABB deterministas de lado 0,1, centradas en puntos reales. El ratio de candidatos suma todos los
puntos de las hojas cuyas celdas intersectan la caja y lo divide por los puntos de origen; la
sobrecarga divide candidatos por los puntos realmente contenidos en la caja. No mide frustum, caché
ni latencia de lectura. El tiempo es construcción y codificación en memoria, no acceso a disco.

## Partición adaptativa por límite de hoja

| Distribución | Divisiones/eje | Nodos | Hojas | Profundidad | Puntos por hoja (mín/media/máx) | Ratio LZ4 | Candidatos media / p95 | Sobrecarga media / p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Uniforme | 2 | 73 | 64 | 2 | 1.492 / 1.562,5 / 1.640 | 1,0000 | 3,30 % / 6,34 % | 33,29 / 70,29 |
| Uniforme | 4 | 65 | 64 | 1 | 1.492 / 1.562,5 / 1.640 | 1,0000 | 3,30 % / 6,34 % | 33,29 / 70,29 |
| Uniforme | 8 | 513 | 512 | 1 | 154 / 195,3 / 234 | 1,0000 | 0,97 % / 1,60 % | 9,78 / 17,88 |
| Concentrada | 2 | 324 | 283 | 6 | 1 / 353,4 / 1.761 | 1,0000 | 52,94 % / 90,03 % | 3,24 / 17,60 |
| Concentrada | 4 | 1.049 | 1.032 | 3 | 1 / 96,9 / 404 | 1,0000 | 46,51 % / 81,90 % | 3,04 / 17,60 |
| Concentrada | 8 | 1.146 | 1.137 | 2 | 1 / 88,0 / 404 | 1,0000 | 46,48 % / 81,89 % | 1,79 / 6,50 |
| Plana | 2 | 169 | 128 | 3 | 697 / 781,2 / 846 | 1,0000 | 4,45 % / 6,36 % | 4,48 / 6,55 |
| Plana | 4 | 545 | 512 | 2 | 151 / 195,3 / 251 | 1,0000 | 2,42 % / 3,56 % | 2,43 / 3,60 |
| Plana | 8 | 129 | 128 | 1 | 697 / 781,2 / 846 | 1,0000 | 4,45 % / 6,36 % | 4,48 / 6,55 |
| Lineal | 2 | 505 | 442 | 6 | 1 / 226,2 / 1.595 | 1,0000 | 11,31 % / 12,47 % | 1,15 / 1,27 |
| Lineal | 4 | 463 | 442 | 3 | 1 / 226,2 / 1.595 | 1,0000 | 11,31 % / 12,47 % | 1,15 / 1,27 |
| Lineal | 8 | 451 | 442 | 2 | 1 / 226,2 / 1.595 | 1,0000 | 11,31 % / 12,47 % | 1,15 / 1,27 |
| Rejilla | 2 | 73 | 64 | 2 | 1.250 / 1.562,5 / 1.875 | 0,4374 | 3,56 % / 7,50 % | 43,44 / 92,59 |
| Rejilla | 4 | 65 | 64 | 1 | 1.250 / 1.562,5 / 1.875 | 0,4374 | 3,56 % / 7,50 % | 43,44 / 92,59 |
| Rejilla | 8 | 513 | 512 | 1 | 144 / 195,3 / 338 | 0,4546 | 0,97 % / 1,87 % | 11,92 / 23,15 |

## Resolución espacial equivalente

Cada par produjo las mismas celdas hoja, conteos por celda, bytes comprimidos y estadísticas de
consulta. La diferencia está en cuántos nodos intermedios conserva la jerarquía:

| Distribución | Nodos: 2 por eje × 2 niveles / 4 por eje × 1 nivel | Nodos: 2 por eje × 3 niveles / 8 por eje × 1 nivel |
| --- | ---: | ---: |
| Uniforme | 73 / 65 | 585 / 513 |
| Concentrada | 73 / 65 | 585 / 513 |
| Plana | 41 / 33 | 169 / 129 |
| Lineal | 31 / 23 | 81 / 51 |
| Rejilla | 73 / 65 | 585 / 513 |

El factor 4 reduce la profundidad en un nivel y elimina entre 8 nodos intermedios para esta
resolución. El factor 8 elimina entre 30 y 72 nodos respecto a tres niveles binarios. Esa reducción
abarata el índice sintético, mientras que una jerarquía binaria conserva más puntos donde podrían
ubicarse muestras LOD; el benchmark no mide el valor visual ni los bytes de esas muestras.

## Lectura y límites

- En los datos uniformes y la rejilla, el modo adaptativo con 8 mejora la selección espacial frente
  a 2/4, con ocho veces más hojas. La compresión de la rejilla empeora de 0,4374 a 0,4546.
- En datos concentrados, el límite de hoja obliga a subdividir de forma adaptativa. El factor 8
  reduce la sobrecarga de candidatos, pero no gana en cantidad de hojas ni en todas las distribuciones.
- No aparece un ganador universal. Los resultados muestran el coste de los factores y de la
  jerarquía; todavía falta comparar calidad de LOD y memoria fuera de núcleo.
- COPC, EPT y PCL usan octrees con hasta ocho hijos al bisecar los ejes ([COPC](https://copc.io/),
  [EPT](https://entwine.io/en/latest/entwine-point-tile.html),
  [PCL](https://pointclouds.org/documentation/tutorials/octree.html)). Schütz et al. estudian
  generación LOD fuera de memoria con octrees adaptativos y discuten los efectos de densidad desigual
  y localidad de entrada ([Computer Graphics Forum, 2020](https://doi.org/10.1111/cgf.14134)).

Medición en Linux x86-64, AMD Ryzen AI 9 365, Rust 1.99.0, perfil `release`. La construcción y
codificación en memoria tarda pocos milisegundos y no sirve para comparar rendimiento de producción.
El benchmark no usa LAS/LAZ reales, no incluye lectura/escritura en disco, RSS, índice del formato,
muestras internas, error geométrico, transiciones LOD ni calidad visual. La elección de producción
queda abierta hasta medir fixtures representativos, cobertura y error de representantes, bytes
totales, puntos leídos por consulta y memoria pico.
