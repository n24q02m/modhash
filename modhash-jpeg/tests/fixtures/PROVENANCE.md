# Fixture provenance — modhash-jpeg

All `*.jpg` fixtures were generated offline and every `*.raw` is the
reference decoder's pixel output (`PIL.Image.open(...).tobytes()`),
committed so the conformance tests compare bytes, not behaviors.

| fixture | generator | encoder settings | reference decode |
|---|---|---|---|
| base_444.jpg/.raw | gen.py / Pillow 12.3.0 | quality=90, subsampling=0 (4:4:4) | Pillow 12.3.0 (libjpeg-turbo, ISLOW + fancy upsample) |
| base_422.jpg/.raw | gen.py / Pillow 12.3.0 | quality=90, subsampling=1 (4:2:2) | same |
| base_420_rst.jpg/.raw | gen.py / Pillow 12.3.0 | quality=88, subsampling=2 (4:2:0), restart_marker_blocks=2 | same |
| base_gray.jpg/.raw | gen.py / Pillow 12.3.0 | quality=92, grayscale | same |
| base_422_odd.jpg/.raw | gen2.py / Pillow 12.3.0 | 41×23, quality=90, 4:2:2 (odd width) | same |
| base_420_odd.jpg/.raw | gen2.py / Pillow 12.3.0 | 33×17, quality=90, 4:2:0 (odd w & h) | same |
| prog_444_rst.jpg/.raw | gen2.py / Pillow 12.3.0 | quality=88, progressive, restart=2 | same |
| prog_420.jpg/.raw | gen.py / Pillow 12.3.0 | quality=90, progressive, 4:2:0 | same |
| prog_444.jpg/.raw | gen.py / Pillow 12.3.0 | quality=90, progressive, 4:4:4 | same |
| prog_gray.jpg/.raw | gen.py / Pillow 12.3.0 | quality=92, progressive, grayscale | same |
| base_440.jpg/.raw | ffmpeg 9.0.1 `-pix_fmt yuvj440p -q:v 4` from a Pillow PNG | 4:4:0 | Pillow 12.3.0 |

Toolchain: Python 3.x + Pillow 12.3.0, ffmpeg 9.0.1-full_build (gyan.dev),
Windows 11. Regenerate with `python gen.py && python gen2.py` plus the
ffmpeg line above (see `gen2.py` tail comment), then `dump.py` to write
the `.raw` companions.

Tolerance: **zero** — tests assert byte-for-byte equality because the
decoder replicates libjpeg's `islow` integer IDCT, `fancy` (triangle)
chroma upsampling, and fixed-point YCbCr→RGB exactly. The only declared
divergence is i64 intermediates where libjpeg's `int` accumulators would
wrap on hostile magnitudes (a clamp regime the fixtures never reach).
