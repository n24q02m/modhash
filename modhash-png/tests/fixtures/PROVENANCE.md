# Fixture provenance — modhash-png

Every `.png` in this directory was generated offline by
`gen_fixtures.py` (same directory, Python 3.13 stdlib only: `zlib` for
compression + CRC-32, `struct`, `hashlib`). Regenerate with:

    python gen_fixtures.py

The generator is a byte-level writer — filters are applied per row,
Adam7 passes are emitted in specification order, and every expected
pixel is declared in the generator's source matrices, so the test suite
checks the decoder against **file bytes + declared pixels**, never
against the decoder's own output.

`sha256` below is the first 16 hex digits of the file's SHA-256 (so a
corrupted fixture is caught before it reaches a test).

| file | bytes | sha256:16 | construction |
|---|---|---|---|
| pal8_trns_4x4.png | 134 | 33a802de39d2e4ac | ct3 d8, 8-entry PLTE, tRNS alpha = 255,128,0 for entries 0,1,2 then opaque |
| pal4_9x2.png | 128 | 4b1bc3e1b0653179 | ct3 d4, 13-entry PLTE, no tRNS → Rgb8 |
| gray4_5x3.png | 77 | f9c0c0d08fa899d1 | ct0 d4, odd nibble width (5 px/row) |
| gray16_3x2.png | 79 | 013f92e23e78e670 | ct0 d16, big-endian samples |
| rgb8_filters_5x5.png | 106 | ec0716774bf9896a | ct2 d8, rows coded with filters 0,1,2,3,4 in order |
| rgb16_3x2.png | 106 | bee24a3bc3a13857 | ct2 d16 |
| ga8_4x4.png | 102 | 59f5b86354ebddd5 | ct4 d8, decoder expands to Rgba8 |
| ga16_2x2.png | 83 | d3302dbd6ecb156b | ct4 d16 → Rgba16 |
| rgba8_4x4.png | 119 | 6970574e543a18fb | ct6 d8 |
| rgba16_2x2.png | 100 | bb111af6c7e9450f | ct6 d16 |
| pal8_trns_4x4.png | 136 | 692e811b2c5724a6 | ct3 d8, 8-entry PLTE, tRNS alpha = 255,128,0 for entries 0,1,2 then opaque |
| pal4_9x2.png | 123 | 462c7bb4515b77f8 | ct3 d4, 13-entry PLTE, no tRNS → Rgb8 |
| gray8_trns_3x2.png | 87 | fe9f7e8d9db4ddb2 | ct0 d8 + tRNS grey=10 → Rgba8 |
| rgb8_trns_3x2.png | 100 | b4e15488ce4719a3 | ct2 d8 + tRNS rgb=(9,9,9) → Rgba8 |
| paeth_tie_2x2.png | 71 | 01a8dc76219f61fa | ct0 d8 all-Paeth; row1 col1 is a pb==pc<pa tie → predictor = b=20 |
| adam7_13x11_rgb.png | 412 | 0af224a61fd15d1b | ct2 d8 Adam7; per-pass filter mix (p·7+r)%5; pixels = formula in test |
| adam7_5x5_gray8.png | 97 | 2d607d578e0d7b68 | ct0 d8 Adam7; 5×5 partially populates every pass |
| adam7_2x2_gray8.png | 72 | 48e760be01674bb2 | ct0 d8 Adam7 2×2: passes 1-4 empty, only 0/5/6 carry rows |

Coverage by colour type: ct0 — gray8/1/4/16, gray8_trns, paeth_tie,
adam7_5x5_gray8; ct2 — rgb8_filters, rgb16, rgb8_trns, adam7_13x11_rgb;
ct3 — pal8_trns, pal4; ct4 — ga8, ga16; ct6 — rgba8, rgba16.
Adam7: `adam7_13x11_rgb.png` (all passes populated) and
`adam7_5x5_gray8.png` and `adam7_2x2_gray8.png` (empty passes exercised).
