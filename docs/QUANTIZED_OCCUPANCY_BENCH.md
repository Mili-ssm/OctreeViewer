# Ocupación cuantizada y residuales: experimento acotado

Estado: 14-10-2026. Candidato **en memoria**, independiente de la revisión de almacenamiento.
Esta serie anterior a pre2 no cambió v5/pre1. La [entrega física pre2](STORAGE_BENCH.md) posterior
sí serializa el codec y evalúa bytes comprimidos; la memoria de este informe no decide el disco.
Aquí se valida una partición exacta de los
u32 originales y se mide su coste antes de decidir un codec completo.

## Protocolo

[Informe con comandos, hashes y muestras crudas](benchmarks/quantized-occupancy-prefixes-2026-10-14.json).
AMD Ryzen AI 9 365, Linux x86_64, perfil `perf`. Cuatro fuentes LAS/LAZ locales, prefijo de
32768 puntos en orden de fuente por archivo. Son prefijos, **no bloques espaciales de conversión**
ni una muestra representativa de cada dataset completo. Las coordenadas se leen como f64, se
restan del mínimo de cabecera y se cuantizan con error máximo permitido 0,01 antes de f32.
LAS/LAZ y cuantización quedan fuera de los cronómetros. No se leen ni requantizan posiciones f32
del `.oct`. El informe conserva origen/escala/error efectivos de cuantización y checksum CRC32 de los u32.

Tres procesos por fuente, cinco repeticiones dentro de cada proceso y seis perfiles de bits
0/8/16/24/28/32, invirtiendo el orden en repeticiones pares: **15 muestras por perfil/fuente,
360 roundtrips exactos**. Compilación, tests y Clippy terminaron antes de medir. Frecuencia y
carga externa no aisladas; no se afirma significancia estadística ni aceleración del viewer.

`build_ms` incluye empaquetar residuales y construir índice/topología. `reconstruct_ms` incluye
la asignación del vector de salida y la restitución de u32 por fila; comparación y checksum
quedan fuera. `estimated_memory_bytes` suma datos inline y capacidades de vectores, usando
los anchos reales de este proceso nativo (`usize` de 64 bits). Excluye allocator, fuente simultánea,
vector reconstruido, temporales y pico de construcción: **no es RSS ni pico conjunto**.

La fuente retiene 393296 B en todos los casos, incluidos 393216 B de XYZ u32 y el descriptor.
El candidato cuenta metadata, topología por nivel, offsets, IDs de fila y residuales empaquetados.
Estos bytes no equivalen a archivo comprimido; sólo el stream residual está materializado en
bytes. No comparar su longitud sola con una pareja `.oct`/`.octattrs`.

## Resultados

Cada tiempo es la mediana de 15 muestras; mínimos/máximos están en el JSON. Hojas/nodos y bytes
son deterministas en las repeticiones. El perfil de menor memoria difiere según el prefijo:
no se introduce selección automática de bits.

| Fuente | Bits con menor memoria | Fuente retenida (B) | Candidato retenido (B) | Factor |
| --- | ---: | ---: | ---: | ---: |
| Outer Banks e430 | 28 | 393296 | 618344 | 1.57× |
| Belvedere | 28 | 393296 | 607560 | 1.54× |
| Majadas | 24 | 393296 | 559208 | 1.42× |
| Albuera | 28 | 393296 | 626088 | 1.59× |

### Outer Banks e430

| Bits residuales | Hojas | Nodos | Residuales (B) | Retenido candidato (B) | Construcción (ms) | Reconstrucción (ms) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 32768 | 835804 | 0 | 14160264 | 7.179 | 8.940 |
| 8 | 32768 | 573660 | 98304 | 10064072 | 5.462 | 6.137 |
| 16 | 32768 | 311516 | 196608 | 5967880 | 3.702 | 3.435 |
| 24 | 27652 | 49930 | 294912 | 1618472 | 1.897 | 0.707 |
| 28 | 370 | 484 | 344064 | 618344 | 1.167 | 0.174 |
| 32 | 1 | 1 | 393216 | 655608 | 0.325 | 0.169 |

### Belvedere

| Bits residuales | Hojas | Nodos | Residuales (B) | Retenido candidato (B) | Construcción (ms) | Reconstrucción (ms) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 32628 | 731365 | 0 | 12227096 | 13.305 | 16.631 |
| 8 | 32628 | 470341 | 98304 | 8148824 | 10.070 | 10.995 |
| 16 | 32628 | 209317 | 196608 | 4070552 | 6.821 | 5.111 |
| 24 | 1557 | 2289 | 294912 | 610456 | 2.966 | 0.456 |
| 28 | 24 | 50 | 344064 | 607560 | 2.449 | 0.343 |
| 32 | 1 | 1 | 393216 | 655608 | 0.692 | 0.364 |

### Majadas

| Bits residuales | Hojas | Nodos | Residuales (B) | Retenido candidato (B) | Construcción (ms) | Reconstrucción (ms) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 32765 | 619359 | 0 | 10435000 | 6.532 | 6.501 |
| 8 | 32765 | 357239 | 98304 | 6339192 | 4.656 | 3.846 |
| 16 | 32738 | 95119 | 196608 | 2243384 | 2.978 | 1.139 |
| 24 | 34 | 78 | 294912 | 559208 | 1.220 | 0.172 |
| 28 | 4 | 11 | 344064 | 606744 | 1.003 | 0.160 |
| 32 | 1 | 1 | 393216 | 655608 | 0.327 | 0.171 |

### Albuera

| Bits residuales | Hojas | Nodos | Residuales (B) | Retenido candidato (B) | Construcción (ms) | Reconstrucción (ms) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 32768 | 834861 | 0 | 14145176 | 9.380 | 10.417 |
| 8 | 32768 | 572717 | 98304 | 10048984 | 5.987 | 7.970 |
| 16 | 32768 | 310573 | 196608 | 5952792 | 4.176 | 4.115 |
| 24 | 26836 | 49657 | 294912 | 1614104 | 2.306 | 0.737 |
| 28 | 538 | 712 | 344064 | 626088 | 1.439 | 0.212 |
| 32 | 1 | 1 | 393216 | 655608 | 0.348 | 0.173 |

## Decisión y cierre pendiente

La exactitud está verificada, pero **ninguna partición medida reduce memoria retenida frente
al vector original**. La mejor da 1,42–1,59×; las topologías a profundidad 32 alcanzan ~36×.
El empaquetado residual no compensa por sí solo los nodos con índice `usize`, los arrays
de hojas, offsets y la permutación de filas. Esta observación no predice el tamaño de un codec
comprimido que derive esos índices en lugar de persistirlos.

Siguiente experimento: máscaras compactas por nivel y derivación de índices/offsets; evaluar
filas por rangos implícitos o codificación compacta. Materializar **todos** los componentes y
validar su decodificación antes de comparar bytes con raw/delta+LZ4. Añadir límites de lectura
y medir acceso selectivo y pico simultáneo antes de integrar otro backend pre. La grilla física
común sigue requiriendo CRS/unidades/ejes; el entero sesgado de este candidato no la resuelve.

## Reproducir

```sh
cargo build --profile perf --offline -p viewer_convert --example quantized_occupancy_bench
target/perf/examples/quantized_occupancy_bench \
  data/datasets/belv_2009_digital-aerial_pcd_copc.laz 32768 5 0,8,16,24,28,32
```

Repetir el proceso tres veces por fuente. El límite explícito del ejemplo es 65536 puntos y
100 repeticiones; rechaza bits fuera de 0–32 antes de leer. No limita el RSS interno del decoder
LAZ. Los datasets no se redistribuyen; las rutas y hashes están en el informe.
