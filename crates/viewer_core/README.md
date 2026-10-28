# viewer_core

Núcleo independiente de plataforma: geometría, marco local, atributos tipados, jerarquía,
selección LOD, scheduler y caché de bloques acotada. No hace I/O ni renderiza.
Entrada: `DatasetSource` (bloques) y el contrato LAS compartido en `las`. Feature `experimental`
apagada por defecto. Mapa: [inventario](../../docs/reference/inventory.md).
