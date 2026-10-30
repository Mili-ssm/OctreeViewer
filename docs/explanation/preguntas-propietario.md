# Preguntas pendientes para el propietario

Documento vivo: decisiones que el trabajo autónomo no puede tomar y que bloquean fases concretas.
Cada entrada indica qué bloquea, las opciones, la recomendación técnica y su evidencia. Al decidirse,
la respuesta pasa al plan o ADR correspondiente y la entrada se borra de aquí.

## Render y carga a escala (30-10-2026)

Contexto: [evaluación](../records/archive/RENDER_ASSESSMENT_2026-10-30.md) y
[plan](render-y-carga-escala.md). La fase R1 avanza sin estas respuestas.

| Id | Pregunta | Opciones | Recomendación | Bloquea |
| --- | --- | --- | --- | --- |
| P1 | ¿El aspecto de «vóxel» (cubo) es requisito, o basta con que cada muestra ocupe 1–8 px en pantalla? | Cubos instanciados (hoy) · quads/splats orientados a pantalla · rasterización de puntos por compute | Splats orientados a pantalla con tamaño en el shader; compute solo si 1 px con miles de millones de puntos es requisito | R3 |
| P2 | ¿Qué hardware mínimo fijamos? | iGPU con ~2 GB compartidos · dGPU de 8 GB; RAM de 8 o 16 GB | iGPU 2 GB + 16 GB RAM como mínimo (equipo de referencia: Radeon 880M) | R3, presupuestos |
| P3 | ¿Se acepta reconvertir los datasets a una pirámide LOD densa en V7 (más nodos y +15–35 % de disco estimado)? | Pirámide densa (un nivel por ×2 de densidad, hojas ≤ 16–64 k puntos) · mantener 8³ representantes | Pirámide densa: hoy se leen 1 100–2 700 puntos por vóxel dibujado | R4, R5 |
| P4 | Por debajo de la densidad de la fuente, ¿mostrar los huecos reales o agrandar las muestras para continuidad visual? | Fidelidad literal · continuidad (splats mayores donde falta densidad) | Continuidad por defecto con un modo «literal» conmutable | R3, R4 |
| P5 | ¿El rango 1–8 px es dinámico (p. ej. 8 px en movimiento y 1–2 px en reposo) o fijo por sesión? | Fijo · dinámico por movimiento · dinámico por tiempo de frame | Dinámico por movimiento con histéresis | R1 (parcial), R3 |
| P6 | ¿Los datasets de > 500 GB serán remotos (HTTP Range) o siempre locales? ¿Se pueden servir cabeceras COOP/COEP (hilos wasm)? | Local (`Blob`, ADR 0009) · remoto · ambos; con o sin COOP/COEP | Mantener local + preparar paginación; sin COOP/COEP salvo necesidad medida | R5, R2 |
| P7 | Con el worker de construcción, mientras la cámara se mueve, ¿se acepta una escena con un tamaño de vóxel distinto del objetivo (desfasada 0,5–5 s según el dataset)? | Reconstruir siempre con la última vista · reconstruir solo al reposar · reconstruir solo si cambia el corte | Última vista con reproyección (los cubos en mundo siguen siendo válidos); revisar tras R2b | R2 (política en movimiento) |
| P8 | ¿Se activa el worker de construcción por defecto en cuanto cumpla la latencia de frames, aunque la escena llegue con desfase? | Apagado hasta R2b · encendido tras R2-T3 | Encendido tras R2-T3 si los criterios de latencia pasan y no aumentan huecos | R2 (valor por defecto) |

## Conversor web paralelo (30-10-2026)

> Decisión 3 (presupuesto de spools de 2 o 4 GiB) resuelta con la medición de M3b: 2 GiB por defecto ([registro](../records/benchmarks/M3B_BELVEDERE_2026-10-30.md), [ADR 0017](../adr/0017-zonas-opfs-paginadas-para-spools-web.md)).

| Id | Pregunta | Opciones | Recomendación | Bloquea |
| --- | --- | --- | --- | --- |
| W1 | Fusionar `feat/web-converter-stage4` en `master` al cerrar M3b | Fusionar · revisar antes | Fusionar tras `check.sh` y las puertas de las Tareas 6 y 7 | Cierre de la etapa 4 |
| W2 | Majadas con un worker: el modelo da un pico OPFS ≈ todo el pico vivo (17,5–20 GiB), por encima de los 10 GiB de Firefox sin `persist()` ([registro](../records/benchmarks/HYBRID_POLICY_2026-10-30.md)) | Exigir `persist()` en Firefox para Majadas · limitar el tamaño admitido sin `persist()` | Exigir `persist()` con mensaje claro (ya es el comportamiento de R4) | M6 |
