# 0006 La política de versiones vive solo en `viewer_format`

Estado: Aceptada · Fecha: 28-10-2026

## Contexto
Los literales `FormatVersion::V<n>` repartidos por convert/wasm obligaban a tocar varios crates por revisión y permitían que un consumidor aceptara versiones sin backend.

## Decisión
`FormatVersion::DEFAULT_OUTPUT`, `FormatVersion::WRITABLE` y `supports_block_checkpoint()` (`version.rs`) son la única fuente; los demás crates usan esa API. `scripts/check_version_literals.sh` (paso de `scripts/check.sh`) falla si aparece `FormatVersion::V<n>` fuera de `crates/viewer_format/` salvo en tests.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Convención sin check | Se erosiona; el check es barato. |

## Consecuencias
Añadir una revisión no toca otros crates; los tests/ejemplos pueden usar literales.

## Revisar si
El check produce falsos positivos frecuentes o la API de capacidades deja de bastar.

## Evidencia
[version.rs](../../crates/viewer_format/src/version.rs); [CONTRIBUTING](../../CONTRIBUTING.md#convenciones)
