# Índice de documentación

Punto de entrada del proyecto: [README](../README.md) (portada en inglés); [estado y prioridades](estado.md); lo pendiente, en [issues](https://github.com/Mili-ssm/OctreeViewer/issues). Marcas:
**vivo** = se mantiene con el código; **histórico** = registro fechado, no se edita ([política](../CONTRIBUTING.md#convenciones)).

## Requisitos

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Requisitos](requirements.md) | Propósito, usuarios, escenarios de escala, requisitos EARS, restricciones, supuestos y preguntas abiertas. | vivo |
| [Estado](estado.md) | Estado por línea de trabajo, prioridades, orden general y uso del conversor. | vivo |

## Referencia

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Inventario](reference/inventory.md) | Mapa de componentes, identidades de formato, herramientas, límites y candidatos. | vivo |
| [Formato V8](reference/format/FORMAT_V8.md) | **Vigente y único**: especificación del `.oct`/`.octattrs` V8 (identidad, marco global, índice paginado, bloques, códecs LZ4 y errores de los formatos retirados; [ADR 0058](adr/0058-formato-v8-experimental.md), [0060](adr/0060-v8-solo-lz4.md), [0061](adr/0061-ept-y-checkpoints-se-retiran-con-la-v8.md)). | vivo |
| [Formato V5](reference/format/FORMAT_V5.md) | Especificación del V5, formato retirado con la V8 (D4). | histórico |
| [Formato V7](reference/format/FORMAT_V7.md) | Especificación de la identidad final `v7` (disposición de v7.5-pre, magics propios), retirada con la V8. | histórico |
| [V7.1-pre](reference/format/FORMAT_V7_PRE1.md), [V7.2-pre](reference/format/FORMAT_V7_PRE2.md), [V7.3-pre](reference/format/FORMAT_V7_PRE3.md), [V7.4-pre](reference/format/FORMAT_V7_PRE4.md), [V7.5-pre](reference/format/FORMAT_V7_PRE5.md) | Especificaciones de las revisiones V7 experimentales (v7.5: índice paginado, R5), retiradas con la V8. | histórico |
| [V8.1-pre](reference/format/FORMAT_V8_PRE1.md) | Revisión previa de la V8 (cuantiles de distancia, columnas LOD, color de pantalla derivado, reglas de almacenamiento por bloque); la V8 vigente usa su disposición. | histórico |
| [V8.2-pre](reference/format/FORMAT_V8_PRE2.md) | Revisión previa de la V8: v8.1-pre más zstd opcional en `.octattrs` (`--attr-codec`), experimento de D3 que no pasó a la V8 ([ADR 0060](adr/0060-v8-solo-lz4.md)). | histórico |

## Planes y specs

Cada plan lleva en la línea 3 `Estado: activo | en curso | aprobado | pendiente | en pausa | completado | cerrado · dd-mm-aaaa`
(`grep -n '^Estado:' docs/plans/*.md` da el estado de todos). Un plan cerrado (`cerrado` o `completado`; uno «completado salvo…» sigue aquí hasta cerrar lo que le falta)
pasa a [records/archive/](records/archive/) con sus enlaces entrantes corregidos en el mismo cambio
(decisión del propietario, 18-11-2026); no se reescribe: el trabajo nuevo va en un plan nuevo.

| Documento | Contenido | Estado |
| --- | --- | --- |
| [Mejoras y pendientes (13-11-2026)](plans/mejoras-pendientes-13-11-2026.md) | Orden de trabajo tras la revisión de técnicas: higiene, cierre de R3, velocidad de conversión y robustez remota. | activo (19-11-2026; D3c y D4 hechos) |
| [Nivel pedido y LOD por celda](plans/nivel-pedido-y-lod-por-celda.md) | Niveles a demanda (#35), LOD por celda según distancia (#44) y selector de tamaño mínimo (ADR 0048–0050, 0052 y 0053); 2c medida; el cierre espera OQ-17 (#99). | en curso |
| [Fase 1: niveles a demanda](plans/fase-1-niveles-a-demanda.md) | Plan detallado de la fase 1 (#35): rango construible, peticiones de nivel entre workers y construcción a demanda. | completado salvo el p95 (#54) |
| [Conversor web paralelo](plans/conversor-web-paralelo.md) | Spec: paridad byte a byte con el CLI, núcleo sin I/O y pool de workers hasta escala Majadas; siguiente hito M6. | en pausa |
| [Conversión paralela (CLI y navegador)](plans/conversion-paralela.md) | Camino crítico medido, cola de listos del CLI en W hilos, spools comprimidos en la web y pool de workers con conductor por pasos; sustituye al plan anterior. | aprobado |
| [Temporales del conversor](plans/temporales-del-conversor.md) | Bajar el disco temporal del CLI de 11 × a ≤ 2 × el LAZ (RQ-06) sin cambiar la salida: spools comprimidos (LZ4 por planos) y escritura por subárbol; fase B fusionada (#108, 3,5 ×); la A, aparcada (#109). | en pausa |
| [Temporales, fase A](plans/temporales-del-conversor-fase-a.md) | Plan de implementación de la escritura por subárbol: codificador sin jerarquía, escritor en flujo, sumidero de bloques y un hijo de la raíz cada vez. | en pausa (#109) |

## Explicación

| Documento | Contenido | Marca |
| --- | --- | --- |
| [Resolución regional LOD](explanation/REGIONAL_VOXEL_LOD.md) | Contrato de celdas, fallback y regresiones del LOD Replacement. | vivo |
| [Resúmenes por nodo](explanation/NODE_ATTRIBUTE_SUMMARIES.md) | Contenedores experimentales de resúmenes y diccionario conjunto. | vivo |

## Registros

[Índice de registros](records/README.md): medidas, spikes, evaluaciones y documentos retirados, por línea de trabajo; inmutables.

## Decisiones

[ADR](adr/README.md): decisiones de formato, versiones, límites, toolchain, seguridad y conversión web, con alternativas y condiciones de revisión (vivo).

## Datos y recursos

[Datasets de escala](../data/datasets/README.md) y [muestras](../data/samples/README.md) (procedencia y preparación); [host web](../examples/web-host/README.md) (API, build, probes; en inglés); [capturas](point-cloud-previews/) de nubes de ejemplo.

## Dónde va cada cosa

| Contenido nuevo | Destino | Regla |
| --- | --- | --- |
| Requisito, restricción, supuesto o pregunta abierta para el propietario | [requirements.md](requirements.md) | IDs estables (`RF-`, `RQ-`, `CON-`, `ASM-`, `OQ-`); no se renumeran |
| Decisión de diseño duradera | [adr/](adr/README.md) | `Estado:` en la línea 3; una aceptada se sustituye o enmienda con un ADR nuevo |
| Spec o plan de implementación | [plans/](plans/) | `Estado:` en la línea 3; enlazarlo en este índice; al cerrarlo pasa a `records/archive/` |
| Contrato vigente (formato, API, límites) | [reference/](reference/) | Se actualiza en el mismo cambio que el código |
| Por qué funciona algo así, sin pasos | [explanation/](explanation/) | Vivo; los resultados fechados van a registros |
| Medida, spike, evaluación o diagnóstico | [records/benchmarks/](records/benchmarks/) | `NOMBRE_dd-mm-aaaa.md`, inmutable; perfil, equipo y revisión dentro; su fila va en [records/README.md](records/README.md) |
| Documento retirado o sustituido, o plan cerrado | [records/archive/](records/archive/) | Inmutable salvo enlaces rotos; su fila va en [records/README.md](records/README.md) |
| Estado y prioridades activas | [Estado](estado.md#prioridades-pendientes) | Resumen con enlaces a issues |
| Tarea pendiente, fallo, hallazgo aparcado o decisión que espera al propietario | [Issue de GitHub](https://github.com/Mili-ssm/OctreeViewer/issues) | Plantillas y etiquetas en [CONTRIBUTING](../CONTRIBUTING.md#issues-y-prs); el `.md` enlaza la issue |
