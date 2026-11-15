# 0025 Codificador LZ4 propio en `viewer_format`

Estado: Aceptada · Fecha: 30-10-2026

## Contexto
- `lz4_flex` 0.14 codifica distinto en wasm32 y en 64 bits (`block/hashtable.rs:26-50`, README prioridad 9).
- El proyecto vendoriza una bifurcación parcheada (`Cargo.toml` l.14 y l.43).
- Las SHA de salida y la paridad de bytes web/CLI (RQ-08) dependen de los bytes del codificador.

## Decisión
- Un codificador de bloques LZ4 determinista propio en `viewer_format`.
- El decodificador puede seguir siendo el de upstream.
- Se elimina la bifurcación vendorizada cuando el codificador propio esté dentro.

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Declarar la bifurcación como contrato | Los bytes del formato dependerían de un parche sobre código ajeno. |
| Enviar el parche a upstream | No controlamos plazo ni estabilidad de bytes entre versiones de upstream. |
| Cambiar de códec en V7 | Cambia el códec sin que el problema esté en el formato de bloque LZ4. |

## Consecuencias
- Si los bytes del codificador propio difieren de la bifurcación, las SHA de referencia R1 cambian; hacerlo antes del congelado de V7 ([ADR 0022](0022-alcance-y-congelacion-de-v7-final.md)) evita una regeneración extra.
- **Pendiente:** codificador, retirada de `vendor/lz4_flex` y comparación de bytes.

## Revisar si
Upstream publica un codificador con salida garantizada e idéntica entre arquitecturas.

## Evidencia
[estado](../estado.md#prioridades-pendientes) · [Requisitos CON-09](../requirements.md)

## Revisión 15-11-2026
El propietario decidió congelar V7 con la bifurcación vendorizada ([ADR 0044](0044-v7-final-congelada-y-predeterminada.md)). El codificador propio va después: no cambia el formato; cambia los bytes y las SHA de salida, pero los datasets ya escritos siguen siendo válidos sin regenerarlos.
