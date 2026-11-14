# R3 B2b: intensidad y clase en la ruta asíncrona de celdas — 14-11-2026

**Estado: pasa. Corrige el [registro de B2](R3_B2_COLOR_14-11-2026.md)**, que daba por buenos en el
navegador los modos de intensidad y clasificación. En Chrome, las celdas se construyen en el worker de
construcción por la ruta asíncrona, y esa ruta leía en modo RGB solo la geometría: las celdas quedaban
sin intensidad (palabra 65535) ni clase (−1), y al pasar a esos modos el shader caía en sus colores de
respaldo (luminancia del RGB y RGB). La corrección de B2.2 solo cubría la ruta síncrona. Tras esta
corrección, las dos rutas dan el mismo tipo de color en Belvedere y Majadas.

## Causa y corrección

- Los datos: en Belvedere y Majadas v7.5, las columnas de intensidad y clasificación valen 0 en todos
  los puntos (rangos `[0, 0]` de la cabecera, comprobados contra los valores de varios nodos). No es un
  fallo del conversor ni del lector.
- El fallo: `build-worker.js` planificaba las lecturas según el modo de color (en RGB, `"geometry"`), y
  `insertAsyncLodBlock` admitía un solo atributo.
- La corrección (`6f37e29`): con las celdas activas, el worker usa la selección `geometryAttributes`,
  que lee geometría, RGB, las columnas de intensidad y clase y, en un modo `attribute:N:M`, la columna
  pintada; `insertAsyncLodBlock` recibe esas columnas. La ruta CPU no cambia. Coste: 3 B decodificados
  más por punto; los bytes leídos del disco no se midieron.
- Test: `async_cell_blocks_carry_intensity_and_class_in_rgb_mode` falla con la ruta anterior
  (`[(65535, −1)]`) y pasa con la nueva (`(150, 2)`).

## Navegador

Probe `probe-cells-colour.mjs`, Chrome con WebGPU, 4 px, encuadre, una ejecución por dataset.

| Modo | Ruta CPU (Majadas) | Celdas antes | Celdas después | Nodos enviados al cambiar |
| --- | --- | --- | --- | --- |
| Intensidad (columna constante 0) | gris plano | luminancia del RGB | gris plano, mismo color dominante (78,78,78) | 0 |
| Clasificación (columna constante 0) | rojo plano | RGB | rojo plano (157,0,0) | 0 |

Belvedere da lo mismo: clasificación en rojo plano y 0 nodos enviados al cambiar a intensidad,
elevación o clasificación. En los dos datasets, `cellPath` sigue en `enabled`, el `cutDigestAtRest` no
cambia entre modos y `attribute:0:0` recolorea (13 nodos en Majadas, 22 en Belvedere). La única
entrada de consola es el 404 sin URL de siempre.

## Límites

- La comparación con la ruta CPU es por tipo de color (gris o rojo planos y color dominante), no píxel a
  píxel: las dos rutas no voxelizan con la misma rejilla (R3-Q32). Las capturas quedan fuera del
  repositorio (`target/r3-b2/` y `target/r3-b2b/`).
- Estos datasets no tienen intensidad ni clase con valores distintos de 0, así que el navegador solo
  comprueba el caso constante. La variedad de valores la cubren los tests de paridad de B2.

## Identidad y reproducción

- Código: rama `explore/b2b-async-attrs`, revisión `6f37e29`, integrada después en `feat/r3-v75`.
  Entorno: el de [T11](data/r3-t11-smoke-13-11-2026/identity.txt).
- Datos en [data/r3-b2b-async-attrs-14-11-2026](data/r3-b2b-async-attrs-14-11-2026/).
