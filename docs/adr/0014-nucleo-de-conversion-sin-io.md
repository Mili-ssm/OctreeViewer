# 0014 Núcleo de conversión sin I/O compartido por CLI y navegador

Estado: Aceptada · Fecha: 29-10-2026

## Contexto
Hay dos conversores LAS/LAZ divergentes: `viewer_convert` (octree adaptativo, presupuesto de
memoria, bytes independientes del número de workers) y `viewer_wasm/src/browser_convert.rs`
(rejilla fija 2×2×2, otros tamaños de bloque y límites). El propietario exige paridad byte a byte
con el CLI en el navegador y escala Majadas.

## Decisión
Extraer la lógica del pipeline (plan espacial, partición, jerarquía, preparación, presupuesto) a un
crate `viewer_convert_core` sin `std::fs`, hilos, `Instant` ni `laz-parallel`, con traits
`SpoolStore`, `PointSource` y `Clock` y trabajos serializables. `viewer_convert` y `viewer_wasm` son
shells que implementan esos traits. Se llega por pasos: primero traits dentro de `viewer_convert`
con SHA idénticos, después el crate.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Mantener el conversor web duplicado | No alcanza paridad; dos planificadores que mantener. |
| `cfg`/features dentro de `viewer_convert` | Condicionales dispersos en ~1 800 líneas; no expresa trabajos para un pool de workers. |

## Consecuencias
Un único planificador; el núcleo compila para wasm32 (comprobado en `scripts/check.sh`). Coste:
división de la API pública de `viewer_convert` y una forma serializable de los bloques preparados en
`viewer_format`.

## Revisar si
El rendimiento nativo empeora más de 3% o los SHA nativo y wasm divergen.

## Evidencia
[Spec del conversor web paralelo](../plans/conversor-web-paralelo.md)
