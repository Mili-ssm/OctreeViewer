# R3: scratch compacto para una región (05-11-2026)

**Resultado observado:** en tres parejas de Belvedere, el tiempo compacto/base tuvo mediana 0,884 y rango 0,881–0,884; en Majadas hubo una sola pareja (0,862). Son muestras nativas de CPU, no una conclusión general de rendimiento ni evidencia de GPU o navegador. El cambio es experimental y no cierra ninguna puerta de R3.

## Alcance e identidad

El experimento sustituye el registro temporal de clave de 80 por 48 bytes cuando el nodo cabe en una región; conserva la ruta ancha para el resto. Es una estimación nominal del payload temporal, no capacidad asignada ni RSS; no cambia el formato ni los datos en disco. El `NodeCell` final sigue en 80 bytes. Base Git `3eb78f70f2a90d5a8b7b68d39efc5fd406d2983d`, con cambios locales previos en la base experimental. Perfil `perf`, recorrido completo (`max_rows=0`), `/usr/bin/time -v`.

| Identidad SHA-256 | Valor |
|---|---|
| Binario base `/tmp/r3-adaptive-benchmark/subregions-fast-node-cells` | `d72d4f7655e722012b318dd8ea46d98d58eaf3f464ef1cc964cf9b88319567b6` |
| Binario compacto `/tmp/r3-layout-narrow-node-cells` | `9a7daf7bb9171e3c9438a15b3951eee01f3784ae7f8e2183bdfa2e07e4449f8a` |
| Diff completo frente a HEAD, coincidente con la huella registrada durante la medida | `35a37a5a18ecdc1a8fc5c9ec7ae704c04ac08cccbfc2ef7cb91ecab766a997d7` |
| Parche incremental `/tmp/r3-layout-narrow.patch` | `96324a28e6bb970d349cd4f05ed18cdce2d9608e217900502ea29644c91c3250` |
| Belvedere `.oct` / `.octattrs` | `101464e9152145270f525f3be6818e702aca5c8bd9456e133641d339c26ed148` / `a0d4d915b949273522512aa7e1e03b05e5debb49ec67f21fc259dd0cb6f07e1e` |
| Majadas `.oct` / `.octattrs` | `269d5df1e6da8485a0fd5ded3798a0f79bdde61ff298f624e5497dba6b29f1b1` / `cc253c28ff90b1d5361f5cdb781217f5d1d78b809b1736298066f16c7149b3fd` |

El parche incremental no es el diff completo cuya huella figura en los metadatos; se conservan ambos. Las huellas identifican los binarios y el árbol observado, pero no prueban por sí solas la procedencia de compilación de los binarios.

## Protocolo y muestras

AMD Ryzen AI 9 365, Linux 7.2.15 Fedora 45, Rust nightly 1.101.0 (27-10-2026). Ejecuciones seriales; afinidad permitida 0–19 sin fijación ni aislamiento, carga inicial 0,76/1,40/1,39. No había cargo, rustc, valgrind ni benchmark competidor antes de medir; no se descarta interferencia posterior. Belvedere: un calentamiento por binario (54,75 y 48,96 s), seguido por parejas base/compacto, compacto/base, base/compacto. Majadas: una pareja base/compacto. Todos los procesos retornaron 0.

| Dataset | Tiempos base → compacto (s) | RSS máxima base → compacto (kB) | Pico nominal temporal base → compacto (bytes) |
|---|---|---|---|
| Belvedere | 53,88 / 53,61 / 53,60 → 47,45 / 47,40 / 47,40 | 70.440 / 70.524 / 70.704 → 65.828 / 65.264 / 65.644 | 11.547.600 → 6.928.560 |
| Majadas | 361,66 → 311,70 | 230.224 → 218.400 | 30.226.800 → 18.136.080 |

En Belvedere las medianas de tiempo son 53,61 → 47,40 s y las de RSS 70.524 → 65.644 kB. La mediana y rango de ratios se calculan **por pareja** (0,88066; 0,88416; 0,88433), no mediante el cociente de medianas. En Majadas una sola pareja no permite estimar variabilidad.

## Comprobación y límites

Los resúmenes coinciden en 232.396.472 celdas para Belvedere y 1.384.873.569 para Majadas, además de sus totales de payload CPU y GPU proyectado y distribuciones impresas. Es igualdad de resúmenes, no prueba de igualdad de contenido celda a celda. Ambos recorridos Majadas verificaron 24.091 IDs de jerarquía paginada. El check registrado terminó con 24 pasos aprobados; la paridad de bloques reales se omitió por falta del dataset. En el log aparecen 25 tests aprobados en cada una de las dos ejecuciones correspondientes. Estas pruebas no demuestran equivalencia exhaustiva ni rendimiento web.

Las [capturas y parches comprimidos](data/r3-layout-compact-05-11-2026/) incluyen órdenes, stdout, stderr, tiempos, códigos, metadatos, cronología y log de checks. Se sustituyó el nombre de usuario en las rutas de las capturas; los parches se conservan exactos. La cronología tiene campos `END` mal formados; los archivos `.rc` separados registran el resultado de cada proceso.
