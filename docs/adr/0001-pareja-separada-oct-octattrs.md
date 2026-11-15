# 0001 La salida es una pareja separada `.oct` / `.octattrs`

Estado: Aceptada · Fecha: fecha no registrada (vigente desde v5 definitivo)

## Contexto
El visor necesita abrir geometría y jerarquía sin pagar RGB ni columnas numéricas, y leer atributos por bloque/fila de forma independiente. Los `.oct` v1–v4 y v5 provisional (`OCTVWFRM`) monolíticos no se leen.

## Decisión
El `.oct` guarda marco, jerarquía, esquema e índice común; el `.octattrs` guarda RGB y columnas alineadas por bloque/fila. El `.oct` abre solo (gris, RGB neutro `(160,160,160)`); ambos exigen identidad exacta y asociación de dataset compatibles. Existe también un modo monolítico por API de streams con el mismo layout de payloads.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Archivo único con atributos | Abrir coordenadas obliga a leer o saltar atributos; cada columna nueva cambia el archivo principal. |

## Consecuencias
Dos archivos que pueden desincronizarse; su publicación no es atómica (el conversor valida candidatos y publica atributos antes que coordenadas). La demo web debe asociar el acompañante.

## Revisar si
Se necesita publicación atómica de la pareja o un contenedor único con carga parcial.

## Evidencia
[FORMAT_V5](../reference/format/FORMAT_V5.md); [uso y formato](../estado.md#uso-y-formato)
