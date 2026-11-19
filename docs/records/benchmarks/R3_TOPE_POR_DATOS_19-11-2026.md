# R3: el tope por datos conectado al deseo de las celdas — 19-11-2026

**Estado: medido; el tope queda conectado.** Limitar el deseo de cada octante a la cota por datos del nodo no
cambia la imagen en reposo (diferencia ≤ 0,119 % en P1 a P4), no deja ninguna muestra dibujable más gruesa que
antes y libera las páginas que el pool precargaba junto al ojo en escenas pequeñas (288 de 2602 en P2). En Majadas
el ahorro es mínimo (72 de 45 806 páginas a 1 px). Para conectarla hubo que **redefinir la cota**: la de la tarea 5
hacía depender el corte final del orden de llegada de los niveles.

Es la tarea 7a del [plan del ADR 0055 en modo celda](../../plans/adr-0055-rango-por-nodo.md). Continúa los registros
de la [cota por datos](R3_EXTREMO_FINO_POR_DATOS_18-11-2026.md) y de sus
[fuentes](R3_FUENTES_COTA_POR_DATOS_18-11-2026.md).

## Condiciones

| Dato | Valor |
| --- | --- |
| Revisión | Antes: `d8e49d8`. Después: `5d46959` (rama `feat/adr-0055-tramo-b`) |
| Perfil y toolchain | Muestra y Belvedere: `fast`. Majadas: `perf`. rustc 1.101.0-nightly (2026-10-27) |
| Equipo | AMD Ryzen AI 9 365 |
| Navegador | Chromium sin cabeza con WebGPU (Playwright), 1280×800 y 390×844 |
| Datasets | Muestra integrada de la demo (poses P1 a P5, 4 px); `target/render-data/belvedere-v7.oct`; `target/render-data/majadas-v7.oct` dentro de una hoja |
| Comandos | `cargo test -p viewer_wasm --profile fast --offline --lib -- --ignored <eye_node_residency_at_poses \| synthetic_zoom_in_lag \| real_lag_belvedere_dolly \| real_lag_belvedere_orbit> --nocapture` (los reales con `R3_REQUIRE_REAL_DATA=1`) · `BENCH_DATASET=majadas BENCH_POSE=px BENCH_PX=<1\|4> BENCH_HI_D=1 <binario perf> --ignored cells_bench --nocapture` |

Las cifras solo valen para estas revisiones, perfiles, navegador, datasets y equipo.

## La cota que se conecta

| | Tarea 5 (descartada al conectarla) | Tarea 7a |
| --- | --- | --- |
| Definición | Cota de las cajas de página del nivel residente más fino; con la pose fija solo sube | Mínimo, entre los niveles residentes `L` del nodo, de `max(b(L), L)`, y nunca bajo `lo`; `b(L)` es la cota de las cajas de página de `L` recortadas a la vista |
| Nivel residente sin cajas a la vista | No aporta cota (sin tope) | Aporta `L`: ningún dato del nodo está a la vista |
| Depende de | El orden en que llegaron los niveles | Solo de qué niveles son residentes |

Por qué es válida: cada `b(L)` es una cota (las cajas de un nivel grueso contienen los datos de los finos), así que
su mínimo también; un nivel nunca aporta menos que su propia profundidad, así que el tope no expulsa el nivel que
lo justifica; los niveles por encima del tope no pueden bajarlo.

Lo que falló por el camino, con la cota conectada:

| Definición | Fallo |
| --- | --- |
| Tarea 5 | 3 simulaciones sintéticas acaban en cortes distintos según el orden de llegada (`synthetic_build_cap_defers…`, `synthetic_regraft…`, `…tight_page_budget…`) |
| Mínimo sobre los niveles, sin la regla de «sin cajas a la vista» | `sim_synthetic_pool_reset_and_driver_replaced_mid_load_converge`, los 4 casos de dolly: el nodo 21 con residentes 7 a 10 da cota 10 (solo la caja del nivel 7 toca la vista) y con residentes 8 a 15 no da ninguna |

## Resultados

Residencia junto al ojo en la muestra, regla contigua (páginas del nodo 14 / de ellas, sobre la cota; total del
pool):

| Pose | Antes | Después | Pool antes | Pool después |
| --- | --- | --- | --- | --- |
| P1 | 50 / 0 | 50 / 0 | 1193 | 1193 |
| P2 | 701 / 360 | 413 / 72 | 2602 | 2314 |
| P3 | 701 / 288 | 557 / 144 | 2890 | 2746 |
| P4 | 341 / 144 | 197 / 0 | 2076 | 1860 |
| P5 | 701 / 288 | 485 / 72 | 2746 | 2530 |

Las páginas que quedan sobre la cota son del nivel 18, residente pero ni deseado ni activo: aún no se ha expulsado.

Majadas (páginas residentes del pool):

| Objetivo | Antes | Después | Pico antes | Pico después |
| --- | --- | --- | --- | --- |
| 4 px | 16 214 | 16 059 | — | — |
| 1 px | 45 806 | 45 734 | 46 052 | 46 016 |

Acercamiento en Belvedere (muestras-frame más gruesas que la referencia, en celdas ocupadas):

| Simulación | Frames hasta asentarse, antes → después | Dentro del recorte de la vista: juzgadas / más gruesas | Fuera del recorte: más gruesas a 1 nivel / a 2 |
| --- | --- | --- | --- |
| Dolly, semilla 3 | 29 → 29 | 703 / 0 | 267 779 / 426 |
| Órbita, semilla 1 | 54 → 53 | 0 / 0 | 86 690 / 0 |
| Órbita, semilla 7 | 37 → 37 | 0 / 0 | 57 419 / 0 |

Antes del tope no había ninguna muestra más gruesa (salvo 1526 en la órbita 7 con la regla de ventana). Las de
después están todas fuera de los planos cercano y lejano de la vista: no se dibujan. Zoom sintético sobre la
muestra: 0 antes y después. Las simulaciones reales mantienen `unexcused_drops`, `zone_coarsenings` y
`lost_samples` a 0 sin tocar el oráculo; solo cambian las estadísticas de elección de celda (`cell_node_changed`
827 → 382 y 2166 → 1854).

Navegador, compilado desde `66f7ac6`:

| Flujo | Resultado |
| --- | --- |
| Poses P1 a P4 frente a la compilación anterior | Diferencia de píxeles 0,119 %, 0,005 %, 0,003 % y 0,017 % |
| Zoom de 8 pasos desde P2 y vuelta | Sin huecos en los 16 pasos; al volver, 0,002 % respecto al inicio |
| Giro de 6 arrastres y vuelta | Asentado en unos 0,16 s tras cada arrastre; al volver, 0,003 % |
| Belvedere, las dos reglas de activos, escritorio y móvil | Se asienta, peticiones a 0, contadores constantes 10 s en reposo |
| 15 s en reposo en P2 | Páginas y peticiones constantes |

## Límites

- Las cámaras de las simulaciones tienen una franja de recorte de 1,9 a 3,9 unidades: casi ninguna muestra cae
  dentro (703 en el dolly, ninguna en las órbitas). La ausencia de retraso visible se apoya en el navegador.
- En el navegador no hay referencia de la compilación anterior para el **tiempo** de llegada del detalle al
  acercarse o girar; solo se comprobó que no hay huecos y que se asienta.
- Mientras la cámara se mueve dentro del margen de deriva de un nodo (4 % de su distancia, 5 % de giro), el nodo
  conserva la cota de la pose anterior; en reposo se recalcula todo. Sin medir su efecto en movimiento.
- Cada nivel que llega o sale recalcula la cota de todos los nodos del corte. En Majadas a 1 px el frame mediano no
  empeora (645 → 509 ms, medida no comparable entre ejecuciones).
- Los ficheros `data/datasets/*.oct` son del formato v5 y la compilación actual no los abre; Belvedere se abre desde
  `target/render-data/belvedere-v7.oct`.
