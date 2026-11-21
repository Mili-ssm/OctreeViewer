# Fixtures de formatos retirados

Cabecera (24 bytes, `<versión>.head`) y pie (40 bytes, `<versión>.foot`) del `.oct` real que escribía cada
familia que la tarea 5 de [formato-v8-congelado](../../../../../docs/plans/formato-v8-congelado.md) retira:
v5, v6.1-pre … v6.4-pre, v7.1-pre … v7.5-pre, v7, v8.1-pre y v8.2-pre. Sirven al test de la tarea 5: abrir un
fichero retirado debe decir «version N: reconvert to V8» sin conservar lectores antiguos (A4 del pre-mortem).

| Dato | Cómo identifica la versión |
| --- | --- |
| Cabecera | `OCTVFRM1`, `u16` mayor en los bytes 8-9 y `u32` revisión en los bytes 16-19 (0 = final) |
| Pie | La magia de 8 bytes lo identifica solo: `OCTVEND5`, `OCTVE61P` … `OCTVE75P`, `OCTVEND7`, `OCTVE81P`, `OCTVE82P` (V8 final: `OCTVEND8`) |

El pie basta para distinguir las 13 familias, también las paginadas (v7.5-pre en adelante), así que un test
puede comprobar cada fuente por separado. El pie del `.octattrs` lleva la misma magia; no se guarda.

Se generan con `generate.sh [ruta/al/viewer_convert]` a partir de `data/samples/fs_e430_n4008_18.las`; hace falta
un conversor que aún tenga los escritores (hasta la tarea 5, o una revisión anterior). Los ficheros son
binarios fijos: no se regeneran al retirar los escritores.

`v8.2-pre.head` y `v8.2-pre.foot` se generaron antes de la tarea 3 (commit `be4b69f`), que retiró el escritor de
v8.2-pre: `generate.sh` ya no los regenera y se conservan tal cual.
