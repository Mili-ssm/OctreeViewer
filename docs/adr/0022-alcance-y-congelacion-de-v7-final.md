# 0022 V7 final incluye R4, R5 y la rejilla global, y se congela una vez

Estado: Aceptada (umbral de conversión revisado por el [ADR 0043](0043-coste-de-conversion-de-v7-frente-a-v5.md); congelado y promoción ejecutados por el [ADR 0044](0044-v7-final-congelada-y-predeterminada.md)) · Fecha: 30-10-2026

## Contexto
El [plan V7](../plans/format-v7-plan.md) (l.21–22) reconocía que no había mínimos de hardware, almacenamiento ni tolerancias acordados antes de congelar el formato. Ahora R4 ([ADR 0020](0020-piramide-lod-densa.md)), R5 y la rejilla global ([ADR 0021](0021-rejilla-global-determinista-y-crs.md)) cambian el formato.

## Decisión
- R4, R5 (jerarquía paginada) y la rejilla global entran en V7 como nuevas revisiones `v7.N-pre` ([ADR 0005](0005-identidades-exactas-v7n-pre-por-candidato.md)).
- V7 final se congela cuando estén dentro y se cumpla RQ-07: latencia de lectura y navegación igual o mejor que V5; tiempo y RSS de conversión ≤ +30 %; disco, solo lo que añada la pirámide.
- Una sola regeneración de datasets. V5 sigue siendo el valor por defecto hasta el congelado ([ADR 0002](0002-v5-predeterminado-en-transicion.md)).
- El formato conserva CRS, flags LAS y Extra Bytes (sin waveform). El escritor rechaza esquemas de más de 256 atributos ([ADR 0007](0007-limites-por-defecto-del-lector.md)).
- Codificador LZ4 propio ([ADR 0025](0025-codificador-lz4-propio.md)).
- Resuelve la carencia de objetivos del plan V7: los umbrales son RQ-07 y los escenarios EQ-REF/ESC-1.

## Alternativas
| Opción | Descartada porque |
| --- | --- |
| Promover V7.3 ya y dejar R4 para V8 | Dos regeneraciones de datasets. |
| Congelar con R4 y añadir R5 después como extensión compatible | Se pierde la ocasión de una sola regeneración; el propietario decidió incluir R5 en el congelado. |

## Consecuencias
- La promoción (fase 6 del plan V7) depende de R3–R5 y del congelado; el retiro de V5/V6 ([ADR 0004](0004-retirar-v5-v6-tras-v7.md)) la sigue.
- Si los bytes del codificador propio difieren de la bifurcación, las SHA de referencia R1 cambian: se hace antes del congelado.
- **Pendiente:** ninguna revisión `v7.N-pre` nueva existe todavía; el plan V7 aún no refleja este alcance.

## Revisar si
RQ-07 no se cumple con la pirámide densa, o R5 exige un cambio incompatible tras el congelado.

## Evidencia
[Plan V7](../plans/format-v7-plan.md) · [Requisitos RQ-07, RF-24](../requirements.md)

## Revisión 15-11-2026
El congelado se ejecuta con el [ADR 0044](0044-v7-final-congelada-y-predeterminada.md). Por decisión del propietario, el codificador LZ4 propio queda fuera y va después ([ADR 0025](0025-codificador-lz4-propio.md#revisión-15-11-2026)); RQ-07 se mide con V7 antes de cambiar el valor por defecto.
