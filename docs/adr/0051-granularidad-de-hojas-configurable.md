# 0051 La granularidad de las hojas es configurable: puntos máximos, lado mínimo y distancia de fusión

Estado: Aceptada (pendiente de implementar tras #35 y #44; N se fija con el experimento [#46](https://github.com/Mili-ssm/OctreeViewer/issues/46)) · Fecha: 15-11-2026 · Decide: propietario (los parámetros y M activado; el resto de predeterminados, el agente) · Precisa el [ADR 0044](0044-v7-final-congelada-y-predeterminada.md) en la regla de hoja fija

## Contexto
- V7 fija 64.000 puntos por hoja como regla del escritor; el fichero solo guarda la política como texto descriptivo
  (`sample-grid:256;leaf:64000`, que el lector no interpreta). Las hojas grandes cargan de golpe.
- El propietario (15-11-2026) elige la opción híbrida de #46 y pide que sea configurable, sin ser estricta: nubes
  de 1 m o de pocos cm muy densas deben funcionar, y «lo normal sería querer cubos de entre 1 mm y 0,1 mm como
  último nodo antes de ir directo a puntos literales». También una distancia mínima que funda puntos (por
  ejemplo, en una nube de 100×100 m, nodo de 1 mm y puntos de 0,1 mm), usada como referencia para que la nube
  sea fusionable con otras. Los predeterminados los decide el agente, fijos o adaptativos, salvo M: el propietario
  lo quiere activado y muy bajo, proporcional a la nube (que funda en torno al 1 % de los puntos), para quitar
  duplicados y redundancias en nubes muy grandes y errores en nubes pequeñas.

## Decisión
- El conversor acepta tres parámetros: N en puntos; S y M como longitudes (m, cm, mm) que se pasan a unidades del
  CRS con la unidad que lee el [ADR 0049](0049-tamano-minimo-del-voxel.md):
  - **N, puntos máximos por hoja:** una hoja se parte mientras tenga más de N puntos…
  - **S, lado mínimo de hoja:** …y su lado sea mayor que S. Por debajo de S se queda en hoja con los puntos que
    tenga; las celdas siguen bajando hasta separar los puntos ([ADR 0048](0048-el-render-dibuja-el-nivel-pedido.md)).
  - **M, distancia de fusión:** los puntos que caen en la misma celda diádica de lado M se funden en uno. Es una
    celda, no una distancia exacta: dos puntos muy cercanos a ambos lados de un borde no se funden. El punto
    fundido toma la media de color e intensidad, la clase mayoritaria, y los flags y Extra Bytes del primer punto
    en orden de la fuente (determinista, RQ-08).
- **S y M se redondean hacia abajo a potencias de dos** de la rejilla global ([ADR 0029](0029-octree-global-por-bits-de-enteros.md)),
  con M ≥ el cuanto (2⁻¹⁵) y S ≥ M.
  Así los nodos y los puntos fundidos de nubes distintas caen en las mismas celdas y siguen siendo fusionables
  (RF-21, RF-23). El valor pedido es una referencia; el usado se informa.
- **Predeterminados:**
  - S fijo = 2⁻¹⁰ unidades del CRS (≈ 1 mm en metros): un S derivado de cada nube daría rejillas de hojas distintas.
  - M **activado y adaptativo**: la mayor potencia de dos que funde como mucho el 1 % de los puntos de la nube
    (fracción configurable; M se puede desactivar). Los duplicados exactos (misma posición cuantizada) se funden
    siempre; M sube mientras la fracción fundida no pase del 1 %. Las rejillas diádicas se anidan, así que dos
    nubes con M distintos siguen alineadas: al fusionarlas basta con la M más gruesa (volver a fundir puntos ya
    fundidos no da exactamente lo mismo que fundir una vez; es una referencia, no una regla estricta). El M usado
    y los puntos fundidos se informan.
  - Con M, una hoja de lado S tiene como mucho (S/M)³ puntos; sin M, una hoja en el límite S no tiene tope.
  - N lo fija el experimento #46 (candidatos 8–16 k).

## Alternativas
| Opción | Por qué no |
| --- | --- |
| A: máximo de puntos como % de la nube | Iguala el número de hojas, no la granularidad visible; hojas enormes en nubes grandes |
| B: solo lado mínimo | Sin tope de puntos, una hoja densa puede tener millones |
| S adaptativo por nube | Cada nube tendría su propia rejilla de hojas |
| M desactivado por defecto | El propietario lo quiere activo para quitar duplicados y redundancias |
| Mantener 64.000 fijos | La carga sigue siendo brusca en nubes con nodos grandes |

## Consecuencias
- Es una **extensión compatible** de V7 en el sentido del ADR 0044: el lector no depende de N, S ni M, así que los
  ficheros siguen siendo V7. Cambian los bytes de la salida predeterminada: hay que regenerar referencias y dorados,
  y actualizar `FORMAT_V7_PRE4.md` y `FORMAT_V7_PRE5.md` («Writer restrictions»), `docs/reference/inventory.md` y
  `ConvertOptions` (`options.rs`, hoy `FixedByFormat` para una hoja distinta de 64.000).
- Los puntos fundidos son una excepción explícita a RF-24: sus flags y Extra Bytes son los del primer punto.
- La política se describe en el fichero; los parámetros usados se informan en el resumen de conversión.
- La conversión web y la nativa deben dar los mismos bytes con los mismos parámetros (RQ-08).
- Por defecto la conversión pierde hasta ~1 % de los puntos (fundidos): el resumen lo dice, y elegir M adaptativo
  necesita contar celdas distintas por potencia de dos (una pasada, o una muestra en nubes grandes).
- Coste de conversión (incluida la pasada que elige M) y tamaño del índice se miden frente a RQ-07.

## Revisar si
El experimento #46 muestra que el tamaño de hoja no cambia la suavidad de la carga una vez hechos #35 y #44, o la
fusión por defecto quita puntos que el usuario echa en falta.
