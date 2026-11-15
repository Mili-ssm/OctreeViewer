# Publicación del repositorio en GitHub

Estado: activo · 15-11-2026

**Objetivo:** hacer público `Mili-ssm/OctreeViewer` como escaparate experimental (pre-1.0) con la
documentación, la portada y la demo en orden, y un flujo de desarrollo sin fricción para un solo
mantenedor. El desarrollo de producto sigue después, en otra petición.

**Decisiones del propietario (15-11-2026):** fusionar las PR #1 y #29 en `main`; demo a criterio del
agente (se publica como demo experimental, [ADR 0045](../adr/0045-demo-experimental-publica-antes-de-la-1-0.md));
el email del historial se mantiene; protección ligera de `main`; las imágenes de SensOFOREST salen del
árbol actual sin reescribir el historial; el README principal pasa al inglés.

**Evidencia de partida:** auditorías del 15-11-2026 (seguridad: sin secretos; documentación: el README
es un registro interno; demo: abre la muestra, pero una conversión V7 pequeña no se dibuja, #30).

## Tareas

- [x] Fusionar #1 y #29 en `main` con merge commit (conserva los SHA citados). Comprobación: `main` en `2c7364d`.
- [x] Arreglar #30 (celdas de una conversión V7 pequeña). Comprobación: test que falla antes y pasa después; la demo la dibuja.
- [x] Quitar del árbol los renders de SensOFOREST (sin licencia) en `main` y en las ramas de experimentos, y
  atribuir los renders del Stanford Bunny. Comprobación: `git ls-files | grep -ci majadas.*png` = 0; enlaces en verde.
- [x] Portada en inglés (`README.md`, ~70 líneas) y estado y prioridades en `docs/estado.md`; excepción de
  idioma en `AGENTS.md` y `CONTRIBUTING.md`; `SECURITY.md`. Comprobación: checks de docs en verde.
- [ ] Demo: inicio rápido y requisitos en `examples/web-host/README.md`, sondas a `PROBES.md`, la muestra se abre
  sola, script de servidor local y publicación en GitHub Pages desde una rama construida en local (ADR 0045).
  Comprobación: Playwright sobre la URL pública, escritorio y móvil.
- [x] Ajustes de GitHub: descripción, temas, web; solo merge commit; borrar ramas al fusionar; wiki y proyectos
  desactivados; regla ligera en `main`; avisos de seguridad y de secretos; borrar las ejecuciones de Actions
  antiguas y las ramas ya fusionadas. Comprobación: `gh api repos/Mili-ssm/OctreeViewer`.
- [x] Hacer público el repositorio y comprobarlo sin sesión. Comprobación: la portada y la demo cargan sin login.
- [ ] Arreglar #35 (una conversión cuya z cruza 0 se ve como un punto) y volver a publicar la demo con
  `scripts/publish-demo.sh --push`. Comprobación: Playwright sobre la URL pública con la conversión de la muestra.
