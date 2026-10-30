# Guía para agentes

Proyecto desarrollado sobre todo por agentes LLM. Este fichero solo recoge lo que no se descubre
leyendo el código; las reglas completas viven en [CONTRIBUTING](CONTRIBUTING.md#convenciones).

## Comprobar

- `scripts/check.sh` es el único punto de entrada de checks (tests, clippy nativo y wasm32,
  paridad wasm32/nativo, docs, enlaces, índices). Usa `--offline`; tarda minutos: ejecútalo en
  segundo plano. `FUZZ_CHECK=0` y `CHECK_WASM_BINDINGS=0` omiten pasos que necesitan toolchains extra.
- No cubre GPU, probes de navegador ni presentación visual: un cambio en `examples/web-host/`
  se verifica además con Playwright (flujos, consola, escritorio y móvil).
- Toolchain nightly fijado en `rust-toolchain.toml`; subir la fecha es decisión del propietario.

## Orientarse

| Pregunta | Dónde |
| --- | --- |
| Qué se está haciendo ahora | [README, «Prioridades pendientes»](README.md#prioridades-pendientes) |
| Qué debe hacer el producto | [docs/requirements.md](docs/requirements.md) (IDs `RF-`, `RQ-`, `CON-`, `OQ-`) |
| Por qué se decidió algo | [docs/adr/](docs/adr/README.md); `grep -n '^Estado:' docs/adr/*.md` |
| Plan en curso y su estado | [docs/plans/](docs/README.md#planes-y-specs); `grep -n '^Estado:' docs/plans/*.md` |
| Componentes, features y límites | [docs/reference/inventory.md](docs/reference/inventory.md) |
| Dónde guardar un documento nuevo | [docs/README.md, «Dónde va cada cosa»](docs/README.md#dónde-va-cada-cosa) |

Los documentos largos se leen por rangos: primero sus encabezados (`grep -n '^#' FICHERO`).

## Reglas que más cuesta romper

- **No reabrir decisiones**: un ADR aceptado o una resolución del propietario
  ([requisitos](docs/requirements.md#resoluciones-del-30-10-2026)) solo cambia con un ADR nuevo y
  con el visto bueno del propietario. Las preguntas nuevas van a
  [preguntas abiertas](docs/requirements.md#preguntas-abiertas).
- **Registros inmutables**: `docs/records/` no se edita salvo enlaces rotos; la evidencia nueva va
  en un registro nuevo fechado.
- **Cifras de benchmark** solo valen para su revisión, perfil (`perf` para medir, `release` para
  producción), navegador, dataset y equipo; no se trasladan.
- **Versiones de formato**: `FormatVersion::V<n>` solo dentro de `viewer_format`
  (lo vigila `scripts/check_version_literals.sh`).
- **Idioma y fechas**: documentación en español (salvo `examples/web-host/README.md`, en inglés);
  código, comentarios y commits en inglés; fechas `dd-mm-aaaa`.
