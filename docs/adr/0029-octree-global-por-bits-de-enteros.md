# 0029 Octree global sobre coordenadas enteras con cuanto binario

Estado: Aceptada · Fecha: 01-11-2026 · Decide: propietario · Precisa [ADR 0021](0021-rejilla-global-determinista-y-crs.md)

## Contexto
- ADR 0021 ancla la rejilla en el CRS para que las celdas de dos nubes coincidan, pero deja pendientes el ancla, la unidad y los ejes.
- El pre-mortem del [plan de R4](../plans/render-r4-piramide-densa.md) mostró dos fallos de una rejilla anclada solo en la cuantización: los nodos seguían saliendo de la caja de cada nube (`cube_bounds`, `viewer_convert_core/src/spatial.rs:1953`), y una caja que cruza una potencia de 2 grande obligaba a agrandar la raíz.

## Decisión
- Cada punto se convierte a un entero global por eje: `q = round(c × 2^15)`, con `c` en unidades del CRS (cuanto 2⁻¹⁵, ~0,03 mm si la unidad es el metro). Es una multiplicación exacta por una potencia de 2; el entero es `i64`.
- El octree está implícito en los bits de `q`: un nodo de nivel *m* es un prefijo de bits común en los tres ejes, y su lado es una potencia de 2 en cuantos. Dos nubes con el mismo CRS comparten nodos y celdas en todos los niveles (RF-23).
- Se omite la cadena superior de nodos con un solo hijo: la raíz guardada es el primer nodo con más de un hijo, identificado por su nivel y su prefijo. Ninguna caja agranda la raíz.
- Cada bloque guarda los puntos como desplazamientos respecto al origen de su nodo (`u32` por eje: nodos de hasta 2^32 cuantos = 131 km de lado; más allá se parte el nodo).
- Los metadatos guardan la escala y el offset LAS de origen: si la escala es ≥ 10⁻⁴, la exportación redondea a la rejilla de origen y devuelve los enteros LAS exactos.
- Al fusionar nubes con el mismo CRS solo se regeneran los nodos que comparten prefijo (RF-21).

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Cuanto decimal de 0,1 mm | Convertir exige multiplicar por 10ⁿ con redondeo, no solo cambiar el exponente; mismo resultado de exportación |
| Rejilla solo para nodos, puntos por bloque como hoy | Dos marcos de coordenadas, y la fusión no puede reutilizar los bloques |
| Raíz anclada en la caja de cada nube y agrandada a 2^m | Una caja que cruza 2^22 (UTM) añade unos 10 niveles vacíos |

## Consecuencias
- Cambia el marco de coordenadas del formato (`v7.4-pre`) y del visor: el origen de render pasa a ser un prefijo del árbol.
- Precisión de ~0,03 mm frente a menos de un µm hoy. La exportación sigue siendo exacta para escalas LAS ≥ 10⁻⁴; las escalas más finas pierden exactitud.
- La profundidad absoluta llega a 64 bits: la profundidad de celda de R3 (`MAX_CELL_DEPTH`) pasa a ser relativa a la raíz guardada.

## Revisar si
Una fuente real necesita una escala menor que 10⁻⁴, o un CRS tiene unidades en las que 2⁻¹⁵ no basta (por ejemplo, grados).
