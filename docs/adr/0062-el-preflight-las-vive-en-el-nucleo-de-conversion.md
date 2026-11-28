# 0062 El preflight LAS vive en el núcleo de conversión, no en el dominio

Estado: Aceptada · Fecha: 28-11-2026 · Decide: propietario · Precisa la ubicación del [ADR 0012](0012-validacion-previa-de-cabeceras-las-laz.md) (su decisión de validar antes de parsear no cambia)

## Contexto
- El ADR 0012 decide validar las cabeceras LAS/LAZ antes de que `las`/`laz` las parseen y sitúa la validación en
  `viewer_core::las::preflight_las`.
- Se puso en `viewer_core` el 29-10-2026 (`9d3933f`) para compartirla entre el CLI y el conversor del navegador. El
  [ADR 0014](0014-nucleo-de-conversion-sin-io.md) dio después a ambos un núcleo común, `viewer_convert_core`, que hoy es su
  único usuario en producción (`viewer_convert_core/src/source.rs`).
- `viewer_core` se declara sin I/O (`lib.rs:2-3`), pero el preflight lee con `Read + Seek` y obliga a `viewer_core` a
  depender de `laz` (feature `laz`).
- Revisión de arquitectura del 28-11-2026, hallazgo F5 ([plan](../plans/reorganizacion-bloque-2a.md), tarea 2).

## Decisión
El propietario (28-11-2026): **el preflight LAS (validación de cabecera, tabla de chunks LAZ y sus errores) vive en
`viewer_convert_core`**. `viewer_core` conserva lo que usan crates que no son de conversión: los descriptores de *extra
bytes* (`las/extra.rs`, que usa `viewer_format`), el esquema de atributos LAS y la escala visual. Si `viewer_core` deja de
usar `laz`, pierde esa dependencia y su feature.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Dejarlo en `viewer_core` | El motivo de ponerlo allí ya no existe; el dominio mantiene I/O y una dependencia de formato de entrada |
| Un crate propio de entrada LAS | Un crate más para ~900 líneas con un solo usuario |

## Consecuencias
- Los usuarios cambian de ruta: `viewer_convert_core/src/error.rs`, `viewer_convert/tests/las_preflight.rs` y el objetivo
  de fuzz `fuzz_las_preflight` (que pasa a depender de `viewer_convert_core`).
- Sin cambio de comportamiento ni de mensajes de error.
- El texto del ADR 0012 no se edita: su ruta `viewer_core::las::preflight_las` queda histórica y este ADR la precisa.

## Revisar si
Otro crate que no sea de conversión (el visor, `viewer_format`) necesita validar cabeceras LAS/LAZ.
