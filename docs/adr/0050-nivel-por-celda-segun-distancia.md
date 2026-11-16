# 0050 El nivel de detalle sigue la distancia de cada celda, no la de su octante, y las transiciones son graduales

Estado: Aceptada · Fecha: 15-11-2026 · Decide: propietario (el requisito; el mecanismo se decide en el plan de [#44](https://github.com/Mili-ssm/OctreeViewer/issues/44)) · Sustituye la regla «nivel por octante» del [ADR 0034](0034-nivel-por-octante-con-arista-real.md)

## Contexto
- El ADR 0034 elige un nivel por octante de cada nodo, medido en el punto del octante más cercano al ojo. Todo el
  octante cambia de nivel a la vez.
- V7 fija 64.000 puntos por bloque, así que los nodos son grandes: la muestra de la demo (589.824 puntos) tiene
  21 nodos, y un octante de la raíz mide 32×32 m. Un giro leve de la cámara cambia el nivel de un cuadrante
  entero, con un salto brusco y con retraso respecto al movimiento.
- El propietario (15-11-2026): «debería ser un radio, no por cuadrante»; «si me acerco corta por la mitad en vez de
  hacer un oleaje de LODs». La carga debe ser gradual, y se acepta algo más de latencia total a cambio de una
  transición más suave, progresiva y coherente.

## Decisión
- **Requisito:** el tamaño dibujado de cada celda sigue su propia distancia al ojo. El límite entre dos niveles es
  una superficie alrededor del ojo, no el borde de un octante o de un nodo: al acercarse, los niveles avanzan en
  bandas concéntricas («oleaje»), sin cortes rectos.
- **Transición gradual:** al moverse la cámara, las celdas cambian de nivel una a una según su distancia; ningún
  octante o nodo cambia entero de golpe. Los niveles se piden antes de necesitarlos, y la latencia total puede
  crecer.
- Siguen RQ-09 (ningún cubo por encima del objetivo, ahora medido por celda), R3-Q21 (la imagen solo se refina) y
  el [ADR 0048](0048-el-render-dibuja-el-nivel-pedido.md) (el nivel pedido se construye si no está guardado).
- El mecanismo se elige en el plan de #44 con medidas en Majadas y Belvedere. Candidatos: decidir el nivel por
  instancia en el shader, con los dos niveles vecinos residentes en la franja de transición, o la rejilla de 512
  celdas por nodo descrita como alternativa en el ADR 0034.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Mantener un nivel por octante (ADR 0034) | Saltos de cuadrantes enteros, visibles en la demo |
| Nodos más pequeños en la conversión | Cambia V7, que está congelado (ADR 0044), y solo reduce el salto |

## Consecuencias
- En la franja de transición pueden estar residentes dos niveles de un mismo nodo: más memoria de GPU.
- Cambian los criterios de instancias de la Tarea 4 de R3 (ADR 0034) y los dorados del corte; hay que volver a
  medirlos.

## Revisar si
La memoria o el tiempo de frame superan los presupuestos de RQ en Majadas o Belvedere.
