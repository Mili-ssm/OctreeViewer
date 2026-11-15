# 0007 El lector aplica límites por defecto configurables

Estado: Aceptada (30-10-2026) · Fecha: fecha no registrada

## Contexto
Las cabeceras y el índice declaran tamaños controlados por el archivo; sin tope, un archivo corrupto o hostil reserva memoria arbitraria.

## Decisión
`FormatReaderLimits` (`crates/viewer_format/src/reader_limits.rs`): 256 atributos, 65 535 componentes, 1 MiB por cadena, 256 MiB de índice, 256 MiB por bloque, 2 000 000 nodos y 2 000 000 bloques. Esquemas mayores se abren con `FormatReader::with_limits`. Pregunta abierta: ¿debe el writer rechazar esquemas que el lector por defecto no puede abrir?

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Sin límites | Reserva ilimitada ante archivos hostiles. |
| Writer rechaza esquemas >256 | Aún sin decidir: evita archivos que el lector por defecto no abre, pero impone un tope al formato, que sí admite esquemas anchos. |

## Consecuencias
Los archivos válidos grandes necesitan opt-in. El test `wide_schema_requires_explicit_reader_limits` (`crates/viewer_format/src/writer_tests.rs`) cubre un esquema de 257 atributos en todas las revisiones de `FormatVersion::WRITABLE`: el writer lo acepta, los límites por defecto lo rechazan (`CountLimit`) y límites propios lo abren.

## Revisar si
Se decide la política de compatibilidad por defecto, o un corpus real supera algún límite.

## Evidencia
[reader_limits.rs](../../crates/viewer_format/src/reader_limits.rs); [inventario](../reference/inventory.md#límites)

## Revisión 30-10-2026
Aceptada por el propietario. Pregunta abierta resuelta: el escritor rechaza esquemas de más de 256 atributos con un mensaje claro (RF-27 de [requisitos](../requirements.md)). **Pendiente:** hoy el escritor los acepta (test `wide_schema_requires_explicit_reader_limits`); hay que cambiar el escritor y ese test. La columna de alternativas conserva su texto original.

## Revisión 15-11-2026
El escritor ya rechaza esquemas de más de 256 atributos (`FormatError::TooManyAttributes`, RF-27). El test `wide_schema_requires_explicit_reader_limits` pasó a llamarse `wide_schema_is_limited_to_the_default_reader_limit` (`crates/viewer_format/src/writer_tests.rs`); el test de integración `writer_rejects_more_than_256_attributes` (`crates/viewer_format/tests/v7_final.rs`) cubre el mismo rechazo.
