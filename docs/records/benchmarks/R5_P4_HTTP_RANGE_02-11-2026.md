# R5 P4: apertura remota por HTTP Range

Fecha: 02-11-2026. Rama: `feat/r5-paged-hierarchy` (`bc102c6`). Estado: **RQ-04 se cumple con holgura en los cinco ficheros medidos; vista fija sin órbita**.

Registro fechado e inmutable. Tarea P4 del [plan R5](../archive/render-r5-jerarquia-paginada.md#tarea-p4-http-range); formato en [FORMAT_V7_PRE5](../../reference/format/FORMAT_V7_PRE5.md), fuente en [ADR 0023](../../adr/0023-fuentes-locales-y-remotas.md).

## Método

- Perfil `release`, Chrome headless 1280×720, equipo de T6b. Objetivo de 4 px.
- Red RED-REF (100 Mbit/s, 50 ms de RTT) simulada por `examples/web-host/red-ref-server.mjs`; datos y app en orígenes distintos.
- Sonda `examples/web-host/probe-remote-open.mjs DATA_DIR NAME.oct OUT.json`.
- Fuente `http-range-source.mjs`: apertura con una petición de sufijo (`Range: bytes=-1 MiB`), después un rango simple por petición con `If-Range: <ETag>`; las lecturas contiguas del mismo tick se unen. Sin multirango.
- Ficheros: `v7.5-pre` con páginas por defecto (*k* = 5), `belvedere-v75k1` con `--page-target-bytes` pequeño (*k* = 1, 767 páginas), y `v7.4-pre` como línea base.

## Resultados

| Fichero | Primer frame | 4 px | GET hasta 1.er frame | GET totales | Bytes de cuerpo |
| --- | --- | --- | --- | --- | --- |
| belvedere-v75 | 377 ms | 954 ms | 11 | 23 | 2,05 MB |
| belvedere-v75k1 | 390 ms | 1 253 ms | 11 | 24 | 2,06 MB |
| belvedere-v74 (base) | 784 ms | 1 500 ms | 13 | 25 | 4,46 MB |
| majadas-v75 | 370 ms | 861 ms | 7 | 27 | 1,89 MB |
| majadas-v74 (base) | 2 195 ms | 2 883 ms | 9 | 29 | 15,4 MB |

## Veredicto frente a los requisitos

| Criterio | Umbral | Medido (peor caso v7.5) | Resultado |
| --- | --- | --- | --- |
| RQ-04, primer frame | ≤ 10 s | 390 ms | **pasa** (25 × de margen) |
| RQ-04, objetivo de 4 px | ≤ 120 s | 1 253 ms | **pasa** (96 × de margen) |
| Parada P4: peticiones extra en el primer frame | 0 | v7.5 abre con 1 petición `.oct` frente a 3 de v7.4 | pasa |
| Parada P4: viajes de página en serie a 4 px | ≤ 3 | *k* = 5: 0 lecturas de página; *k* = 1: 4 lecturas en 1 oleada | pasa |
| ASM-02, bytes por vista a 4 px | medir | 1,9–2,1 MB (1 MiB es la lectura de cola) | medido |

Frente a `v7.4-pre`, v7.5 reduce el primer frame de Majadas de 2 195 a 370 ms y los bytes de 15,4 a 1,89 MB (el índice entero ya no se descarga).

## Comprobaciones de comportamiento

- Escritorio y móvil (390×844): `belvedere-v75.oct` abierto por `?url=`, objetivo alcanzado, consola sin errores salvo el 404 de `favicon.ico` del servidor estático.
- Con `--ignore-range` el visor informa `HttpRangeError: the server ignores Range` y aborta.

## Límites

- Vista fija sin órbita: no es la matriz T7 ni mide la carga continua.
- Sin S3 real ni servidor con compresión; el RTT y el ancho de banda son simulados.
- Hueco de unión de rangos 0: solo se unen rangos contiguos.
- Una ronda por fichero; sin dispersión. Las cifras valen para esta revisión, perfil, navegador, dataset y equipo.
