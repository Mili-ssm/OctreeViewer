# 0053 Las transiciones de nivel avanzan de un nivel en un nivel, en el espacio y en el tiempo

Estado: Aceptada (la cola bajo la separación, pendiente de revisión) · Fecha: 16-11-2026 · Decide: propietario · Precisa el [ADR 0050](0050-nivel-por-celda-segun-distancia.md), R3-Q21 y la residencia D4 del [plan](../records/archive/nivel-pedido-y-lod-por-celda.md)

## Contexto
- El ADR 0050 pide bandas por distancia y transiciones graduales, y R3-Q21 que la imagen solo se refine dibujando
  el residente más cercano al nivel pedido. Ninguno limita cuántos niveles separan dos bandas vecinas ni cuántos
  sube una zona de golpe: con cadenas podadas (residentes `{6, 13}`), una banda puede pasar del 6 al 13, y al
  llegar un nivel lejano la zona salta directamente a él.
- El propietario (16-11-2026) quiere que las cargas sean progresivas, sin saltos de más de un nivel, también
  cuando la cadena guardada salta niveles, construyendo los intermedios: por la imagen y para dar tiempo a cargar.

## Decisión
- **Espacial:** dos celdas vecinas dibujadas difieren como mucho en un nivel. Cada nodo mantiene activos todos los
  niveles entre el más grueso y el más fino que dibuja; los que la cadena no guarda se construyen a demanda
  ([ADR 0048](0048-el-render-dibuja-el-nivel-pedido.md)).
- **Temporal:** entre dos frames consecutivos, el nivel dibujado en cada punto cambia como mucho en uno, al
  acercarse y al alejarse (propietario, 16-11-2026: «máximo un nivel de diferencia por frame»). El siguiente nivel
  se pide antes de necesitarlo; la latencia total puede crecer (ya aceptado en el ADR 0050). Al alejarse, los hijos
  no se sueltan hasta que el nivel grueso está listo (D4).
- **Niveles intermedios regenerables desde sus hijos** (propietario, 16-11-2026, como posible optimización): un nivel
  más grueso puede derivarse de un nivel más fino ya construido o residente agregando cada grupo de hijos, sin
  volver a leer los bloques del nodo; sirve sobre todo al alejarse y para rellenar intermedios. Se evalúa en la
  fase 2b. Para que coincida con el nivel construido desde los puntos hacen falta los recuentos por celda (media
  de color e intensidad ponderada, clase mayoritaria); la instancia de 16 B no los guarda, así que o se guardan
  aparte o se acepta una diferencia acotada.
- **Cola bajo la separación** (celdas de un solo punto, donde bajar de nivel solo encoge el cubo del punto): lo
  ideal es aplicar también los pasos de un nivel; queda **pendiente de revisión** con medidas de rendimiento y de
  memoria (peor caso estimado en la fase 0a: unos 531 MiB con todos los niveles, frente a un pool de 512 MiB).
- Se implementa en la fase 2b (conjunto de niveles por nodo y residencia) y se comprueba en la 2c con
  diferencias entre frames.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Solo temporal | Dos bandas vecinas podrían seguir separadas por varios niveles |
| Un mínimo de frames por nivel | No lo pide el propietario: el límite es de un nivel por frame |
| Solo espacial | Al llegar un nivel lejano la zona saltaría de golpe |
| Residente más cercano sin límite (R3-Q21 tal cual) | Saltos de varios niveles con cadenas podadas |

## Consecuencias
- La residencia D4 deja de ser «niveles dibujados y el siguiente más fino» con huecos: el rango entre el más grueso
  y el más fino dibujado es contiguo. Más memoria; si no cabe, sube el objetivo de px efectivo de toda la nube
  (R3-Q11).
- R3-Q21 sigue (la imagen solo se refina; el residente más cercano mientras carga), con el límite de un nivel por frame.
- La fase 2c mide, en una diferencia entre frames consecutivos, que ningún punto cambia más de un nivel y que las vecinas difieren
  como mucho en uno.

## Revisar si
La medida de la cola bajo la separación (fase 2b) muestra que no cabe o que su coste no compensa, o la latencia de
los pasos se nota más que la suavidad que gana.
