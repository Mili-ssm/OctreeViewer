# Experimento de ramificación espacial

Estado: comparativa sintética para explorar la partición; todavía no fija la política de producción.

Repetir con:

```sh
cargo run -p viewer_format --release --example spatial_branching_bench
```

## Método

El benchmark genera 100.000 puntos deterministas en cinco distribuciones: uniforme aleatoria,
90 % concentrada en un cubo pequeño, plano, línea y rejilla regular de 100 × 100 × 10. Repite la
partición con 2, 4 y 8 divisiones por eje, creando solo celdas ocupadas. Las hojas tienen un máximo
de 2.048 puntos y se conserva cada punto exactamente una vez.

Para comparar la compresión de forma reproducible, cada payload de hoja contiene tres coordenadas
`u32` cuantizadas respecto a la celda y una intensidad `u16` (14 bytes por punto); se usa el codec
LZ4 de `viewer_format`. No incluye colores, otros atributos, índice, cabeceras ni las muestras de
LOD de los nodos internos. Las consultas son 100 cajas AABB deterministas de lado 0,1; el ratio
cuenta todos los puntos de las hojas cuyas celdas intersectan la caja. No es una medición de
frustum, caché ni latencia de lectura.

## Resultados

| Distribución | Divisiones/eje | Nodos | Hojas | Profundidad | Puntos por hoja (mín/media/máx) | Ratio LZ4 | Candidatos consulta media / p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Uniforme | 2 | 73 | 64 | 2 | 1.492 / 1.562,5 / 1.640 | 1,0000 | 3,60 % / 6,31 % |
| Uniforme | 4 | 65 | 64 | 1 | 1.492 / 1.562,5 / 1.640 | 1,0000 | 3,60 % / 6,31 % |
| Uniforme | 8 | 513 | 512 | 1 | 154 / 195,3 / 234 | 1,0000 | 1,08 % / 1,60 % |
| Concentrada | 2 | 324 | 283 | 6 | 1 / 353,4 / 1.761 | 1,0000 | 0,35 % / 0,66 % |
| Concentrada | 4 | 1.049 | 1.032 | 3 | 1 / 96,9 / 404 | 1,0000 | 0,30 % / 0,66 % |
| Concentrada | 8 | 1.146 | 1.137 | 2 | 1 / 88,0 / 404 | 1,0000 | 0,11 % / 0,17 % |
| Plana | 2 | 169 | 128 | 3 | 697 / 781,2 / 846 | 1,0000 | 1,02 % / 6,17 % |
| Plana | 4 | 545 | 512 | 2 | 151 / 195,3 / 251 | 1,0000 | 0,38 % / 3,48 % |
| Plana | 8 | 129 | 128 | 1 | 697 / 781,2 / 846 | 1,0000 | 1,02 % / 6,17 % |
| Lineal | 2 | 505 | 442 | 6 | 1 / 226,2 / 1.595 | 1,0000 | 0,07 % / 0,07 % |
| Lineal | 4 | 463 | 442 | 3 | 1 / 226,2 / 1.595 | 1,0000 | 0,06 % / 0,06 % |
| Lineal | 8 | 451 | 442 | 2 | 1 / 226,2 / 1.595 | 1,0000 | 0,05 % / 0,06 % |
| Rejilla | 2 | 73 | 64 | 2 | 1.250 / 1.562,5 / 1.875 | 0,4374 | 3,54 % / 7,50 % |
| Rejilla | 4 | 65 | 64 | 1 | 1.250 / 1.562,5 / 1.875 | 0,4374 | 3,54 % / 7,50 % |
| Rejilla | 8 | 513 | 512 | 1 | 144 / 195,3 / 338 | 0,4546 | 1,02 % / 1,87 % |

Medición en Linux x86-64, AMD Ryzen AI 9 365, Rust 1.99.0, perfil `release`. El tiempo de
construcción y codificación osciló entre 3 y 15 ms para estos datos; esa duración es demasiado corta
para tratarla como comparación de rendimiento.

## Lectura

- Con datos uniformes y la rejilla, 4 divisiones por eje produce las mismas hojas, los mismos
  candidatos y la misma compresión que dos niveles de 2 divisiones por eje; omite 8 nodos
  intermedios y reduce la profundidad en uno. Esos nodos intermedios podrían servir como niveles LOD.
- 8 divisiones por eje mejora la selección espacial en el caso uniforme y la rejilla, pero usa ocho
  veces más hojas en esos casos. En la rejilla, el ratio almacenado sube de 0,4374 a 0,4546.
- En datos concentrados, planos y lineales cambia la ocupación y el reparto de puntos de forma
  distinta. La densidad y la alineación de la consulta con las celdas importan; ningún factor gana en
  todos los resultados.
- La especificación COPC y EPT y la documentación de PCL usan octrees con hasta ocho hijos, producto
  de bisecar cada eje ([COPC](https://copc.io/), [EPT](https://entwine.io/en/latest/entwine-point-tile.html),
  [PCL](https://pointclouds.org/documentation/tutorials/octree.html)). El trabajo de Schütz et al.
  estudia generación LOD fuera de memoria con octrees adaptativos y advierte que densidad desigual y
  localidad de entrada afectan el coste ([Computer Graphics Forum, 2020](https://doi.org/10.1111/cgf.14134)).

Estos resultados no miden muestras internas, error geométrico, calidad visual, paso de una selección
LOD a otra, archivos LAS reales ni acceso desde disco. Por tanto, no justifican aún reemplazar el
octree convencional. Para elegir la política hacen falta fixtures LAS/LAZ representativos y pruebas
que midan cobertura/error de representantes, bytes totales comprimidos, puntos cargados por consulta
y estabilidad en densidades desiguales.
