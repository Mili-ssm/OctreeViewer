# R3: línea base del modo celda antes de retirar el modo octante — 18-11-2026

**Estado: medido; sirve de referencia, no de resultado.** Fija lo que hace hoy el modo celda (`LodMode::Cell`) para
que las tareas del [plan de retirada](../../plans/retirada-del-modo-octante.md) (ADR 0056) demuestren que borrar el
modo octante no lo cambia: los seis tests `real_sim` de Belvedere (tres en modo celda, con cuatro simulaciones), el golden de `cell_cut_ranged` y la lista de
tests de `viewer_render_wgpu` y `viewer_wasm`.

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | `4116b38` (rama `refactor/retire-octant`; añade el golden de celda, sin cambios de lógica) |
| Perfil y toolchain | `fast` · rustc 1.101.0-nightly (2026-10-27) |
| Equipo | AMD Ryzen AI 9 365 w/ Radeon 880M |
| Dataset | `target/render-data/belvedere-v7.oct` |
| Comando de las simulaciones | `R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile fast --offline --lib -- --ignored real_sim --nocapture` |
| Comando de la lista de tests | `cargo test -p viewer_render_wgpu -p viewer_wasm --profile fast --offline --lib -- --list 2>/dev/null \| grep ': test$' \| sort` |
| Datos | [`data/retirada-octante-18-11-2026/`](data/retirada-octante-18-11-2026/): `real_sim.txt`, `tests-before.txt` |

## Resultado

### Simulaciones reales en modo celda

Valores copiados de las líneas `Cell:` y `digest` de `real_sim.txt`. En las cuatro, `missing_reports`,
`zone coarsenings`, `unexcused` (caídas bajo el deseo sin excusa) y `lost samples` valen 0, y `cell_within_node`,
`cell_border_visible_over_one`, `hops` y `octant_jumps` también (los dos últimos solo cuentan en octante).

| Simulación | `updates` | `cell_node_changed` | `cell_border_pairs` | `cell_border_over_one` | `cell_border_visible_pairs` | Resumen (`digest`) |
| --- | --- | --- | --- | --- | --- | --- |
| dolly, semilla 3 | 75 | 827 | 4540 | 1074 | 0 | 8689825059067788954 |
| orbit, semilla 1 | 98 | 2166 | 17741 | 2627 | 49 | 7011128788589610482 |
| orbit, semilla 7 | 98 | 343 | 12240 | 1857 | 27 | 7011128788589610482 |
| orbit+inject, semilla 5 | 100 | 1282 | 12060 | 1894 | 27 | 7011128788589610482 |

El `digest` de cada línea coincide con el «in-process» que imprime a su lado. Los otros contadores de `SimStats`
(mensajes, evicciones, reinicios del pool) están en `real_sim.txt`.

### Tests

| Crate | Pasan | Ignorados | Fallan |
| --- | --- | --- | --- |
| `viewer_render_wgpu` | 410 | 3 | 0 |
| `viewer_wasm` | 185 | 20 | 0 |

`tests-before.txt` tiene 618 nombres (413 + 205, ignorados incluidos). Los 410 de `viewer_render_wgpu` incluyen
`cell_cut_digest_golden_in_cell_mode`, nuevo en esta rama (409 antes).

## Cómo comparar después

- **Simulaciones**: repetir el comando y comparar sin las líneas variables, que son las de «has been running», las
  de `test result … finished in` y el orden (los seis tests corren en paralelo y las líneas se entrelazan):
  `diff <(grep -v -E 'has been running for over|^test result|^$' NUEVO | sort) real_sim.txt`. Dos ejecuciones
  seguidas de esta revisión coincidieron salvo en esas líneas.
- **Tests**: `comm -3 tests-before.txt <(lista nueva ordenada)`; solo deben aparecer los nombres que el plan
  borra o añade a propósito.
- **Golden del corte**: `cargo test -p viewer_render_wgpu --profile fast --offline --lib cell_cut_digest_golden`
  (octante y celda) no debe cambiar su tabla.

## Límites

- Perfil `fast`, un equipo, un dataset; las cifras no se trasladan a otro perfil o equipo.
- `real_sim.txt` guarda también las líneas de modo octante: están en el fichero pero no son la línea base; el
  modo octante desaparece con la retirada. Las simulaciones no dan tiempos útiles (200 s y 372 s en dos ejecuciones).
- `real_sim.txt` va ordenado alfabéticamente: el orden original de la salida no es estable.
- No cubre GPU ni navegador, ni `cells_bench`.
