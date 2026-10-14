# Localidad de planos de bytes: experimento cerrado — 14-10-2026

Decisión: **conservar el recorrido columnar actual**. Los candidatos row-first y tile256
mantienen los bytes, pero no mejoran los tamaños/anchuras habituales en este experimento.
No se ha modificado el encoder ni el decoder de planos de bytes del producto.

## Protocolo

`byte_plane_bench` compara tres kernels en ambas direcciones:

- `column`: byte exterior / fila interior, como el codec actual.
- `row`: fila exterior / byte interior.
- `tile256`: bloques de 256 filas, byte exterior / fila interior dentro de cada bloque.

Anchuras 3/6/8/16 bytes; 262.144 y 1.048.576 filas; nueve repeticiones por caso, rotando el
orden de las tres estrategias. Perfil nativo `perf`, datos deterministas, misma entrada por
forma. Se mide asignación/puesta a cero de salida y transposición; igualdad completa y CRC
quedan fuera del tiempo. No se incluye LZ4, selección de candidatos, parsing, disco ni transporte.
La carga del host/caché no se aísla; código del ejemplo y optimizaciones del compilador pueden
diferir del codec integrado. No mide RSS ni velocidad de conversión/carga del viewer.

432 transformaciones verifican igualdad de bytes frente al control. Tests adicionales cubren
un layout conocido, ida/vuelta, cero/una fila, 31/255/256/257 filas y anchuras hasta 257 bytes,
para comprobar colas de tile y formas rectangulares.

## Resultados

Medianas en milisegundos con el tamaño habitual de 262.144 filas:

| Anchura | Encode column | Encode tile256 | Decode column | Decode tile256 |
| --- | ---: | ---: | ---: | ---: |
| 3 | 0,473 | 0,550 | 0,313 | 0,366 |
| 6 | 0,636 | 0,739 | 0,636 | 0,740 |
| 8 | 0,850 | 0,983 | 0,855 | 0,988 |
| 16 | 1,722 | 1,981 | 1,231 | 1,393 |

Tile256 empeora los ocho casos de esta tabla. Row-first también es más lento en todos los
casos medidos. Para 1.048.576 filas, column sigue ganando con anchuras 3/6/8. Solo a 16 bytes
gana tile256: encode 7,838 → 6,388 ms (−18,5 %), decode 7,188 → 6,110 ms (−15,0 %).

Ese caso favorable supera el límite actual de 1.000.000 puntos por bloque del conversor y
no acredita una mejora en bloques admitidos, archivos reales o WASM. El contrato del escritor
es independiente; podrían existir otros consumidores, pero no se han medido aquí. No se
introduce una regla por tamaño/anchura basándose solo en este microbenchmark.

[Muestras, checksums, rangos, binario y comando](benchmarks/storage-v6-byte-plane-kernels-2026-10-14.json).

```sh
cargo run --profile perf -p viewer_format --example byte_plane_bench -- 9
cargo test -p viewer_format --example byte_plane_bench
```

## Siguiente criterio de cierre

Si aparece un consumidor real con columnas anchas y bloques grandes, medir alrededor de
sus tamaños reales (incluyendo límites/colas) y comparar parejas completas y decoding selectivo
antes de integrar un kernel alternativo. Para la ruta actual, priorizar medición selectiva de
XYZ/RGB/atributo y costes de transporte; no añadir complejidad a planos que aquí ya son más rápidos.
