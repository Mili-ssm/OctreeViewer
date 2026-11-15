# R3: layout de páginas por anchura de región — 05-11-2026

**Observado:** la mejor variante en los dos datasets completos es **W = 10 con los flags en la página**: instancia de **12 B**, 12,19 B por celda en Belvedere y 12,95 en Majadas, con la vista cercana como indicador. La variante de 10 bits con los flags en la instancia, equivalente a la instancia de 16 B del ADR 0031, da 16,19 y 16,92 B/celda. La diferencia es de **−25 % y −23 %** de bytes nominales de GPU. W = 8 depende del dataset: es buena en Belvedere (12,95) y mala en Majadas (24,59 cerca y 37,24 lejos), porque en Majadas queda sin usar el 51 % de los bytes de página. La codificación supone entre el 1,4 % y el 2,5 % del tiempo de construcción en todas las anchuras, así que el **cálculo no decide** y la puntuación la marca la memoria. Son bytes nominales calculados en CPU: no se ha medido la GPU ni el navegador. Plan: [r3-anchura-de-region](../../plans/r3-anchura-de-region.md), Tarea 1.

## Resultados (modelo b, reparto voraz en orden Morton)

Bytes por celda = (páginas × (8192 + 32)) / celdas. «Cerca» = nivel guardado más fino de cada nodo; «lejos» = el más grueso. Puntuación = media de (GPU cerca y lejos, normalizada) y (construcción + codificación, normalizada), frente a W = 10 con flags por página. Menor es mejor.

| W | Flags | Instancia | Belvedere cerca / lejos | Majadas cerca / lejos | Puntuación Belvedere / Majadas |
| ---: | --- | ---: | --- | --- | --- |
| 8 | página o instancia | 12 B | 12,95 / 13,55 | 24,59 / 37,24 | 1,041 / 1,646 |
| **10** | **página** | **12 B** | **12,19 / 12,30** | **12,95 / 13,86** | **1,000 / 1,000** |
| 10 | instancia | 16 B | 16,19 / 16,30 | 16,92 / 17,79 | 1,165 / 1,149 |
| 12, 16 | cualquiera | 16 B | 16,19 / 16,29 | 16,19–16,22 / 16,34–16,40 | 1,165 / 1,109–1,110 |
| 21 | página | 16 B | 16,19 / 16,29 | 16,19 / 16,34 | 1,165 / 1,109 |
| 21, 24 | instancia (24: cualquiera) | 20 B | 20,23 / 20,33 | 20,23 / 20,38 | 1,332 / 1,262 |
| 32 | página | 20 B | 20,23 / 20,33 | 20,23 / 20,38 | 1,332 / 1,262 |
| 32 | instancia | 24 B | 24,24 / 24,34 | 24,24 / 24,39 | 1,498 / 1,413 |

- **Bytes de página sin usar, vista cercana** (cola / (páginas × 8192)):
  - Belvedere: 0,6–1,1 % con W ≥ 10 y 7,0 % con W = 8.
  - Majadas: 7,0 % con W = 10 y flags por página, 5,1 % con flags por instancia, 0,6–1,0 % con W ≥ 12 y **51 %** con W = 8.
  - El criterio de parada «sobrecoste < 1 %» no se cumple con W = 10 en Majadas, así que la decisión no puede tomarse solo por la clase de palabra.
- **Los modelos de página casi coinciden:** el alineado (a) y el voraz (b) difieren en menos de 0,1 B/celda, salvo en dos casos:
  - W = 8: 13,69 frente a 12,95 en Belvedere y 26,67 frente a 24,59 en Majadas.
  - W = 10 en Majadas: unos 0,13 B/celda.
- **Clases de nivel:** los niveles internos caben siempre en una página de 2⁸ (como máximo 8 bits relativos), así que en ellos `W` solo cambia el tamaño de la instancia. La variación por reparto en páginas (cola y número de páginas) viene de la hoja final. Tras la poda, en Belvedere solo 1 nivel cuenta como «hoja no final».
- **Bits relativos máximos:** 22 en Belvedere y 21 en Majadas, con 0 alarmas por encima de 24. La comprobación `f32` no falla en ninguna anchura, porque ningún `local` llega a 2²⁴.
- **Tiempo de codificación:** el rango entre repeticiones es ≤ 1,9 % de la mediana, y las diferencias entre clases de palabra son mayores que ese rango. Con 1, 2, 3 y 4 palabras de posición (instancias de 12, 16, 20 y 24 B), Belvedere tarda unos 0,47, 0,58, 0,70 y 0,83 s. Aun así, la construcción tarda 32,7 s en Belvedere y 239,6 s en Majadas.
- **Hipótesis fuera de la puntuación:** con una máscara de 4³ por página está ocupado de media el 7–8 % de los sub-bloques con W = 8, y el 1,6–3,0 % con W ≥ 10. Una máscara de 64 bits por página permitiría descartar casi todo el volumen de cada página. Esto no se ha medido sobre el recorte ni sobre la GPU.

[Análisis completo](data/r3-region-layout-05-11-2026/analysis.txt), incluidos los bytes por clase y los modelos a y b.

## Supuestos y límites

- **Los flags por página** solo son válidos si las páginas son homogéneas en `has_intensity` y `has_class`. Eso es *inferido*: los atributos se buscan por bloque (`node_cells.rs:242-243`) y no se ha medido. `single_point` es por celda; la variante de flags por página supone que la GPU no lo necesita. La instancia de 12 B de W = 10 deja libres 2 bits de la palabra de posición.
- `PageEntry` de 32 B y el payload de 8 B (rgba8, intensidad `u16`, clase `i16`) son supuestos nominales. No se ha comprobado que una instancia de 12 B, con zancada de 3 `u32` en un `array<u32>`, funcione en WGSL.
- Los totales suman niveles que nunca están residentes a la vez. Los indicadores de vista cercana y lejana acotan el caso, pero no son un recorte real.
- El modelo (a) solo cuenta bytes: no se codifica ni se decodifica. El decodificador independiente basado solo en `u32` se comprueba en el modelo (b), en las 14 combinaciones.
- Medida en una sola ejecución por dataset; el tiempo de construcción no depende de `W`, porque el constructor no cambia. La Tarea 2 (coste de construcción con clave regional por `W`) no hace falta: la memoria decide.

## Identidad y reproducción

AMD Ryzen AI 9 365, Linux 7.2.15 (Fedora 45), Rust nightly 1.101.0 (41089d07a, 27-10-2026), perfil `perf`; ejecuciones en serie, sin otro benchmark en paralelo (había dos revisores leyendo ficheros). Belvedere: 1:36,5, RSS máxima de 69.112 KiB. Majadas: 11:27,5 y 236.784 KiB. Las dos terminaron con código 0.

Base `3eb78f7` + el [diff de crates de Codex](data/r3-region-layout-05-11-2026/base-codex-crates.patch) (SHA-256 `35a37a5a…`, el mismo de `R3_LAYOUT_COMPACT_05-11-2026`) + el [diff de fuentes rastreadas](data/r3-region-layout-05-11-2026/spike-source.patch) (`node_cell_origin`; SHA-256 `ebb6d58b…`, igual al del momento de la medida) + la [fuente del spike](data/r3-region-layout-05-11-2026/region_width_spike.rs.txt) (SHA-256 `9c853f38…`). Esa fuente solo difiere del binario medido en la cabecera `failed_pages` → `failed_levels` del bloque `f32_failures`, que cuenta niveles. La [identidad](data/r3-region-layout-05-11-2026/identity.txt) recoge el SHA del binario y los de los datasets, que coinciden con los de los registros previos. [Comando](data/r3-region-layout-05-11-2026/command.sh), salidas [Belvedere](data/r3-region-layout-05-11-2026/belvedere.tsv) y [Majadas](data/r3-region-layout-05-11-2026/majadas.tsv), y [script de análisis](data/r3-region-layout-05-11-2026/analyze.py).
