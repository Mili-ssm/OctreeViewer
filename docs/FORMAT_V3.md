# Octree Viewer Block Format, version 3

Version 3 adds a positive finite `visual_scale` used only by the renderer. All other byte encodings
remain as specified by [version 2](FORMAT_V2.md), including replacement and additive hierarchy
semantics. Stored positions, dataset origin, bounds, CRS, quantization error and geometric error remain
in source units.

## Version markers

The 24-byte header has the same layout as earlier versions, with `version = 3`. The index starts with
`OCTVIDX3`; the 40-byte footer uses `OCTVEND3`. Header, index and footer checksums and all offsets
retain their earlier definitions.

## Index change

Immediately after the hierarchy semantics byte and before the attribute count, v3 stores one `f64`
`visual_scale`. It must be finite and greater than zero. The renderer multiplies local coordinates,
point glyph dimensions, camera bounds and geometric error by this factor. Spatial selection converts
the view frustum back to source coordinates. The value does not change the coordinates or units stored
in blocks or metadata.

New LAS/LAZ and EPT conversions choose the factor so the largest root-bounds extent maps to 100
rendering units. A degenerate zero-extent dataset uses `1.0`. If the factor cannot be represented by
the current renderer, conversion fails rather than clamping it. This normalization is for presentation;
measurements and CRS continue to use the original coordinates.

## Compatibility

The current reader accepts v1, v2 and v3. Older versions use an effective `visual_scale` of `1.0`.
Versions 1 and 2 retain their existing byte layout. A v1 or v2 reader must reject v3 because it cannot
interpret the scale field.
