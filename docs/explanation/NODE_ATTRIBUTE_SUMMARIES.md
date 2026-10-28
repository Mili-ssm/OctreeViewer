# Resúmenes de atributos y diccionario conjunto por columnas

Contrato acordado el 14-10-2026. Primera implementación **experimental**; v5 y pre1/pre2/pre3
conservan sus contratos. No se publica una revisión pre4 ni se modifica el visor.

## Identidad y separación de datos

Cada bloque tiene una topología ocupada con arrays por profundidad. Para los **nodos internos**,
el resumen en `(depth, index)` corresponde exactamente al nodo con esos índices. La parte de
referencias solo guarda IDs de combinaciones: no necesita copiar máscaras ni relaciones.
Las tablas del diccionario son columnas de igual cardinalidad K; la fila K representa la misma
combinación en todas las columnas. El acceso es:

```text
id = referencias[depth][index]
valor = tabla_del_atributo[id]
```

Las hojas conservan el mapa hoja → filas originales existente y todas las columnas originales.
Varias filas pueden compartir celda con atributos diferentes. **Un ID de resumen no sustituye
esas filas**. Originales y resúmenes tienen tipos distintos y usan grupos/diccionarios separados.
El diccionario de resúmenes es local al bloque y compartido entre sus profundidades internas.
No se define aquí una grilla física compartida ni se cambian las coordenadas originales.

## Políticas explícitas por atributo

El esquema actual solo expresa nombre, tipo y componentes. `NodeAttributePolicy` es una entrada
explícita del constructor; no se infieren reglas a partir de nombres.

| Dato | Resumen interno | Combinación de hijos |
|---|---|---|
| RGB de presentación | Media de componentes en espacio lineal | Ponderación por filas válidas |
| Intensidad | **Solo media**, sin extremos | Ponderación por filas válidas |
| Otro numérico elegido | Media o extremos según su uso | Ponderación o unión de extremos |
| Categoría/string categorizado | Máscara de presencia | OR |
| Timestamp | Mínimo y máximo, sin media | Mínimo de mínimos y máximo de máximos |

`summarize_attribute` y `summarize_colors` producen `NodeSummaryColumn` con registros de ancho
fijo y niveles internos en orden raíz → padres de hojas. Las semillas de las hojas son temporales.
El árbol vacío devuelve niveles internos vacíos; profundidad cero no tiene nodos internos.
No se acumulan listas de filas por cada padre. El índice fuente permanece inmutable.

### Medias y validez

Cada componente guarda `(count:u64, mean:f64)`, 16 bytes little-endian. El contador representa
filas originales válidas, incluidos duplicados. Un padre combina medias con sus conteos, nunca
con pesos iguales por hijo. Se usa una combinación convexa para evitar desbordar sumas de
valores extremos. Las medias son aproximaciones f64, incluidos enteros grandes; no sustituyen
los valores originales. El orden de agregación es determinista, no se promete igualdad bit a
bit frente a otros órdenes de reducción.

NaN y ±infinito no contribuyen a medias/extremos; sus bits originales permanecen en hojas.
Conteo cero significa ausencia y los bytes de valor son cero como relleno, **no un valor válido**.
El modelo actual no tiene nulls ni máscara de validez: cero numérico no se interpreta como missing.
Los conteos pueden diferir por componente. RGB usa la proyección RGB8 → lineal f32 ya existente,
acumulada en f64; no se redondea de vuelta a RGB8 en este módulo.

### Categorías

El llamante proporciona códigos **unsigned escalares y densos** en `[0, category_count)` y un
dominio estable para todo el bloque/atributo. El ancho depende del dominio, no del máximo de
cada nodo: 1/2/4/8/16 bytes hasta 8/16/32/64/128 categorías; después `ceil(K/64)` palabras u64.
Bit c indica presencia del código c; bytes little-endian. Los bits fuera del dominio son cero.
Códigos fuera del dominio, tipos signed/float o múltiples componentes se rechazan.

`CategoricalAttribute::new` permite preparar esos códigos a partir de valores numéricos
dispersos o tuplas multicomponente completas: asigna códigos u32 por primera aparición y
conserva un diccionario tipado código → tupla original. Su igualdad es por bits; si se elige
categorizar floats, NaN distintos y signed zero son categorías distintas. No normaliza ni
descarta valores. El dominio resultante es local a la columna suministrada; para varias nubes
hará falta reconciliar sus tablas antes de combinar máscaras. La columna vacía tiene dominio
cero; no se construye una máscara de dominio desconocido/cero.

La máscara no conserva frecuencias ni categoría predominante. No se implementa aún ingestión
de strings: el futuro esquema debe persistir código → string, normalización/igualdad y dominio.
Una categoría multicomponente se convierte primero en un código para su valor completo. Dominios
enormes y dispersos justifican otro experimento; no asignar arbitrariamente grandes códigos.

### Extremos

Por componente: `count:u64, min:T, max:T`, con T igual al tipo original. u64/i64 se comparan sin
pasar por f64, conservando timestamps mayores que 2^53 y límites enteros. Para floats finitos
numéricamente iguales se conserva el primer extremo encontrado en el recorrido determinista
(incluido el signo de cero); los originales siguen intactos.

Los extremos permiten **descartar** nodos sin intersección con un rango. La intersección no
demuestra que exista un punto dentro: tiempos 10 y 100 no prueban presencia entre 40 y 50.
Máscaras y extremos de atributos diferentes tampoco conservan correlaciones entre filas.
Los filtros exactos deben descender y comprobar filas; el experimento no implementa filtros.

## Contenedor experimental y acceso

`viewer_format::experimental_tuple_dictionary` construye combinaciones completas mediante claves
de bytes con igualdad ante colisiones y asigna IDs por primera aparición. Columnas originales
con NaN, signed zero y enteros grandes conservan todos los bits. Todas las columnas de un grupo
deben compartir número y orden de registros.

Layout autónomo (no es `.octattrs`): cabecera de 32 B (`OCTATEXP`, filas u32, K u32, número de
segmentos u32, columnas u32, bits/ID u8 y siete ceros), conteos u32 por segmento, anchos u32 por
columna, directorio de 29 B por payload y CRC32 de toda la metadata. El primer payload contiene
IDs empaquetados LSB-first con `ceil(log2(K))` bits; K=1 usa cero bits. Los restantes contienen
tablas independientes por columna, de K registros. Cada descriptor contiene offset u64, bytes
almacenados u64, bytes brutos u64, codec u8 y CRC32 de los bytes almacenados. Cada payload elige
raw o LZ4. El modo de control `OCTARAWX` mantiene columnas completas independientes con IDs
implícitos y descriptor de IDs vacío; permite comparar la misma envoltura.

El lector comprueba metadata, dimensiones, offsets contiguos, archivo completo, límites, CRC
de los payloads solicitados, todos los IDs y padding. Solo lee metadata, IDs comunes y la tabla
solicitada. Las otras tablas se validan al solicitarlas. `ColumnProjection::record(depth,index)`
consulta los buffers residentes sin expandir la columna. LZ4 requiere descomprimir la tabla
solicitada completa dentro del bloque: no hay acceso físico a un escalar comprimido aislado.

`limit` limita metadata, cada buffer seleccionado y tamaño expandido por separado; **no limita
RSS ni el pico combinado**. `expand()` reserva además la columna completa. El constructor usa
la política normal de asignación Rust; pueden coexistir fuente, hash map, tablas, IDs y candidatos.
No hay aún paginación, caché compartida entre consultas ni enlace de identidad con `.oct`.

## Experimento y cierre

El ejemplo `node_attribute_bench` compara diccionario conjunto columnar e independientes
raw/LZ4 sobre **exactamente los mismos datos**. Separa el grupo de filas originales del de
resúmenes internos: añadir resúmenes cuesta bytes, no se presenta como ahorro sobre pre3.
Las reglas se pasan explícitamente (`mean:INDEX,category:INDEX,range:INDEX`); RGB siempre
usa media lineal. Todos los atributos originales permanecen en el grupo exacto.
`category:INDEX` categoriza los valores originales; `category:INDEX:DOMAIN` acepta códigos
ya densos de un dominio explícito. El experimento guarda las tablas de categorización en
artefactos separados: u32 K, u32 componentes, tuplas originales. El tipo proviene del esquema
de la entrada, conservado en el log; no son sidecars autónomos para distribución. Sus bytes
se reportan aparte como coste común a ambos candidatos. El tiempo de categorización se mide
una vez por bloque, separado de las medianas de construcción de resúmenes.

Para las muestras reales, construye una **grilla experimental normalizada por bloque de depth 4**
sobre las posiciones f32 leídas. No es la topología persistida pre2, ni evidencia de integración
del contrato `(depth,index)` en los archivos actuales; la alineación se prueba contra el índice
construido. El corpus sintético incluye datos repetidos y timestamps únicos como contraejemplo.

Por bloque se alterna el orden de candidatos durante tres repeticiones, se escribe cada artefacto,
se relee y se comprueba byte a byte cada columna. Se mide codificación, lectura/descompresión/
validación de todas las columnas (consultas independientes) y expansión por separado; la
comparación byte a byte queda fuera de los cronómetros. Se registra además bytes leídos por
consulta de columna, incluyendo metadata e IDs. No se controla caché del SO ni se mide durabilidad.
Los agregados CPU suman medianas por bloque: no son tiempos de conversión end-to-end.

```sh
cargo build --offline --profile perf -p viewer_format --example node_attribute_bench
target/perf/examples/node_attribute_bench --synthetic target/node-attrs-new 2 3
target/perf/examples/node_attribute_bench RUTA.oct target/node-attrs-real 16 3 \
  'mean:0,category:1,range:7'
cargo test --offline -p viewer_core --test node_attributes
cargo test --offline -p viewer_format --test experimental_tuple_dictionary
```

## Resultados de la primera entrega — 14-10-2026

[Resumen reproducible y hashes](../records/benchmarks/data/node-attributes-2026-10-14.json). Perfil Cargo `perf`,
Linux x86_64, NVMe local, caché no controlada. Tres repeticiones con candidatos alternados
dentro de cada bloque. Las muestras reales cubren todos los bloques de las entradas indicadas,
incluidos representantes LOD; son filas almacenadas, no necesariamente puntos fuente únicos.
Todos los bytes son longitudes lógicas, no espacio físico tras compresión del filesystem.

| Muestra / grupo | Filas | Control raw/LZ4 (B) | Diccionario conjunto columnar (B) | Cambio |
|---|---:|---:|---:|---:|
| Belvedere / originales | 81.706.211 | 611.122.652 | 293.253.872 | −52,01 % |
| Belvedere / internos | 208.726 | 8.064.187 | 8.180.715 | +1,45 % |
| Outer Banks / originales | 111.768 | 932.103 | 1.104.949 | +18,54 % |
| Outer Banks / internos | 8.532 | 292.966 | 298.502 | +1,89 % |
| Bunny parcial / originales | 40.577 | 10.477 | 6.612 | −36,89 % |
| Bunny parcial / internos | 2.606 | 23.588 | 19.148 | −18,82 % |
| Sintéticas juntas / originales | 8.192 | 26.518 | 32.488 | +22,51 % |
| Sintéticas juntas / internos | 1.170 | 12.252 | 12.204 | −0,39 % |

Belvedere tiene 17.578.926 combinaciones originales distintas sumadas por bloque (21,51 % de
las filas), frente a 200.328 combinaciones de resúmenes para 208.726 nodos internos (95,98 %).
Los extremos/medias/conteos introducen combinaciones nuevas: el ahorro de originales no
predice el ahorro de resúmenes. El coste común adicional de los dominios categóricos es
12.087 B en Belvedere y 423 B en Outer Banks; se suma al candidato elegido, no está oculto en
los porcentajes de cada grupo. Bunny resume solo RGB; las otras muestras reales resumen
RGB, intensidad (media), clasificación (máscara densa) y GPS time (extremos).

Las dos sintéticas separadas conservan el contraejemplo: la agrupada pasa de 807 a 633 B en
originales y de 1.647 a 867 B en internos; con timestamps únicos pasa de 25.711 a 31.855 B y
de 10.605 a 11.337 B respectivamente. No interpretar el agregado como mejora universal.

### Comparación con pre3 y acceso selectivo

El control es raw/LZ4 independiente, **no** los codecs adaptativos de pre3. Los payloads de
atributos de las entradas pre3 ocupan 439.181.762 B en Belvedere, 797.791 B en Outer Banks y
285 B en Bunny; excluyen su directorio en `.oct` y envoltura. Los contenedores experimentales
incluyen metadata propia por bloque. Comparar estos valores informa el siguiente experimento,
pero no constituye una pareja válida ni una reducción medida del formato publicado.
En particular, superar raw/LZ4 en Bunny no supera sus constantes pre3.

En Belvedere, consultar únicamente clasificación lee 871.614 B con el control y
150.161.717 B con el diccionario conjunto (≈172 veces), incluyendo metadata. Consultar RGB16
lee 360.642.483 / 239.015.899 B. Separar tablas evita leer atributos ajenos, pero el stream de
IDs depende de la cardinalidad conjunta y penaliza atributos de cardinalidad baja.

| Belvedere / CPU sumada de medianas por bloque | Control | Diccionario conjunto |
|---|---:|---:|
| Codificar originales | 2,078 s | 6,841 s |
| Leer/descomprimir/validar originales, consultas independientes por columna | 1,913 s | 8,961 s |
| Expandir originales | 1,297 s | 8,183 s |
| Codificar internos | 0,027 s | 0,053 s |
| Leer/descomprimir/validar internos | 0,025 s | 0,030 s |

Construir resúmenes Belvedere suma 4,212 s de medianas; categorizar clasificación una vez por
bloque suma 0,836 s. No incluyen construir el índice, lectura del origen ni persistencia.
Cada consulta de columna vuelve a leer/validar los IDs: no es un lector completo optimizado
que los cachee entre columnas. Los 116,994 s de ejecución del ejemplo incluyen escrituras,
relecturas y comprobaciones de tres repeticiones, no son un tiempo de conversión.

Decisión de esta entrega: conservar el candidato para grupos donde compense, con selección
adaptativa pendiente que incluya raw/LZ4 y codecs pre3. Ensayar RGB8+RGB16 juntos y atributos
numéricos/categóricos/temporales independientes; evaluar también el coste de los IDs durante
consultas. No forzar el diccionario completo ni dar por integrado un ahorro de almacenamiento.

### Reproducción de la suite y validación

```sh
python3 crates/viewer_format/benchmarks/node_attribute_suite.py \
  --binary target/perf/examples/node_attribute_bench \
  --output target/node-attrs-new-suite --report target/node-attrs-new-report.json \
  --sample Synthetic synthetic '' \
  --sample Belvedere RUTA/Belvedere.oct 'mean:0,category:1,range:7'
```

El resumen enlazado identifica entradas y acompañantes, binario, fuentes, comandos, logs y
manifiestos SHA-256 de todos los artefactos. Artefactos/logs completos están en
`target/node-attrs-suite-20261014/` (ignorados por Git). El informe persiste muestras completadas
si falla una posterior. No mezclar un prefijo `--max-blocks` con una muestra completa.

Pasan 196 tests de `viewer_core` + `viewer_format --all-targets`, incluidos siete de resúmenes/
categorización y tres del contenedor, Clippy con `-D warnings`, fmt y comprobación WASM de ambas
bibliotecas. Python: sintaxis y `--help` comprobados; Ruff no está instalado. La revisión
estática focalizada no encontró fallos concretos. No hay cambios de interfaz que comprobar.

Pendiente antes de publicar otra revisión: comparar codecs pre3 con envoltura equivalente,
medir RSS/caché fría y ejecución WASM, decidir agrupaciones/fallback, persistir políticas/
dominios/validez, vincular geometría/atributos, integrar conversión y lectura, checkpoints,
límites globales y LOD/filtros con navegador. Compilar para WASM no acredita su rendimiento.
