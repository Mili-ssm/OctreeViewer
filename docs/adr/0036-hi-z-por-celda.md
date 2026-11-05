# 0036 Hi-Z por celda en dos fases sobre el pool

Estado: Aceptada · Fecha: 05-11-2026 · Decide: propietario · Enmienda a [ADR 0035](0035-instancia-de-16-b-con-w-16.md) (técnica de oclusión de la Tarea 16)

## Contexto
- El [ADR 0035](0035-instancia-de-16-b-con-w-16.md) decidió compactar en GPU y recortar por oclusión «con Hi-Z por página», antes de la Tarea 6.
- La Tarea 16a lo midió sobre el spike de GPU ([registro](../records/benchmarks/R3_T16_OCCLUSION_05-11-2026.md)), con la caja ajustada de cada página y en seis direcciones dentro de una hoja. En el peor caso (Majadas mirando a +y, 4 px):

  | Variante | GPU |
  | --- | ---: |
  | `draw` por rango (Tarea 5) | 21,66 ms |
  | compactación sin oclusión (`k`) | 7,62 ms |
  | `k` + Hi-Z por página (`kp`) | 7,69 ms |
  | `kp` + Hi-Z por celda (`kpc`) | 4,39 ms (p95 en movimiento, 8,27 ms) |

- **El Hi-Z por página no aporta por sí solo.** *Inferido:* las páginas abarcan demasiados píxeles. Lo que recorta es la prueba por celda dentro de cada página.

## Decisión
- La técnica de la Tarea 16 es **`kpc` tal como se midió:**
  - pase de páginas con frustum y Hi-Z sobre la caja ajustada;
  - pase de huecos, con un grupo de trabajo por página, que descarta vacíos, máscara 8³, octantes de otro nivel, frustum y, en la fase 2, Hi-Z por celda;
  - oclusión en dos fases con bits de visibilidad en doble búfer;
  - un `drawIndexedIndirect` por fase, sobre índices absolutos de hueco.
- `PageEntry` (Tarea 6) lleva la caja ajustada de los huecos usados.
- No se pasa al raymarching (OQ-08).

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Hi-Z solo por página, como decía 0035 | +0,9 % frente a no tener oclusión |
| Solo compactación (`k`) | Cumple los 16,7 ms, pero `kpc` es un 42 % más rápido (−3,23 ms) en el caso que decide |
| Quitar la prueba Hi-Z del pase de páginas | *Inferido* neutral (`kp` ≈ `k`), pero no se midió esa combinación. Solo se quita si la Tarea 16b lo mide |

## Consecuencias
- La Tarea 16b lleva estos pases a `hzb.rs` dentro de la Tarea 7.
- **Límites:** el pase de huecos usa 8 búferes de almacenamiento, y el pool de 256 MiB ([ADR 0033](0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md)) no cabe en un binding de 128 MiB. `gpu.rs` pide hoy `downlevel_defaults` (4 por etapa). La ruta de celdas pide ≥ 8 búferes por etapa, que es el mínimo del estándar WebGPU, y el tamaño de binding y de búfer del pool, sin pasar de lo que da el adaptador. Si el adaptador no llega, se usa la ruta CPU (R3-Q5).
- La Tarea 16b debe derivar el margen de profundidad del Hi-Z de una cota de error; en el spike, 4 ulp solo están validados empíricamente.

## Revisar si
- La Tarea 16b o la 17 superan 16,7 ms en Majadas dentro de una hoja a 4 px → raymarching (OQ-08).
- Con los límites de producción, el pase de huecos no cabe sin partirlo, y partirlo cuesta más de lo que recorta el Hi-Z por celda.
