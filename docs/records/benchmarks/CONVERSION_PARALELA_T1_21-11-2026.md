# Conversión paralela, tarea 1: cola de listos en W hilos — 21-11-2026

**Estado: medido.** Tarea 1b del [plan de conversión paralela](../../plans/conversion-paralela.md); base y método en
[CONVERSION_PARALELA_T0](CONVERSION_PARALELA_T0_21-11-2026.md). Datos en bruto en
[`data/conversion-paralela-t1-21-11-2026/`](data/conversion-paralela-t1-21-11-2026/).

**Conclusión:**

- **(a) W = 1 no empeora: −1,2 % (mediana de las diferencias emparejadas B frente a A; rango −5,5 % a +0,3 %, 5 rondas
  intercaladas en Belvedere).** Medianas: A (`c9f4722`) 130,8 s, B (HEAD) 129,9 s. Está dentro del 3 % del plan.
- **(b) Aceleración de W = 8 frente a W = 4: ×1,26 en Belvedere (52,4 → 41,6 s) y ×1,19 en Majadas (323,7 → 273,0 s).
  De W = 16 frente a W = 8: ×1,07 (41,6 → 38,8 s) y ×1,06 (273,0 → 257,0 s).** Frente a W = 4, W = 16 da ×1,35 y ×1,26.
  Antes de la tarea 1, W = 8 frente a W = 4 era ×1,08 y ×1,05 (T0).
- **(c) Criterio del plan: Majadas con W = 8 frente a W = 4 es ×1,19 < 1,3 → no se cumple; la tarea 2 (partición por
  rangos de los nodos internos grandes, empezando por un spike) es la siguiente.** Una sola ejecución en Majadas por W
  (sin dispersión medida; W = 8 empezó con carga 3,8 y W = 4 con 1,2, así que una réplica podría mover el cociente, aunque
  haría falta un 9 % para llegar a 1,3); en Belvedere el rango de W = 8 es 39,4–42,8 s, es decir ×1,22–1,33 frente a la mediana de W = 4, y
  el modelo de T0 daba un tope de ×1,27–1,35 para Belvedere y ×1,28–1,34 para Majadas.
- **(d) La curva se aplana a partir de W = 8 (de 8 a 16 solo ×1,06–1,07), por dos límites que no dependen de W:** la
  jerarquía queda cerca del límite de dependencias del árbol y la partición de la raíz no escala. La jerarquía con W = 16
  mide 18,1 s (Belvedere) y 140,7 s (Majadas), igual que la cola libre del modelo sobre los tiempos por nodo de esa misma
  ejecución (17,9–18,0 s y 138,5 s), y la ruta crítica del árbol con esos tiempos es de 15,5 s y 122,7 s (el 86–87 % de
  la pared): el reparto desigual deja hilos ociosos (menos de 16 trabajos en curso) el 62–69 % del tiempo del modelo. La partición de
  la raíz (13,4 s y 56,7 s con W = 16) pasa a ser el 35 % y el 22 % del total. El valor por defecto de W sigue en 4; esto solo informa.
- **SHA idéntica en todas las ejecuciones.** `.oct` Belvedere `8518efc0…` en las 19 ejecuciones (coincide con T0);
  `.octattrs` Belvedere `20d08de0…` en las 19; Majadas `.oct` `d2d95238…` (coincide con T0) y `.octattrs` `285d7575…` en
  las 4 ejecuciones. `spatial_workers_used` = W pedido en las 18 ejecuciones del binario B (no hubo repliegue a un hilo;
  el ejemplo imprime ese campo del informe, no la línea de la CLI; el binario A no lo imprime).

## Condiciones

- Equipo del propietario: AMD Ryzen AI 9 365, 4 núcleos Zen 5 (hasta 5,09 GHz) y 6 Zen 5c (hasta 3,32 GHz), SMT 2
  (20 hilos), 30 GiB de RAM; btrfs con `compress=zstd:1`. Por encima de 4 hilos el trabajo cae también en núcleos lentos.
  Rama `feat/conversion-paralela`. Binario B: HEAD
  `d38372b` (toda la tarea 1a, `c9f4722..d38372b`, con la reserva de memoria y el repliegue a un hilo, no solo la cola, más una línea del ejemplo que imprime `spatial_workers_used`); binario A:
  `c9f4722` en un worktree temporal con su propio `CARGO_TARGET_DIR` (ya eliminado). Ambos
  `cargo build --offline --profile perf -p viewer_convert --example real_dataset_bench`, copiados a un directorio
  de trabajo para que no cambien durante la matriz. Cifras solo válidas para esa revisión, perfil, equipo y datasets.
- Comando por ejecución: `BIN=<binario> run.sh ETIQUETA ENTRADA.laz DIR_TRABAJO W 4`, que llama a
  `real_dataset_bench convert-profile ENTRADA SALIDA W 4` (4 rangos de raíz siempre explícitos),
  directorio de trabajo y salida en btrfs (`target/`), con `run.sh` (copiado de T0, añade la SHA del `.octattrs`) y orden
  y matriz en `driver.sh` (rutas absolutas del equipo del propietario; entradas: Belvedere
  `belv_2009_digital-aerial_pcd_copc.laz`, 618.390.994 bytes, y Majadas
  `SensOFOREST_2026_02_26_Majadas_pointcloud_TLS_20260503.laz`, 1.343.266.470 bytes): W = 1 con A/B en 5 rondas que alternan quién va primero; Belvedere W = 4, 8, 16 × 3 rondas
  intercaladas; Majadas W = 1, 4, 8, 16 × 1. Una conversión a la vez, nadie más usaba el equipo.
- Carga previa (`loadavg` 1 min en cada `.meta`): 1,2–3,3 en W = 1 y 1,2–7,0 en el resto (6,96 y 5,75 en
  W = 4 r2–r3, 7,0 en Majadas W = 1, residuo de la conversión anterior). Mismo equipo que T0, pero T0 vio cargas de otras sesiones.
- Ventanas de pared de `stage_*_ms`; jerarquía por nodo de `internal_spatial_node_event` con `analyze.py` (copia de T0 con
  una `deque` en la simulación, sin cambiar resultados, solo para que Majadas termine; su ruta crítica se corrigió el
  21-11-2026 para tomar los hijos de los enlaces al padre, porque el último campo del evento son bytes de spool, no ids).

## Curva (mediana; mín.–máx.), pared en segundos

Belvedere, 3 rondas (W = 1: 5 rondas, binario B); Majadas, 1 ejecución:

| Config. | Belvedere total | ×vs W = 4 | Partición raíz | Jerarquía | Bloques | Validación |
| --- | --- | --- | --- | --- | --- | --- |
| W = 1 (A, `c9f4722`) | 130,8 (130,0–139,2) | — | 13,3 | 73,6 | 33,2 | 11,2 |
| W = 1 (B, HEAD) | 129,9 (128,8–131,5) | — | 12,5 | 73,0 | 32,9 | 11,3 |
| W = 4 | 52,4 (51,7–52,4) | 1 | 13,5 | 25,1 | 10,8 | 2,8 |
| W = 8 | 41,6 (39,4–42,8) | 1,26 | 13,5 | 18,8 | 7,3 | 1,9 |
| W = 16 | 38,8 (36,9–39,5) | 1,35 | 13,4 | 18,1 | 5,7 | 1,5 |

| Config. | Majadas total | ×vs W = 4 | Partición raíz | Jerarquía | Bloques | Validación |
| --- | --- | --- | --- | --- | --- | --- |
| W = 1 | 833,1 | — | 57,1 | 551,6 | 167,7 | 56,6 |
| W = 4 | 323,7 | 1 | 47,7 | 198,2 | 63,1 | 14,7 |
| W = 8 | 273,0 | 1,19 | 49,2 | 161,6 | 51,9 | 10,2 |
| W = 16 | 257,0 | 1,26 | 56,7 | 140,7 | 52,2 | 7,3 |

Con W = 4 la jerarquía es la de T0 (24,1 y 196,6 s allí; 25,1 y 198,2 s aquí). El total de W = 4 en Belvedere sube de 49,4
a 52,4 s frente a T0 (la jerarquía es la misma; el resto de etapas no se
ha comparado una a una), diferencia no atribuida (las rondas W = 4 r2–r3 empezaron con carga 5,8–7,0). Majadas con W = 4 baja de 334,0 a 323,7 s.

## W = 1 emparejado (Belvedere, `real_dataset_bench convert-profile … 1 4`)

| Ronda | 1 | 2 | 3 | 4 | 5 |
| --- | --- | --- | --- | --- | --- |
| A (s) | 132,5 | 130,0 | 130,8 | 139,2 | 130,3 |
| B (s) | 128,8 | 130,4 | 129,9 | 131,5 | 128,8 |
| B frente a A | −2,8 % | +0,3 % | −0,7 % | −5,5 % | −1,2 % |

## Concurrencia de la jerarquía

Los `start_seq`/`end_seq` de los eventos de nodo no son un reloj ni un intervalo de ejecución (el fin de un nodo se asigna
al consolidar), y el muestreo de `/proc/stat` no separa núcleos ocupados; no hay perfil de hilos ocupados. Se usa
**la concurrencia media = suma del trabajo propio de los nodos de profundidad ≥ 1 ÷ pared de la jerarquía**, y el
**modelo de cola libre** de T0 sobre los tiempos por nodo de cada ejecución:

| Ejecución | Trabajo (s) | Pared jerarquía (s) | Concurrencia media | Modelo cola libre, mismos tiempos (s) | % del tiempo con < W trabajos en curso (modelo) | Predicción T0 con 8 hilos (s) |
| --- | --- | --- | --- | --- | --- | --- |
| Belvedere W = 4 | 87,9–89,4 | 24,8–25,2 | 3,5 | 24,7–25,1 | 18–20 | — |
| Belvedere W = 8 | 102,6–105,4 | 18,8–19,4 | 5,5 | 18,6–19,3 | 44–45 | 15,4–17,6 |
| Belvedere W = 16 | 139,6–139,9 | 18,0–18,1 | 7,7 | 17,9–18,0 | 67–69 | — |
| Majadas W = 4 | 620,2 | 198,2 | 3,1 | 195,6 | 30 | — |
| Majadas W = 8 | 762,6 | 161,6 | 4,7 | 160,9 | 51 | 129,8–141,6 |
| Majadas W = 16 | 1013,1 | 140,7 | 7,2 | 138,5 | 62 | — |

- La jerarquía real coincide con el modelo (±2 %): en el modelo, la cola de listos no deja tiempo por aprovechar (no hay perfil
  medido de hilos ocupados); lo que limita es la estructura del árbol (un hijo de la raíz con el 87–100 % de los bytes).
  Ruta crítica del árbol con los tiempos de cada ejecución: Belvedere 14,4 s (W = 8, r1) y 15,5 s (W = 16, r1); Majadas
  105,2 s, 115,1 s y 122,7 s (W = 4, 8, 16). Con W = 8 la jerarquía de Majadas no puede bajar de ~115 s sin partir nodos.
- **Se queda por encima de la predicción de T0** (18,8 s frente a 15,4–17,6 s; 161,6 s frente a 129,8–141,6 s) porque el trabajo
  por nodo crece con la concurrencia (suma de Belvedere 73 → 88 → 104 → 140 s con W = 1, 4, 8, 16; Majadas 552 → 620 → 763 →
  1013 s), algo que el modelo de T0 no tenía. De W = 4 a W = 8 la subida de Majadas (×1,23) se parece a la de la frecuencia
  media al sumar 4 núcleos lentos a 4 rápidos (~×1,21; inferido, no medido); el resto (SMT, memoria, E/S) no se separó (con tiempos de W = 8, T0 modeló 38,8 s y 260,5 s de
  total; medidos 41,6 s y 273,0 s).

## Lo que no se midió

- Perfil de hilos ocupados o concurrencia instantánea de trabajos de jerarquía (ver arriba).
- Dispersión en Majadas (1 ejecución por W); W = 2, 3, 6 y valores no potencias de dos; tmpfs con W alto.
- Causa de la subida del trabajo por nodo con W: no se fijaron hilos a núcleos (`taskset`) para separar núcleos lentos y SMT
  de la contención de memoria o de E/S.
- Réplica de Majadas con W = 4 y W = 8 a la misma carga previa.
- Navegador (tareas 3–5) y el efecto sobre el RSS pico con W alto (el presupuesto se calcula de antemano; no se registró el RSS).
- La suma de la partición de la raíz (rangos fijos, 4) frente a W: es el siguiente límite tras el árbol.
