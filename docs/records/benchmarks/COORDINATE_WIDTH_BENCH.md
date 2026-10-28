# Evaluación de coordenadas `u16`

## Pregunta y método

Se midió si una cuantización afín `u16` independiente por bloque puede reducir el payload de
coordenadas del formato actual, que almacena tres `u32` por punto. El ejemplo
[`quantization_width_bench.rs`](../../../crates/viewer_format/examples/quantization_width_bench.rs)
lee y verifica todos los bloques de una salida `.oct`, calcula min/max por eje y vuelve a
cuantizar a 16 bits. Si cualquier punto del bloque excede el error candidato permitido, todo el
bloque se cuenta para fallback a `u32`.

Ejecutar desde la raíz con la salida Belvedere descrita en
[`data/datasets/README.md`](../../../data/datasets/README.md):

```sh
cargo bench-quantization data/datasets/belv_2009_digital-aerial_pcd_copc.oct
```

El umbral predeterminado es 0,01 en unidades de la fuente; se puede sustituir con un segundo
argumento. El error se mide contra las posiciones `f32` reconstruidas por el lector actual, no contra
las coordenadas LAS originales. El ahorro asume 6 bytes menos por punto elegible y no incluye el
índice, transformaciones, fallback metadata ni los cambios que produciría LZ4.

## Resultado

Una ejecución local sobre Belvedere leyó 81.706.211 puntos en 1.343 bloques. Con candidato `u16` y
umbral 0,01:

| Medida | Resultado |
| --- | ---: |
| Máximo error incremental observado | 0,034913176 |
| Puntos que excedieron el umbral | 620 |
| Bloques que necesitarían fallback completo a `u32` | 19 de 1.343 |
| Puntos en bloques fallback | 1.594 |
| Puntos elegibles para `u16` según esta prueba | 81.704.617 |
| Ahorro teórico de coordenadas sin comprimir | 490.227.702 B |

La cifra de error excede 0,01, así que un `u16` uniforme no cumple el umbral. Un esquema híbrido con
fallback por bloque retendría precisión incremental bajo ese umbral en esta salida, pero no se ha
medido el error acumulado desde LAS ni el tamaño comprimido resultante. Por tanto no se propone aún
cambiar el formato: repetir la medición directamente desde la fuente y con otras distribuciones antes
de fijar representación, versión y metadatos de fallback.
