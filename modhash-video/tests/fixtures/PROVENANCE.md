# Fixture provenance — `modhash-video`

All fixtures were generated **locally and offline** on 2026-10-03 with
ffmpeg 9.0.1 (`-full_build-www.gyan.dev`, libx264 inside) on the
development machine. Video sources are ffmpeg's deterministic lavfi
generators (`testsrc2`, `mandelbrot`, `rgbtestsrc`, `gradients`,
`color`) — no third-party footage, no redistributed ITU/JVT conformance
vectors (those archives are not license-clean; same gap the
`modhash-h264` lane documented).

## Generation recipe

```sh
# content A (testsrc2), 4 s @ 8 fps, baseline + CAVLC, no B-frames
ffmpeg -f lavfi -i "testsrc2=size=64x48:rate=8:duration=4" \
    -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -g 16 -crf 23 a_64x48.mp4
# same content, scaled 2x nearest then re-encoded
ffmpeg -f lavfi -i "testsrc2=size=64x48:rate=8:duration=4" \
    -vf scale=128:96:flags=neighbor \
    -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -g 16 -crf 23 a_128x96.mp4
# same content, lower bitrate
ffmpeg -f lavfi -i "testsrc2=size=64x48:rate=8:duration=4" \
    -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -g 16 -crf 40 a_crf40.mp4
# same elementary stream remuxed mp4 -> mov (same ISO-BMFF box tree)
ffmpeg -i a_64x48.mp4 -c copy a_64x48.mov

# other contents
ffmpeg -f lavfi -i "mandelbrot=size=64x48:rate=8"   -t 4 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -g 16 -crf 23 b_64x48.mp4
ffmpeg -f lavfi -i "rgbtestsrc=size=64x48:rate=8"   -t 4 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -g 16 -crf 23 c_64x48.mp4
ffmpeg -f lavfi -i "gradients=size=64x48:speed=0.05:rate=8" -t 4 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -g 16 -crf 23 d_64x48.mp4

# tiny / boundary fixtures
ffmpeg -f lavfi -i "color=red:size=16x16:rate=8"  -frames:v 4 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -g 16 -crf 30 tiny_16x16.mp4
ffmpeg -f lavfi -i "color=blue:size=32x24:rate=8" -frames:v 3 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -crf 30 short_32x24.mp4
ffmpeg -f lavfi -i "testsrc2=size=32x24:rate=8"   -frames:v 2 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -g 2 -crf 30 two_32x24.mp4

# negative fixtures
ffmpeg -f lavfi -i "mandelbrot=size=64x48:rate=8" -t 4 -c:v mpeg4 -tag:v mp4v e_mp4v.mp4
ffmpeg -f lavfi -i "sine=frequency=440:duration=1" -c:a aac audioonly.mp4
ffmpeg -f lavfi -i "testsrc2=size=64x48:rate=8" -t 1 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -frag_duration 100000 a_frag.mp4
ffmpeg -f lavfi -i "testsrc2=size=64x48:rate=8" -frames:v 8 -c:v libx264 -profile:v high -pix_fmt yuv420p -crf 23 high_64x48.mp4
ffmpeg -f lavfi -i "testsrc2=size=32x24:rate=8" -t 1 -c:v libx264 -profile:v baseline -pix_fmt yuv420p -bf 0 -crf 30 -f h264 annexb_32x24.h264
```

## SHA-256 (full)

```
be857130d02561ed0c9b7776821cf798a4dcbdb392afd31f6a4767270b16325d  a_128x96.mp4
5340a9bb58a97b3238ac1595f996c8db306ec008b5b279c4befee5e603bb513c  a_64x48.mov
fe641153a25f55b84a67a5f148fdea731d035a90a1d1a9b460408cf892b3589a  a_64x48.mp4
44e60a8929b8a8b74da3591c498c42a0680402632d4c0388e8917b102ef552fa  a_crf40.mp4
85f2a5fba4b27a031cdf8a7a75291bd3900b9eb6339eb59322b9e943b0fc4822  a_frag.mp4
0c96019613ac8ac9e1b723ac04e2a159cb6d6ff5679be4d4afb19e9406b67d75  annexb_32x24.h264
b450a2869107beb187eac60f546108866d7f059681f8b899f4a25b76ca71adb0  audioonly.mp4
7721c9352f71fdd3073f6e1b03da88906a8477d7dc29ba672ca75b14df20382e  b_64x48.mp4
b240265e2b4cf8ab212a3ce6611294c7e04ef716a8bb2d93c0e70c745c30d599  c_64x48.mp4
591a8a8709506c1a53b6d24f2ebd4e50d73625d58f00922545aad09f11f5d988  d_64x48.mp4
d7325b5341add0b1607371040a480c1115302ec370602a3b7fc52252c06e7eb2  e_mp4v.mp4
405ac3df8af9e0c58c498900803bf19dcde9e50c85ed31689a99f4872bc7b1f0  high_64x48.mp4
4909b7404c6f5c56d358cccf6d589fead832c5c7577e97e328b040006dbd4521  short_32x24.mp4
eb750dd601c57bd372f6e0558c4a9facc05bef1d5d49276dad4dd984e3cc5703  tiny_16x16.mp4
d0ab1fc8eacf16a06d354f57f07fc75c008dcb23bee72f9b3b250523b48dc167  two_32x24.mp4
```

The `ffmpeg`/`ffprobe` binaries are encoder/reference tools only; the
crate contains no ffmpeg code.
