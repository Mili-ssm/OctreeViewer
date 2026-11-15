# Octree Viewer

A LiDAR point-cloud viewer in Rust, WebAssembly and WebGPU, with a LAS/LAZ converter that builds a persistent octree hierarchy the viewer loads block by block.

**Live demo (experimental):** <https://mili-ssm.github.io/OctreeViewer/>

## Status

- Experimental and pre-1.0, developed by a single maintainer with LLM coding agents. Nothing is published to npm; APIs and file formats may change.
- Targets desktop Chrome/Edge with WebGPU. Firefox is best effort; Safari and mobile are out of scope.
- The [public demo](https://mili-ssm.github.io/OctreeViewer/) is experimental: it may lag behind `main` and has known bugs (see the [issues](https://github.com/Mili-ssm/OctreeViewer/issues)).

## What it does

- Converts LAS, LAZ, COPC (read as sequential LAS/LAZ) and local EPT to a persistent `.oct` (geometry) and `.octattrs` (attributes) pair with the default `v7` format.
- Loads only the blocks a view needs, from local files or over HTTP Range.
- Keeps per-node voxel cells resident on the GPU with level of detail (LOD).
- Target scale: hundreds of GB of LAZ with bounded memory. **Proven so far:** datasets up to about 6 GB of LAZ (Majadas), measured against budgets for frame time, time to first frame and memory ([requirements](docs/requirements.md), [records](docs/records/benchmarks/)). **Not proven:** the full target scale and parity with Potree.

## Quick start

Requirements: [rustup](https://rustup.rs) (the nightly toolchain pinned in `rust-toolchain.toml` installs itself), the `wasm-bindgen` CLI at the version in `Cargo.lock` (`cargo install wasm-bindgen-cli --version 0.2.129`), Python 3, and Chrome or Edge.

Convert a point cloud (writes `output.oct` and `output.octattrs`):

```sh
cargo run --release -p viewer_convert -- input.laz output.oct
```

Run the demo locally. The script builds the wasm bindings and serves the repository; open the printed URL (`http://127.0.0.1:8000/examples/web-host/`). A bundled sample opens by itself; you can then open your own `.oct` (with its `.octattrs`) or convert a LAS/LAZ file in the page:

```sh
scripts/serve-demo.sh
```

## Repository layout

| Path | Contents |
| --- | --- |
| `crates/viewer_core` | Geometry, hierarchy, LOD, cache and shared contracts |
| `crates/viewer_format` | `.oct`/`.octattrs` formats: quantisation, codecs, selective reads |
| `crates/viewer_convert_core` | I/O-free conversion core (also compiles to wasm32) |
| `crates/viewer_convert` | Native converter CLI (`viewer_convert`) |
| `crates/viewer_render_wgpu` | Camera, voxelisation, GPU buffers and presentation |
| `crates/viewer_wasm` | WebAssembly bindings: worker, LOD loading, web conversion |
| `examples/web-host` | Web demo and embeddable API |
| `tools/` | Parity and probe tools (`convert_parity`, `format_parity`, `laz_chunk_probe`) |
| `fuzz/` | Fuzz targets |
| `scripts/` | Local checks and demo scripts |
| `data/` | Small sample files and dataset provenance |
| `vendor/lz4_flex` | Vendored fork of `lz4_flex` |
| `docs/` | Requirements, ADRs, plans, format specs and benchmark records |

## Documentation

The documentation is in Spanish, except this README, `SECURITY.md` and the web host README.

- [docs/README.md](docs/README.md): documentation index
- [docs/estado.md](docs/estado.md): current status and priorities
- [docs/requirements.md](docs/requirements.md): requirements and open questions
- [docs/adr/](docs/adr/README.md): architecture decisions
- [docs/plans/](docs/README.md#planes-y-specs): plans
- [docs/reference/format/](docs/reference/format/): file format specifications
- [examples/web-host/README.md](examples/web-host/README.md): web host and API (English)

## Development

`scripts/check.sh` runs every check (tests, clippy for native and wasm32, docs, links); it takes minutes. There is no CI: checks run locally. See [CONTRIBUTING.md](CONTRIBUTING.md) for conventions and the issue and PR workflow; pending work lives in [GitHub issues](https://github.com/Mili-ssm/OctreeViewer/issues).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. The vendored `vendor/lz4_flex` fork keeps its MIT license. Provenance of sample data: [data/samples/README.md](data/samples/README.md) and [data/datasets/README.md](data/datasets/README.md).
