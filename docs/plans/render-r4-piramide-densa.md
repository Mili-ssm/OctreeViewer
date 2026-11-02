# R4 — Pirámide LOD densa y octree global: plan de implementación

Estado: activo · 01-11-2026

> **Para agentes:** sub-skill obligatoria: superpowers:subagent-driven-development (recomendada) o
> superpowers:executing-plans, tarea a tarea. Los pasos usan casillas (`- [ ]`).

**Objetivo:** nueva revisión `v7.4-pre` con las coordenadas como enteros globales de cuanto 2⁻¹⁵ y un
octree implícito en sus bits ([ADR 0029](../adr/0029-octree-global-por-bits-de-enteros.md)). La pirámide
divide el espaciado entre 2 en cada nivel hasta hojas pequeñas, con el espaciado explícito por nodo. El CRS
se conserva completo.

**Por qué:** las hojas de hasta 262 144 puntos aplastan varios niveles de densidad en uno (B2): se leen
1 100–2 700 puntos por vóxel dibujado, Belvedere no llega a 4 px en 300 s y la cadena de hoja de R3 llega
al tope en el 78–99 % de las hojas ([registro R3-T1](../records/benchmarks/RENDER_R3_T1_NODE_CELLS_01-11-2026.md)).

**Spec:** [ADR 0020](../adr/0020-piramide-lod-densa.md), [ADR 0021](../adr/0021-rejilla-global-determinista-y-crs.md)
precisada por [ADR 0029](../adr/0029-octree-global-por-bits-de-enteros.md), [ADR 0022](../adr/0022-alcance-y-congelacion-de-v7-final.md),
[ADR 0028](../adr/0028-r4-antes-que-r3.md); [requisitos](../requirements.md) RF-02, RF-21, RF-23, RF-24,
RQ-02, RQ-07, RQ-08, RQ-09.

**Progreso (01-11-2026):** redactado y enmendado tras su pre-mortem. Ninguna tarea empezada.

## Resoluciones del propietario (01-11-2026)

| Id | Resolución |
| --- | --- |
| R4-Q1 | Coordenadas enteras globales con cuanto binario 2⁻¹⁵ y octree implícito en sus bits; se omite la cadena superior de nodos con un solo hijo ([ADR 0029](../adr/0029-octree-global-por-bits-de-enteros.md)) |
| R4-Q2 | El límite de puntos por hoja se fija **midiendo** 16, 32 y 64 k (Tarea 1) |
| R4-Q3 | «Un nivel por ×2 de densidad» = **el espaciado se divide entre 2 en cada nivel** (como Potree/COPC) |
| R4-Q4 | La reproyección con PROJ (RF-22) queda **fuera** de `v7.4-pre`: es una opción posterior del CLI que no cambia el formato |

## Lectura del código (verificada por el pre-mortem, 01-11-2026)

- Los nodos internos guardan 8³ representantes (`SAMPLE_GRID_SIZE`, `viewer_convert_core/src/spatial.rs:32`)
  en un `SampleGrid` denso de g³ entradas (`pipeline/node_job.rs:123-131`). Se parte solo con más de
  262 144 puntos (`options.rs:5`, `node_job.rs:45`). Con `spatial_divisions` = 2 el lado del hijo es la
  mitad; también se admiten 4 y 8 (`options.rs:11`).
- En datos aéreos (2,5D), el espaciado de una hoja de N puntos es ≈ lado/√N y el del padre es 2·lado/g. Que
  queden a un nivel exige g ≈ √N, es decir, 128–256 para hojas de 16–64 k (inferido). La pirámide densa
  necesita **muchos más representantes por nodo interno**, no solo hojas pequeñas.
- Nodos: lado del cubo de la caja de la cabecera (`cube_bounds`, `spatial.rs:1953`) y celdas LOD relativas
  a él (`spatial.rs:2000`). Marco: origen en el mínimo de la cabecera (`source.rs:125`). La cuantización es
  `u32` por bloque, con origen en el mínimo local (`viewer_format/src/quantization.rs:3,96-134`).
- Visor y conversor asumen «8 por eje» en más sitios que `side / 8`: `node_spacing` (`viewer_wasm/src/lod_dataset.rs:863`,
  usado en :541, :586, :604 y :774), `lod_transition.rs:427`, `node_cells.rs:36`, `voxel_grid.rs:86`,
  `sampling_policy` = "8x8x8" (`spatial.rs:535`) y `geometric_error` (`spatial.rs:1063`).
- El CRS es un `Option<String>` (`viewer_format/src/metadata.rs:12`); el conversor LAS lo descarta
  (`source.rs:18,166`).
- Límites: el escritor admite 1 M bloques (`options.rs:13`), y ese límite llega antes que los 2 M nodos del
  lector. Hoy hay 6259 nodos en Majadas; ×5–10 queda lejos de los dos límites.
- ADR 0016 fija la norma (`norm3`), no un error máximo: la cota la da `--max-error` (0,01 por defecto, `options.rs:161`).

## Restricciones globales

- `scripts/check.sh` en verde al cerrar cada tarea; sin cambiar el nightly.
- `FormatVersion::V<n>` solo dentro de `viewer_format` (`scripts/check_version_literals.sh`).
- V5 sigue siendo la salida por defecto hasta el congelado (ADR 0002); v5 y v7.1–3-pre siguen leyéndose y
  mantienen sus digests dorados.
- `v7.4-pre` fija `spatial_divisions` = 2; el límite de hoja y `sample_grid` salen de la versión, no de
  opciones libres. EPT/Additive con `--format v7.4-pre` → error explícito (D2 de R3).
- Sin `log2` en las rutas de formato (ADR 0016): los exponentes salen de los bits de `f64` y se prueban en wasm32.
- Paridad web/CLI byte a byte (RQ-08).
- Registros fechados en `docs/records/benchmarks/`; spec vigente en `docs/reference/format/`; commits
  convencionales en inglés con `Co-Authored-By`; sin push.

## Criterios de parada

- Tarea 1: ningún g ≤ 256 deja el salto de espaciado hoja/padre en ≤ 1 nivel en p95 → volver al
  propietario (ADR 0020).
- Tarea 1: ninguna combinación cumple disco ≤ V5 + 35 % y conversión ≤ +30 % (RQ-07) → volver al
  propietario antes de la Tarea 3.
- Tarea 2: Majadas o Belvedere no identifican su CRS por autoridad y código (EPSG) → definir RF-02 antes de la Tarea 5.
- Tarea 3: error medido > `--max-error` en un fixture a 4,4·10⁶ m, o un nodo que no cabe en `u32` sin partirse → parar.
- Tarea 4b: conversión > +30 % o RSS > `DEFAULT_MEMORY_BUDGET_BYTES` → parar antes de la Tarea 5.
- Cualquier diferencia de bytes web/CLI en fixtures `v7.4-pre` → parar (RQ-08).

---

## Tarea 1: spike de densidad (R4-Q2)

**Archivos:** `crates/viewer_convert_core/src/{options.rs, spatial.rs, pipeline/node_job.rs}`;
`crates/viewer_convert/examples/density_spike.rs`.

- [x] `SampleGrid` disperso (claves ordenadas) y `sample_grid` como parámetro interno, por defecto 8. La
  salida actual no cambia: los tests y las SHA de los fixtures siguen iguales.
- [x] Definir e implementar el espaciado de hoja: lado/2^d, con d el primer nivel en el que las celdas
  ocupadas crecen menos de ×1,5.
- [x] **Regla de elección, escrita antes de medir:** mínimo disco con salto hoja/padre ≤ 1 nivel en p95 y
  RSS ≤ presupuesto.
- [x] Convertir Majadas y Belvedere (v7.3-pre) con hoja {16, 32, 64} k × `sample_grid` {32, 64, 128, 256}.
  Medir solo el conversor: disco frente a V5, tiempo y RSS, nodos, profundidad, puntos por hoja (p50/p95) y
  salto de espaciado. El bench de R3-T1 se repite en la Tarea 6.
- [x] Registro `R4_DENSITY_SPIKE_<fecha>.md`. **Puerta del propietario:** fija el límite de hoja y `sample_grid`.

Resultado (02-11-2026): hoja 64 k y `sample_grid` = 256 (propietario); Majadas +24 % de tiempo, 207 MB de RSS, +18 % de disco frente a V5 ([registro](../records/benchmarks/R4_CONVERT_TIME_2_02-11-2026.md)).

## Tarea 2: spec de `v7.4-pre`

**Archivos:** crear [`docs/reference/format/FORMAT_V7_PRE4.md`](../reference/format/FORMAT_V7_PRE4.md) (a partir de `FORMAT_V7_PRE3.md`); borrador escrito el 02-11-2026, pendiente de revisión.

- [ ] Marco (ADR 0029): `q = round(c × 2^15)` en `i64`; raíz guardada = primer nodo con más de un hijo
  (nivel y prefijo); bloques con desplazamientos `u32` respecto al origen de su nodo; los nodos de más de
  2^32 cuantos se parten.
- [ ] Por nodo: nivel absoluto, prefijo y `spacing` (f64; en hoja, el espaciado medido). `geometric_error`
  se conserva para el lector.
- [ ] CRS: WKT/GeoTIFF completo (RF-24). Identidad = autoridad + código (EPSG del WKT o de la GeoKey
  3072/2048), y bytes solo si no hay código. Sin CRS, la nube es «local» y no se mezcla (RF-02). Escala y
  offset LAS de origen para la exportación exacta.
- [ ] Flags LAS y Extra Bytes (RF-24): se decide aquí si van en `v7.4-pre`.
- [ ] Comprobar el CRS real de Majadas y Belvedere (`las::Header::vlrs()`).
- [ ] Spec revisada con `doc-curator` antes de la Tarea 3.

## Tarea 3: `FormatVersion::V7_PRE4` en `viewer_format`

**Archivos:** `crates/viewer_format/src/{version.rs, layout.rs, index_codec.rs, metadata.rs, quantization.rs}`, tests y fuzz.

- [ ] Tests primero: ida y vuelta del índice (prefijo, nivel, `spacing`, CRS); cuantización exacta con
  origen a 4,4·10⁶ m; exportación que devuelve los enteros LAS exactos con escalas 0,01, 0,001 y 0,0001;
  corrupción (`tests/corruption.rs`); v5 y v7.1–3-pre sin cambios.
- [ ] Implementar; V7_PRE4 en `WRITABLE`, con V5 como `DEFAULT_OUTPUT`. Añadir v7.4 al corpus de
  `fuzz_format_reader` y SHA de referencia R1 para v7.4.

## Tarea 4: conversor denso sobre el octree global

**Archivos:** `crates/viewer_convert_core/src/{source.rs, spatial.rs, pipeline/node_job.rs, options.rs}`,
`crates/viewer_convert/src/{main.rs, checkpoint.rs}`, `crates/viewer_wasm/src/core_convert.rs`.

- [ ] Leer el CRS y la escala/offset de los VLR y escribirlos.
- [ ] Con `--format v7.4-pre`: nodos por prefijo de bits, raíz = primera división real, límite de hoja y
  `sample_grid` de la Tarea 1, `spacing` por nodo y `sampling_policy` generado. Manifiesto de checkpoint
  con versión nueva y los parámetros nuevos.
- [ ] Tests: dos conversiones del mismo fixture dan los mismos bytes; dos recortes solapados de una nube
  comparten prefijos de nodo y celdas en la zona común (RF-23); una caja que cruza 2^22 no añade niveles
  vacíos; paridad web/CLI (RQ-08).

## Tarea 4b: medida intermedia de conversión

- [ ] Majadas en `v7.4-pre`: tiempo, RSS y disco de conversión frente a V5 (criterio de parada).

## Tarea 5: el visor sobre el octree global

**Archivos:** `crates/viewer_wasm/src/lod_dataset.rs`, `crates/viewer_core/src/lod_transition.rs`,
`crates/viewer_render_wgpu/src/{voxel_grid.rs, node_cells.rs}` y examples.

- [ ] Inventario con `rg 'side / 8|/ 8\.0|8\^3|SAMPLE_GRID_SIZE'`; todos los usos pasan a derivar del
  `spacing` del índice cuando existe, y conservan `side / 8` para v5 y v7.1–3-pre.
- [ ] `representative_depth` derivada de `spacing` y relativa a la raíz guardada; origen de render = prefijo de la raíz.
- [ ] Dorados nuevos para v7.4; los dorados de los formatos existentes no cambian.
- [ ] El visor rechaza una nube con otra identidad de CRS y nombra ambas (RF-02).

## Tarea 6: medida y regeneración

- [ ] Regenerar Majadas y Belvedere en `v7.4-pre`; matriz R0 (arnés de T7) frente al
  [registro T7](../records/benchmarks/RENDER_R1_T7_01-11-2026.md).
- [ ] **Pasa si:** disco ≤ V5 + 35 %, conversión ≤ +30 % (RQ-07), y nodos sin cubrir y píxeles vacíos sin
  regresión (RQ-02). Si no pasa, parar antes de reanudar R3.
- [ ] Repetir el bench de R3-T1. Si menos del 1 % de las hojas llegan al tope, R3 se reanuda (ADR 0028).

## Fuera de este plan

Reproyección con PROJ (R4-Q4), fusión incremental de nubes (RF-21, que el octree global habilita), jerarquía
paginada (R5), multinube (RF-01) y congelado de V7 final (ADR 0022).
