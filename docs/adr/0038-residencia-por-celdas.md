# 0038 Residencia por celdas en la ruta de celdas

Estado: Aceptada · Fecha: 14-11-2026 · Decide: propietario (R3-Q28) · Enmienda el [ADR 0037](0037-corte-ajustado-al-presupuesto-de-cpu.md)

## Contexto
- El planificador solo cuenta un nodo como residente si sus bloques decodificados siguen en la caché de bloques (`lod_transition.rs:157`, `cache.peek(..).expect("resolved cut blocks are resident")`), también en la ruta de celdas, donde el nodo ya tiene sus celdas construidas.
- Majadas a 1 px dentro de una hoja necesita ~518 MB de bloques para su corte, y la caché del navegador es de 256 MiB (`main.js`). La caché se llena de bloques protegidos, el corte no avanza y los nodos construidos fuera de él se olvidan y se reconstruyen ([T11](../records/benchmarks/R3_T11_SMOKE_13-11-2026.md); test `majadas_at_1px_settles_with_the_browser_block_cache`). Las simulaciones nativas no lo veían porque usaban una caché de 2 GiB.
- El ajuste del ADR 0037 no se activa: el límite que manda es la caché de bloques, no el presupuesto de CPU de las celdas.

## Decisión
- **En la ruta de celdas, un nodo con celdas construidas cuenta como residente para el plan aunque sus bloques hayan salido de la caché.** Los bloques solo hacen falta para construir las celdas; después pueden expulsarse.
- La caché de bloques ya no protege los bloques de los nodos con celdas.
- En la ruta de celdas, el presupuesto que acota el corte es el de bytes de celdas del ADR 0037, no el de puntos de la ruta CPU.
- La ruta CPU no cambia.

## Alternativas
| Opción | Por qué no |
| --- | --- |
| Segundo presupuesto con los bytes protegidos de la caché de bloques | Conserva la memoria duplicada (bloques y celdas) y deja esa vista en una etapa más gruesa |
| Caché de bloques de 768 MiB–1 GiB | La pestaña llegaría a ~1,5 GB y otra nube o pose volvería a llenarla |

## Consecuencias
- El planificador necesita un predicado de residencia propio de la ruta de celdas.
- Lo que necesite los bloques después de construir (picking de la ruta CPU, color por atributo de la Tarea 12) debe leerlos de nuevo o de las celdas; se comprueba en esas tareas.
- La memoria de la ruta de celdas queda acotada por el presupuesto de celdas, más la caché de bloques como zona de carga.

## Revisar si
- La Tarea 12 necesita los bloques decodificados de nodos ya construidos.
- La Tarea 17 mide expulsiones de la caché de bloques en navegación normal que retrasan el objetivo.
