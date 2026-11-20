# Formato V8: salida más pequeña y distancia mínima entre puntos

Estado: pendiente · 20-11-2026 · diseño en curso: falta la medida de zstd y el visto bueno del propietario

**Objetivo:** reducir el disco que ocupa la salida del conversor, que tras la fase B de temporales es lo que marca
el disco necesario para convertir (Albuera: 15,75 GB = 3,7 × el LAZ), y guardar en el fichero una estimación de la
distancia mínima entre puntos que sirva como tamaño mínimo de vóxel recomendado.

**Por qué ahora:** el [estudio del tamaño de la salida](../records/benchmarks/TAMANO_SALIDA_V7_20-11-2026.md) mide
que v7 guarda 1,97 N puntos (los LOD son otro 0,97 N) y el color dos veces; las reducciones grandes cambian el formato,
congelado en v7 por el [ADR 0044](../adr/0044-v7-final-congelada-y-predeterminada.md). La fase A de temporales queda
aparcada ([#109](https://github.com/Mili-ssm/OctreeViewer/issues/109)).

## Decisiones del propietario (20-11-2026)

| Id | Decisión |
| --- | --- |
| D1 | Se abre una revisión nueva del formato, V8, con su ADR. |
| D2 | Los puntos LOD guardan solo lo que dibuja el visor: coordenadas, RGB, intensidad y clase. Las hojas guardan todos los atributos. |
| D3 | zstd solo si una medida (tamaño, tiempo de conversión y lectura en wasm) lo justifica; la decide el propietario con las cifras. |
| D4 | El visor y el conversor soportan **solo V8**: los ficheros v7 (también el recorte de la demo) se reconvierten. Con el [ADR 0047](../adr/0047-v5-heredado-hasta-v8.md), empezar V8 retira V5 ([#28](https://github.com/Mili-ssm/OctreeViewer/issues/28)); D4 retira además v6 y v7 y sus revisiones previas. |
| D5 | V8 guarda una estimación estocástica de la distancia mínima entre puntos, como tamaño mínimo de vóxel recomendado. Que la demo la use en lugar de los 0,1 mm por defecto se decide más adelante (hoy lo fija la resolución de OQ-13 del 18-11-2026, RQ-09 y el [ADR 0054](../adr/0054-el-objetivo-de-px-es-el-tamano-minimo.md)). |

## Requisitos

| Id | Requisito |
| --- | --- |
| V1 | Con la misma entrada, el visor dibuja lo mismo que con v7 en todos los modos de color predeterminados (RGB, intensidad, elevación, clase), en todos los niveles. |
| V2 | La salida de Albuera ocupa al menos un 35 % menos que en v7 (estimado ~−40 % con D2 y el color derivado, sin zstd); se mide en Albuera, Majadas y Belvedere. |
| V3 | El conversor web sigue produciendo los mismos bytes que el CLI (RQ-08) y la lectura en el navegador no empeora más de lo que acepte el propietario. |
| V4 | RQ-07 (coste de conversión) se vuelve a medir con V8 frente a v7 en `main` y se registra. |
| V5 | La distancia mínima es reproducible (muestra con semilla fija) y se calcula sin otra pasada sobre la entrada. |

## Diseño

### Color de pantalla derivado (decisión técnica, sin pérdida)

Hoy el RGB de pantalla de 8 bits se guarda aparte aunque es `rgb16 >> 8`, o `intensidad >> 8` sin color
(`source_points.rs:140-148`); ocupa el 18,8 % de Albuera y el 27,5 % de Belvedere. En V8 no se guarda en los bloques
de hoja: el lector lo deriva al decodificar el bloque, con la misma regla, antes de subirlo a la GPU.

### Puntos LOD con atributos de dibujo (D2)

- Un bloque LOD guarda coordenadas, RGB de 8 bits (ya listo para pantalla), intensidad y clase. Sin RGB en la fuente,
  la pantalla sale de la intensidad como hoy.
- El esquema de atributos del fichero sigue siendo uno; el índice marca cada bloque como hoja o LOD, y los atributos
  ausentes en un bloque LOD se leen como «no presentes».
- El visor, con un modo de color por atributo genérico (`attribute:N:M`) y un bloque LOD sin ese atributo, dibuja el
  color de intensidad hasta que lleguen las hojas.
- Ahorro estimado: ~3,65 GB en Albuera (−23 %), suponiendo que el coste por columna de una fila LOD es el de una hoja.

### Distancia mínima entre puntos (D5)

- **Qué:** cuantiles de la distancia al vecino más cercano sobre una muestra aleatoria con semilla fija; se guardan
  p1, p5 y p50 en unidades del dataset en la cabecera del índice, junto al espaciado de referencia actual. El
  recomendado como tamaño mínimo de vóxel es p5 (decisión técnica: un percentil bajo resiste duplicados y ruido mejor
  que el mínimo).
- **Por qué no basta lo que ya hay:** el «espaciado de referencia» de v7 es la mediana, ponderada por puntos, del
  tamaño de celda en el que cada hoja deja de llenarse, redondeado a potencia de 2 (`viewer_core/src/spatial.rs:559-582`,
  `node_job.rs:984-1011`); es un tamaño típico, no un mínimo, y puede errar hasta ×2 por el redondeo.
- **Dónde:** en los trabajos de hoja, que ya leen todas las posiciones de la hoja para medir su espaciado
  (`measure_leaf_spacing`, `node_job.rs:1000-1011`): cada hoja aporta una muestra con semilla y busca el vecino en su
  propia hoja (rejilla hash); el borde de la hoja sobreestima algo, y un percentil bajo lo compensa en parte. Sin
  pasada extra sobre el LAZ (V5).
- **Uso:** la API pública lo expone como «tamaño mínimo recomendado»; el modo «espaciado de la nube» del filtro de
  longitud puede pasar a usarlo. Cambiar el valor por defecto de la demo es la decisión pendiente de D5.

### zstd (D3, pendiente de medida)

Se mide en Albuera (tiempo GPS, RGB de 16 bits y geometría) frente a LZ4: tamaño, codificación y decodificación en
nativo y en wasm. Con las cifras, el propietario decide si V8 lo incluye y para qué flujos.

### Solo V8 (D4)

- `FormatVersion` queda con V8; se retiran lectura y escritura de V5, v6 (y revisiones previas) y v7 (y revisiones
  previas), sus pruebas de paridad y los SHA fijados, que se rehacen con V8.
- El conversor escribe V8 sin opción de versión; la demo publica un recorte de Belvedere reconvertido.
- Cómo se cumple esto sin perder la comparación con v7 durante el desarrollo: la medida de V2/V4 se hace con el binario
  de `main` (v7) frente a la rama (V8), antes de retirar v7.

## Fases

1. **V8 en el formato:** versión, bloques hoja/LOD, color derivado, cuantiles de distancia en la cabecera; escritor y
   lector; v7 sigue disponible mientras tanto para comparar.
2. **Conversor y visor con V8:** el conversor escribe V8 (LOD reducido, cuantiles); el visor lee V8 y deriva el color;
   medida frente a v7 (V1–V5).
3. **zstd**, si el propietario lo aprueba con la medida.
4. **Retirada de V5, v6 y v7** (D4, #28) y reconversión del recorte de la demo.

## Fuera de alcance

Densidad de los LOD (×2 por nivel, [ADR 0020](../adr/0020-piramide-lod-densa.md)); LOD como referencias a filas de
hoja; checkpoints en el formato paginado (RF-26); cambiar el mínimo por defecto de la demo (D5, más adelante).

## Preguntas abiertas

| Id | Pregunta |
| --- | --- |
| P1 | ¿Incluye V8 zstd, y en qué flujos? (D3, tras la medida) |
