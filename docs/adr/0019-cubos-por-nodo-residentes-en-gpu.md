# 0019 Cubos 3D por nodo, independientes de la vista y residentes en GPU (R3)

Estado: Aceptada · enmendada por [0033](0033-gpu-minima-4-gb-y-corte-ajustado-al-pool.md) (GPU mínima); modo de huecos sustituido por el selector de tamaño mínimo del [ADR 0049](0049-tamano-minimo-del-voxel.md) · Fecha: 30-10-2026

## Contexto
- La voxelización en CPU por vista del corte completo es el cuello de botella B1 de la [evaluación](../records/archive/RENDER_ASSESSMENT_2026-10-30.md).
- La recomendación técnica era splats orientados a pantalla ([P1](../requirements.md#resoluciones-del-30-10-2026)); el propietario exige **cubos 3D**.
- Hardware de referencia EQ-REF: iGPU con ~2 GB compartidos y 16 GB de RAM; PSS de la pestaña ≤ 8 GiB (RQ-05).

## Decisión
- Geometría por nodo **independiente de la vista** y residente en GPU: cubos instanciados alineados con la rejilla global ([ADR 0021](0021-rejilla-global-determinista-y-crs.md)), con celdas de los niveles de la pirámide densa ([ADR 0020](0020-piramide-lod-densa.md)); el tamaño en pantalla se calcula en el shader. Sustituye a la voxelización CPU por vista.
- **Huecos:** modo literal por defecto (no se dibuja donde la fuente no tiene puntos). Continuidad opcional por nube: tamaño mínimo de cubo derivado de la densidad global de esa nube, no por región.
- **Modos de px:** fijo (por defecto), con refinamiento progresivo hacia el objetivo sin esperar a completarlo; dinámico, que ajusta el objetivo para sostener 16,7 ms por frame con histéresis.
- Se mantiene el contrato de celdas sin solape ([REGIONAL_VOXEL_LOD](../explanation/REGIONAL_VOXEL_LOD.md)); cómo lo cumple la GPU lo fija el plan de R3.

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Splats orientados a pantalla | El propietario exige cubos 3D (P1). |
| Rasterización por compute a 1 px | La recomendación la reservaba a 1 px con miles de millones de puntos; el rango acordado es 1–8 px con cubos. |
| Mantener voxelización CPU con el worker de construcción | Sigue leyendo 1 100–2 700 puntos por vóxel y no escala a ESC-1 ([ADR 0018](0018-worker-de-construccion-por-defecto.md) la mantiene solo hasta R3). |

## Consecuencias
- Más vértices que con splats en una iGPU de ~2 GB.
- Depende de R4 (niveles densos) y de la rejilla global.
- Las capas de render propias (RF-15) comparten el pase.
- La pérdida de dispositivo ([ADR 0010](0010-migracion-a-wgpu-30-y-glam-0-34.md)) sigue abierta.
- **Pendiente:** todo; no hay código de R3 ni plan detallado.

## Revisar si
Los cubos a 4 px no cumplen RQ-01 en EQ-REF.

## Evidencia
[Preguntas P1, P2, P4, P5](../requirements.md#resoluciones-del-30-10-2026) · [Plan de render, R3](../records/archive/render-y-carga-escala.md) · [Requisitos RF-03 a RF-07](../requirements.md)
