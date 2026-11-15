# 0052 Retención por zona, hijos provisionales con el color del padre y ajuste homogéneo al pool

Estado: Aceptada · Fecha: 15-11-2026 · Decide: propietario · Precisa R3-Q19, R3-Q21 y R3-Q11 para el LOD por celda del [ADR 0050](0050-nivel-por-celda-segun-distancia.md)

## Contexto
- R3-Q19, R3-Q21 y R3-Q11 ([requisitos](../requirements.md)) están redactadas por nodo entero. El LOD por celda
  (ADR 0050) necesita su versión por celda ([plan](../plans/nivel-pedido-y-lod-por-celda.md), fase 0b).
- El propietario (15-11-2026):
  - R3-Q19 por celda: sí.
  - R3-Q21: «si te acercas a un vóxel se siga viendo el mismo vóxel hasta que carguen los hijos; si tenemos la
    máscara de población podríamos dibujar los hijos con los mismos colores que el padre hasta que llegue la
    información completa/real de los hijos».
  - R3-Q11: debe afectar a toda la nube, para que se vea lo más homogénea posible, subiendo de tamaño primero lo que
    en pantalla se ve más pequeño.

## Decisión
- **R3-Q19 por zona:** cada zona sigue dibujando lo que tiene residente hasta que el nivel pedido está en GPU en esa
  zona; lo que falta en una zona no bloquea las demás.
- **R3-Q21 por celda, con hijos provisionales:** ninguna celda se dibuja más gruesa mientras espera su nivel fino.
  Cada celda lleva una **máscara de población de 8 bits** (qué hijos tienen puntos), construida con su nivel.
  Mientras el nivel de los hijos no está residente, la celda que debe refinar se dibuja como sus hijos poblados, con
  el color del padre. Cuando llegan los hijos reales, los sustituyen.
- **R3-Q11 homogéneo:** si el corte no cabe en el pool, se sube el **objetivo de px efectivo de toda la nube**, como
  etapa contada. Así engrosan primero las celdas más pequeñas en pantalla y el tamaño en pantalla sigue siendo
  parejo, en lugar de engrosar solo los nodos o las bandas más lejanas.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| R3-Q21 sin hijos provisionales | Al acercarse, el vóxel grande se mantiene y luego salta a sus hijos de golpe |
| R3-Q11 engrosando los nodos o bandas más lejanos | La nube deja de verse homogénea |

## Consecuencias
- La máscara de 8 bits necesita sitio en la instancia de 16 B: hay 6 bits libres en la palabra de flags y la clase
  usa 16 bits aunque bastan 9. Se decide en el plan detallado de la fase 2 (reempaquetar la clase o usar el alfa).
- El worker de construcción calcula la máscara al construir cada nivel (conoce las celdas del nivel siguiente).
- El ajuste al pool pasa a ser un solo número (objetivo de px efectivo), que la interfaz puede mostrar.

## Revisar si
Los hijos provisionales se ven peor que mantener el vóxel padre, o subir el objetivo de toda la nube no basta para
caber en el pool en Majadas.
