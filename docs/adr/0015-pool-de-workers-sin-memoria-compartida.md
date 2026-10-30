# 0015 Paralelismo de conversión web por pool de workers sin memoria compartida

Estado: Aceptada · enmendada por [0024](0024-hilos-wasm-opcionales-con-coop-coep.md) · Fecha: 29-10-2026

## Contexto
El visor se sirve con `python3 -m http.server` y como paquete `file://` con workers `blob:`
(`examples/web-host/package-local.py`). Los hilos wasm con memoria compartida exigen
`SharedArrayBuffer` y aislamiento cross-origin (cabeceras COOP/COEP), que ninguno de los dos modos
ofrece. Una instancia wasm32 tiene como máximo 4 GiB de memoria; el propietario acepta 4–8 GB de
RAM durante la conversión.

## Decisión
La conversión web reparte trabajos entre un pool de Web Workers, cada uno con su propia instancia
wasm32, comunicados por mensajes con `ArrayBuffer` transferidos y `Blob` clonados. Un coordinador es
el único escritor de la salida. La [ADR 0009](0009-transporte-web-por-blob-local.md) evitaba
concurrencia por extrapolación: aquí cada hito exige medidas de navegador antes y después.

## Alternativas consideradas
| Opción | Descartada porque |
| --- | --- |
| Hilos wasm (`wasm-bindgen-rayon`) con COOP/COEP | Rompe `http.server` y `file://`; memoria única de 4 GiB; toolchain con `build-std`. |
| Hilos ahora y pool como alternativa | Dos rutas que mantener sin requisito de servir con cabeceras. |

## Consecuencias
Funciona en ambos modos de entrega y permite repartir más de 4 GB entre instancias. Coste: trabajos
serializables, copias entre workers y un escritor único que puede limitar la escala. No se aplica a
la lectura del visor, que tendrá decisión propia.

## Revisar si
La entrega admite cabeceras COOP/COEP, o las medidas muestran que el escritor único domina.

## Evidencia
[Spec del conversor web paralelo](../plans/conversor-web-paralelo.md)

## Revisión 30-10-2026
- [ADR 0024](0024-hilos-wasm-opcionales-con-coop-coep.md): si la página está aislada, pueden usarse hilos wasm con memoria compartida; este pool sigue siendo la ruta general.
- M4/M5 (varios workers) solo si M6 con un worker supera 60 min o la experiencia lo exige ([requisitos, «Orden de trabajo»](../requirements.md)).
- La lectura del visor se decidió en la [ADR 0018](0018-worker-de-construccion-por-defecto.md).
