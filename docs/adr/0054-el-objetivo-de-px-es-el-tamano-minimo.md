# 0054 El objetivo de px es el tamaño mínimo del vóxel, y el mínimo en longitud está activo por defecto (0,1 mm)

Estado: Aceptada · Fecha: 16-11-2026 · Decide: propietario · Cambia RQ-09 y el predeterminado del filtro de longitud de la [revisión del ADR 0049](0049-tamano-minimo-del-voxel.md#revisión-16-11-2026-dos-filtros-combinables-gana-el-más-grande)

## Contexto
- RQ-09 decía «ningún cubo por encima del objetivo de px»: el corte refina mientras la arista proyectada supera el
  objetivo, así que los cubos dibujados miden entre la mitad del objetivo y el objetivo.
- El propietario (16-11-2026): «los px objetivos son píxeles de tamaño mínimo, nunca debería haber un vóxel más
  pequeño que ese número de píxeles; la idea es bajar todo lo posible sin pasarse de memoria, sin pasarse de detalle
  pequeño en px (lo de los px es una forma de gestionar el uso de memoria indirectamente) y el slider de tamaño mínimo
  (en unidades de la nube) para decidir cuál es el detalle mínimo mostrable por mucho zoom que se haga; de forma
  predeterminada debería ser 0,1 mm, al igual que el px es 4 (quizá debería subir a 8, pero 4 está bien por ahora)».

## Decisión
- **El objetivo de px es un mínimo:** una celda se refina solo si sus hijas seguirían midiendo al menos el objetivo
  en pantalla; los cubos dibujados miden entre el objetivo y el doble. Ningún cubo por debajo del objetivo, salvo que
  la fuente no tenga un nivel más grueso que lo cubra.
- **El mínimo en longitud está activo por defecto, 0,1 mm** en unidades de la nube (metros mientras no se lea la
  unidad del CRS, ADR 0049): una celda se refina solo si sus hijas medirían al menos ese lado. Acota el detalle por
  mucho zoom que se haga, y con él la cola de celdas de un punto bajo la separación (ADR 0053).
- **Memoria:** se baja todo lo posible dentro del presupuesto; si el corte no cabe, sube el objetivo de px efectivo de
  toda la nube (R3-Q11, ADR 0052).
- Predeterminados: 4 px y 0,1 mm (revisable a 8 px).
- Gana el filtro mayor en cada celda, como en la revisión del ADR 0049.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Mantener el objetivo como máximo (RQ-09) | Dibuja cubos más pequeños que el objetivo y gasta memoria en detalle que el propietario no quiere |
| Longitud mínima desactivada por defecto | Sin tope por mucho zoom: la cola bajo la separación crece hasta agotar la memoria |

## Consecuencias
- Cambian la regla de refinado del corte (sonda por octante) y la del modo celda (gemelo y WGSL), y los dorados del
  corte. Se hace en la fase 2b, antes del rango de niveles por nodo.
- Con el mismo objetivo se dibujan menos celdas (cubos de N a 2N px en vez de N/2 a N): menos memoria a igual
  número.
- RQ-09 pasa a decir «ningún cubo por debajo del objetivo de px»; la tolerancia del ADR 0034 se aplica al nuevo
  sentido.

## Revisar si
4 px resulta demasiado fino para la memoria en nubes grandes (subir a 8), o 0,1 mm recorta detalle útil en nubes
muy densas.
