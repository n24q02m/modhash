# Fixture provenance — `modhash-h264` conformance vectors

All fixtures in this directory were generated **locally and offline** on
2026-10-03 with ffmpeg 9.0.1 (`-full_build-www.gyan.dev`, libx264 inside)
running on the development machine. They are **not** the ITU-T/JVT
conformance bitstreams — those archives were not reachable during the
phase (see the crate report); every stream here is Baseline profile
(`profile_idc` 66, constraint_set flags `0xC0`, level 1.0), Annex-B
framed, CAVLC, I/P slices, 8-bit 4:2:0 — exactly the feature set this
crate decodes.

## Generation recipe

Source frames are raw YUV420p test patterns (flat field, horizontal
gradient, gaussian noise-ish gradient) written by a local generator,
then encoded and reference-decoded as:

```sh
ffmpeg -f rawvideo -pix_fmt yuv420p -s WxH -r 25 -i src.yuv \
    -c:v libx264 -profile:v baseline -level 1.0 [-x264-params no-deblock=1] \
    [-slices N] [-bf 0] [-g N] [-tune zerolatency] out.h264
ffmpeg -i out.h264 -pix_fmt yuv420p -f rawvideo out.yuv   # reference pixels
```

`-bf 0` keeps the stream inside the I/P-slice baseline subset; `no-deblock=1`
variants (`*_nd`) pin the pre-loop-filter reconstruction path, while the
plain variants exercise the in-loop deblocking filter end-to-end.

## Inventory

| file | size WxH | frames | exercise |
|---|---|---|---|
| flat16.h264 / .yuv | 16x16 | 1 | flat intra, deblock |
| flat16b.h264 / .yuv | 16x16 | 1 | flat intra variant |
| flat16i4.h264 / .yuv | 16x16 | 1 | flat, I_4x4 forced |
| gd16.h264 / .yuv | 16x16 | 1 | gradient intra, deblock |
| gd16_nd.h264 / .yuv | 16x16 | 1 | same, no deblock |
| grad16.h264 / .yuv | 16x16 | 1 | gradient intra, deblock |
| grad16_nd.h264 / .yuv | 16x16 | 1 | same, no deblock |
| hgrad32.h264 / .yuv | 32x32 | 1 | horizontal gradient intra |
| t1_32x32_ip.h264 / .yuv | 32x32 | 5 | I+P slices, single slice/frame |
| t1_nd.h264 / .yuv | 32x32 | 5 | same, no deblock |
| t2_48x48_ip.h264 / .yuv | 48x48 | 5 | I+P, 3x3 MB grid, I_16x16 + chroma DC |
| t3_16x16.h264 / .yuv | 16x16 | 4 | minimal multi-frame |
| t3_nd.h264 / .yuv | 16x16 | 4 | same, no deblock |
| t4_80x64_sliced.h264 / .yuv | 80x64 | 6 | 4 slices/frame, P_Skip at slice edges, nref>1 |
| t5_64x64_i16.h264 / .yuv | 64x64 | 10 | Intra-16x16 heavy |

SHA-256 (first 16 hex) per `.h264` fixture:

```
flat16.h264          40674da08f5a595d
flat16b.h264         87524ed91cdc7f8a
flat16i4.h264        f27094e00c0d0c74
gd16.h264            c2177dddb9eddb6f
gd16_nd.h264         e04e27bf72606786
grad16.h264          3d1f9f211caaf310
grad16_nd.h264       44948fd3f2fb6318
hgrad32.h264         3d67511d7a3962ff
t1_32x32_ip.h264     937d317feb31f8cc
t1_nd.h264           153cf7b28b2eddd1
t2_48x48_ip.h264     061dd13c6e3387d1
t3_16x16.h264        c3604a1a8b9e0fd7
t3_nd.h264           87e4971da1b3bb31
t4_80x64_sliced.h264 d04f435fb2740215
t5_64x64_i16.h264    e1ad5d749239eb74
```

## License note

Generated test data: no third-party copyrighted material. The reference
decoder is ffmpeg (LGPL/GPL build); we use it only to *produce*
expectations — the crate itself contains no ffmpeg code. ITU-T
conformance vectors remain an open gap (unreachable); if fetched later
they belong here with their own provenance lines.
