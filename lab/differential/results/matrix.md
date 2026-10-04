# Differential matrix — modhash (Rust, zero-dep) vs hashkit 0.5.0 (Python)

Statuses: MATCH byte-equal on that axis · CLOSE small drift ·
MISMATCH real divergence found · BY-DESIGN divergent by spec ·
PY-N/A python cannot ingest · RUST-N/A rust refuses (named) · SKIP.

## Coverage (ingest)

| file | rust modality | rust | python | status |
|---|---|---|---|---|
| audio/l3_48k.mp3 | audio | refused:audio: unsupported: audio sample rate ≠ 44100 Hz | ok | RUST-N/A |
| audio/l3_mono.mp3 | audio | ok | ok | BOTH |
| audio/l3_short.mp3 | audio | ok | ok | BOTH |
| audio/stereo_441.wav | audio | ok | ok | BOTH |
| audio/tone.flac | audio | ok | ok | BOTH |
| audio/tone.wav | audio | ok | ok | BOTH |
| audio/tone22.wav | audio | refused:audio: unsupported: audio sample rate ≠ 44100 Hz | ok | RUST-N/A |
| audio/wav24.wav | audio | ok | ok | BOTH |
| audio/wav8_22k.wav | audio | refused:audio: unsupported: audio sample rate ≠ 44100 Hz | ok | RUST-N/A |
| bin/blob.bin | binary | ok | ok | BOTH |
| bin/f_a.bin | binary | ok | ok | BOTH |
| bin/f_copy.bin | binary | ok | ok | BOTH |
| bin/f_mod.bin | binary | ok | ok | BOTH |
| bin/f_other.bin | binary | ok | ok | BOTH |
| bin/f_tail.bin | binary | ok | ok | BOTH |
| bin/hello.docx | binary | ok | ok | BOTH |
| bin/minimal.zip | binary | ok | ok | BOTH |
| bin/pack_deflated.zip | binary | ok | ok | BOTH |
| bin/pack_stored.zip | binary | ok | ok | BOTH |
| img/adam7_13x11_rgb.png | image | ok | ok | BOTH |
| img/base_422.jpg | image | ok | ok | BOTH |
| img/base_444.jpg | image | ok | ok | BOTH |
| img/base_444.png | image | ok | ok | BOTH |
| img/base_gray.jpg | image | ok | ok | BOTH |
| img/big_1200x800.png | image | ok | ok | BOTH |
| img/flat.gif | binary | ok | error:error: OpenCV(5.0.0) D:\a\opencv-python\opencv-python\ | PY-N/A |
| img/gray16_3x2.png | image | ok | ok | BOTH |
| img/p24_bottom_up.bmp | image | ok | ok | BOTH |
| img/p24_top_down.bmp | image | ok | ok | BOTH |
| img/p32_v5_rgb.bmp | image | ok | ok | BOTH |
| img/phash_rgb8_48x40.png | image | ok | ok | BOTH |
| img/prog_420.jpg | image | ok | ok | BOTH |
| img/prog_444.jpg | image | ok | ok | BOTH |
| img/prog_gray.jpg | image | ok | ok | BOTH |
| img/rgb16_3x2.png | image | ok | ok | BOTH |
| img/rgba16_2x2.png | image | ok | ok | BOTH |
| img/rle8.bmp | image | ok | ok | BOTH |
| real/audio_a.wav | audio | refused:audio: unsupported: audio sample rate ≠ 44100 Hz | ok | RUST-N/A |
| real/audio_a_quiet.wav | audio | refused:audio: unsupported: audio sample rate ≠ 44100 Hz | ok | RUST-N/A |
| real/city_1.jpg | image | ok | ok | BOTH |
| real/img_a.png | image | ok | ok | BOTH |
| real/img_b.jpg | image | ok | ok | BOTH |
| real/img_other.png | image | ok | ok | BOTH |
| real/portrait_1.jpg | image | ok | ok | BOTH |
| real/text_a.txt | text | ok | ok | BOTH |
| real/text_a_edit.txt | text | ok | ok | BOTH |
| real/text_b.txt | text | ok | ok | BOTH |
| real/v_clip.avi | binary | ok | ok | BOTH |
| text/doc_a.txt | text | ok | ok | BOTH |
| text/doc_a_edit.txt | text | ok | ok | BOTH |
| text/doc_b.txt | text | ok | ok | BOTH |
| text/encrypted.pdf | text | refused:text: page 0: object 6 0: unsupported: encrypted doc | ok | RUST-N/A |
| text/macroman.pdf | text | ok | ok | BOTH |
| text/objstm.pdf | text | ok | ok | BOTH |
| text/prose.txt | text | ok | ok | BOTH |
| text/punct_tags.txt | text | ok | ok | BOTH |
| text/short_two.txt | text | ok | ok | BOTH |
| text/standard_default.pdf | text | ok | ok | BOTH |
| text/text_page.pdf | text | ok | ok | BOTH |
| text/tounicode_bfrange.pdf | text | ok | ok | BOTH |
| text/vi_nfc.txt | text | ok | ok | BOTH |
| text/vi_nfd.txt | text | ok | ok | BOTH |
| video/a_128x96.mp4 | video | ok | ok | BOTH |
| video/a_64x48.mov | video | ok | ok | BOTH |
| video/a_64x48.mp4 | video | ok | ok | BOTH |
| video/a_crf40.mp4 | video | ok | ok | BOTH |
| video/a_frag.mp4 | video | refused:video: unsupported: fragmented mp4 (moof) | ok | RUST-N/A |
| video/audioonly.mp4 | video | refused:video: bad value: mp4 has no video track | no-dump | NEITHER |
| video/b_64x48.mp4 | video | ok | no-dump | PY-N/A |
| video/e_mp4v.mp4 | video | refused:video: unsupported: video codec MPEG-4 part 2 | no-dump | NEITHER |
| video/high_64x48.mp4 | video | refused:video: unsupported: h264 profile: High | no-dump | NEITHER |
| video/minimal.mp4 | video | refused:video: truncated: ue(v) prefix | no-dump | NEITHER |

## Tier-1 (SHA-256 on canonical payload)

| file | status |
|---|---|
| audio/l3_48k.mp3 | BY-DESIGN(canonical differs) |
| audio/l3_mono.mp3 | BY-DESIGN(canonical differs) |
| audio/l3_short.mp3 | BY-DESIGN(canonical differs) |
| audio/stereo_441.wav | BY-DESIGN(canonical differs) |
| audio/tone.flac | BY-DESIGN(canonical differs) |
| audio/tone.wav | BY-DESIGN(canonical differs) |
| audio/tone22.wav | BY-DESIGN(canonical differs) |
| audio/wav24.wav | BY-DESIGN(canonical differs) |
| audio/wav8_22k.wav | BY-DESIGN(canonical differs) |
| bin/blob.bin | MATCH |
| bin/f_a.bin | MATCH |
| bin/f_copy.bin | MATCH |
| bin/f_mod.bin | MATCH |
| bin/f_other.bin | MATCH |
| bin/f_tail.bin | MATCH |
| bin/hello.docx | MATCH |
| bin/minimal.zip | MATCH |
| bin/pack_deflated.zip | MATCH |
| bin/pack_stored.zip | MATCH |
| img/adam7_13x11_rgb.png | MATCH |
| img/base_422.jpg | MATCH |
| img/base_444.jpg | MATCH |
| img/base_444.png | MATCH |
| img/base_gray.jpg | MATCH |
| img/big_1200x800.png | BY-DESIGN(py-resize>1024) |
| img/flat.gif | RUST-N/A |
| img/gray16_3x2.png | MATCH |
| img/p24_bottom_up.bmp | MATCH |
| img/p24_top_down.bmp | MATCH |
| img/p32_v5_rgb.bmp | MATCH |
| img/phash_rgb8_48x40.png | MATCH |
| img/prog_420.jpg | MATCH |
| img/prog_444.jpg | MATCH |
| img/prog_gray.jpg | MATCH |
| img/rgb16_3x2.png | MATCH |
| img/rgba16_2x2.png | MATCH |
| img/rle8.bmp | MATCH |
| real/audio_a.wav | BY-DESIGN(canonical differs) |
| real/audio_a_quiet.wav | BY-DESIGN(canonical differs) |
| real/city_1.jpg | BY-DESIGN(py-resize>1024) |
| real/img_a.png | MATCH |
| real/img_b.jpg | MATCH |
| real/img_other.png | MATCH |
| real/portrait_1.jpg | MATCH |
| real/text_a.txt | MATCH |
| real/text_a_edit.txt | MATCH |
| real/text_b.txt | MATCH |
| real/v_clip.avi | BY-DESIGN(canonical differs) |
| text/doc_a.txt | MATCH |
| text/doc_a_edit.txt | MATCH |
| text/doc_b.txt | MATCH |
| text/encrypted.pdf | BY-DESIGN(pdf: rust=text, py=bytes) |
| text/macroman.pdf | BY-DESIGN(pdf: rust=text, py=bytes) |
| text/objstm.pdf | BY-DESIGN(pdf: rust=text, py=bytes) |
| text/prose.txt | MATCH |
| text/punct_tags.txt | MATCH |
| text/short_two.txt | MATCH |
| text/standard_default.pdf | BY-DESIGN(pdf: rust=text, py=bytes) |
| text/text_page.pdf | BY-DESIGN(pdf: rust=text, py=bytes) |
| text/tounicode_bfrange.pdf | BY-DESIGN(pdf: rust=text, py=bytes) |
| text/vi_nfc.txt | MATCH |
| text/vi_nfd.txt | MATCH |
| video/a_128x96.mp4 | BY-DESIGN(canonical differs) |
| video/a_64x48.mov | BY-DESIGN(canonical differs) |
| video/a_64x48.mp4 | BY-DESIGN(canonical differs) |
| video/a_crf40.mp4 | BY-DESIGN(canonical differs) |
| video/a_frag.mp4 | BY-DESIGN(canonical differs) |
| video/audioonly.mp4 | BY-DESIGN(canonical differs) |
| video/b_64x48.mp4 | BY-DESIGN(canonical differs) |
| video/e_mp4v.mp4 | BY-DESIGN(canonical differs) |
| video/high_64x48.mp4 | BY-DESIGN(canonical differs) |
| video/minimal.mp4 | BY-DESIGN(canonical differs) |

## Decode-exactness (dep decoders vs Rust decoders)

| file | axis results |
|---|---|
| audio/l3_48k.mp3 | pcm_i32=MISMATCH · pcm_mono=MISMATCH · pcm_max_abs_diff=255077394 |
| audio/l3_mono.mp3 | pcm_i32=MISMATCH · pcm_mono=MISMATCH · pcm_max_abs_diff=506545792 |
| audio/l3_short.mp3 | pcm_i32=MISMATCH · pcm_mono=MISMATCH · pcm_max_abs_diff=606390736 |
| audio/stereo_441.wav | pcm_i32=MATCH(shl16) · pcm_mono=MATCH(native-via-shl16) · wav_native=MATCH |
| audio/tone.flac | pcm_i32=MATCH(shl16) · pcm_mono=MATCH(native-via-shl16) |
| audio/tone.wav | pcm_i32=MATCH(shl16) · pcm_mono=MATCH(native-via-shl16) · wav_native=MATCH |
| audio/tone22.wav | pcm_i32=MATCH(shl16) · pcm_mono=MATCH(native-via-shl16) · wav_native=MATCH |
| audio/wav24.wav | pcm_i32=MATCH(shl8) · pcm_mono=MATCH(native-via-shl8) · wav_native=MATCH |
| audio/wav8_22k.wav | pcm_i32=MATCH(shl24) · pcm_mono=MATCH(native-via-shl24) · wav_native=MATCH |
| bin/blob.bin | cdc_chunks={'rust': 6, 'py': 17, 'ratio': 0.353} |
| bin/f_a.bin | cdc_chunks={'rust': 42, 'py': 148, 'ratio': 0.284} |
| bin/f_copy.bin | cdc_chunks={'rust': 42, 'py': 148, 'ratio': 0.284} |
| bin/f_mod.bin | cdc_chunks={'rust': 42, 'py': 148, 'ratio': 0.284} |
| bin/f_other.bin | cdc_chunks={'rust': 42, 'py': 160, 'ratio': 0.263} |
| bin/f_tail.bin | cdc_chunks={'rust': 37, 'py': 131, 'ratio': 0.282} |
| bin/hello.docx | zip_members=MATCH · cdc_chunks={'rust': 1, 'py': 1, 'ratio': 1.0} · docx_text=PY-ONLY(rust binary lane) |
| bin/minimal.zip | zip_members=MATCH · cdc_chunks={'rust': 1, 'py': 1, 'ratio': 1.0} |
| bin/pack_deflated.zip | zip_members=MATCH · cdc_chunks={'rust': 6, 'py': 17, 'ratio': 0.353} |
| bin/pack_stored.zip | zip_members=MATCH · cdc_chunks={'rust': 7, 'py': 17, 'ratio': 0.412} |
| img/adam7_13x11_rgb.png | image_rgb8=MATCH |
| img/base_422.jpg | image_rgb8=MATCH |
| img/base_444.jpg | image_rgb8=MATCH |
| img/base_444.png | image_rgb8=MATCH |
| img/base_gray.jpg | image_rgb8=MATCH |
| img/big_1200x800.png | image_rgb8=MATCH |
| img/flat.gif | image_rgb8=SKIP |
| img/gray16_3x2.png | image_rgb8=MATCH |
| img/p24_bottom_up.bmp | image_rgb8=MATCH |
| img/p24_top_down.bmp | image_rgb8=MATCH |
| img/p32_v5_rgb.bmp | image_rgb8=MATCH |
| img/phash_rgb8_48x40.png | image_rgb8=MATCH |
| img/prog_420.jpg | image_rgb8=MATCH |
| img/prog_444.jpg | image_rgb8=MATCH |
| img/prog_gray.jpg | image_rgb8=MATCH |
| img/rgb16_3x2.png | image_rgb8=MATCH |
| img/rgba16_2x2.png | image_rgb8=MATCH |
| img/rle8.bmp | image_rgb8=MATCH |
| real/audio_a.wav | pcm_i32=MATCH(shl16) · pcm_mono=MATCH(native-via-shl16) · wav_native=MATCH |
| real/audio_a_quiet.wav | pcm_i32=MATCH(shl16) · pcm_mono=MATCH(native-via-shl16) · wav_native=MATCH |
| real/city_1.jpg | image_rgb8=MATCH |
| real/img_a.png | image_rgb8=MATCH |
| real/img_b.jpg | image_rgb8=MATCH |
| real/img_other.png | image_rgb8=MATCH |
| real/portrait_1.jpg | image_rgb8=MATCH |
| real/text_a.txt | SKIP |
| real/text_a_edit.txt | SKIP |
| real/text_b.txt | SKIP |
| real/v_clip.avi | video_y=SKIP |
| text/doc_a.txt | SKIP |
| text/doc_a_edit.txt | SKIP |
| text/doc_b.txt | SKIP |
| text/encrypted.pdf | pdf_text=RUST-N/A |
| text/macroman.pdf | pdf_text_jaccard=1.0 · pdf_text=MATCH |
| text/objstm.pdf | pdf_text_jaccard=1.0 · pdf_text=CLOSE · pdf_text_bytes={'rust': 21, 'py': 21} |
| text/prose.txt | SKIP |
| text/punct_tags.txt | SKIP |
| text/short_two.txt | SKIP |
| text/standard_default.pdf | pdf_text_jaccard=1.0 · pdf_text=MATCH |
| text/text_page.pdf | pdf_text_jaccard=1.0 · pdf_text=MATCH |
| text/tounicode_bfrange.pdf | pdf_text_jaccard=1.0 · pdf_text=MATCH |
| text/vi_nfc.txt | SKIP |
| text/vi_nfd.txt | SKIP |
| video/a_128x96.mp4 | video_y=MATCH · frames=[32 frames] · phash_mirror_video=MATCH |
| video/a_64x48.mov | video_y=MATCH · frames=[32 frames] · phash_mirror_video=MATCH |
| video/a_64x48.mp4 | video_y=MATCH · frames=[32 frames] · phash_mirror_video=MATCH |
| video/a_crf40.mp4 | video_y=MATCH · frames=[32 frames] · phash_mirror_video=MATCH |
| video/a_frag.mp4 | video_y=RUST-N/A |
| video/audioonly.mp4 | video_y=RUST-N/A |
| video/b_64x48.mp4 | video_y=MATCH · frames=[32 frames] · phash_mirror_video=MATCH |
| video/e_mp4v.mp4 | video_y=RUST-N/A |
| video/high_64x48.mp4 | video_y=RUST-N/A |
| video/minimal.mp4 | video_y=RUST-N/A |

## Tier-2 (comparable scalars)

| file | results |
|---|---|
| audio/l3_48k.mp3 | audio_peaks=BY-DESIGN(no shared axis) · py_keys=0 |
| audio/l3_mono.mp3 | audio_peaks=BY-DESIGN(no shared axis) · rust={'frames': 6, 'peaks': 9} · py_keys=0 |
| audio/l3_short.mp3 | audio_peaks=BY-DESIGN(no shared axis) · rust={'frames': 10, 'peaks': 53} · py_keys=0 |
| audio/stereo_441.wav | audio_peaks=BY-DESIGN(no shared axis) · rust={'frames': 20, 'peaks': 44} · py_keys=0 |
| audio/tone.flac | audio_peaks=BY-DESIGN(no shared axis) · rust={'frames': 20, 'peaks': 99} · py_keys=0 |
| audio/tone.wav | audio_peaks=BY-DESIGN(no shared axis) · rust={'frames': 20, 'peaks': 99} · py_keys=0 |
| audio/tone22.wav | audio_peaks=BY-DESIGN(no shared axis) · py_keys=0 |
| audio/wav24.wav | audio_peaks=BY-DESIGN(no shared axis) · rust={'frames': 9, 'peaks': 17} · py_keys=0 |
| audio/wav8_22k.wav | audio_peaks=BY-DESIGN(no shared axis) · py_keys=0 |
| bin/blob.bin | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/f_a.bin | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/f_copy.bin | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/f_mod.bin | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/f_other.bin | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/f_tail.bin | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/hello.docx | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/minimal.zip | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/pack_deflated.zip | chunk_set=BY-DESIGN(cdc params+digest differ) |
| bin/pack_stored.zip | chunk_set=BY-DESIGN(cdc params+digest differ) |
| img/adam7_13x11_rgb.png | phash_cross_hamming=10 |
| img/base_422.jpg | phash_cross_hamming=10 |
| img/base_444.jpg | phash_cross_hamming=4 |
| img/base_444.png | phash_cross_hamming=4 |
| img/base_gray.jpg | phash_cross_hamming=10 |
| img/big_1200x800.png | phash_cross_hamming=4 |
| img/flat.gif | phash_status=RUST-N/A |
| img/gray16_3x2.png | phash_cross_hamming=16 |
| img/p24_bottom_up.bmp | phash_cross_hamming=9 |
| img/p24_top_down.bmp | phash_cross_hamming=9 |
| img/p32_v5_rgb.bmp | phash_cross_hamming=9 |
| img/phash_rgb8_48x40.png | phash_cross_hamming=10 |
| img/prog_420.jpg | phash_cross_hamming=4 |
| img/prog_444.jpg | phash_cross_hamming=2 |
| img/prog_gray.jpg | phash_cross_hamming=10 |
| img/rgb16_3x2.png | phash_cross_hamming=27 |
| img/rgba16_2x2.png | phash_cross_hamming=19 |
| img/rle8.bmp | phash_cross_hamming=14 |
| real/audio_a.wav | audio_peaks=BY-DESIGN(no shared axis) · py_keys=0 |
| real/audio_a_quiet.wav | audio_peaks=BY-DESIGN(no shared axis) · py_keys=0 |
| real/city_1.jpg | phash_cross_hamming=2 |
| real/img_a.png | phash_cross_hamming=0 |
| real/img_b.jpg | phash_cross_hamming=0 |
| real/img_other.png | phash_cross_hamming=2 |
| real/portrait_1.jpg | phash_cross_hamming=0 |
| real/text_a.txt | minhash_head={'rust': ['06b18e4d2858dcf2', '01143239129fa839', '054adcd3e7f82ab8', '059359db990c9ff1'], 'py': ['0046286d38fb1037', '000a8a4e8726f608', '0020700b78951c1a', '007d0a11c3079a20']} · minhas |
| real/text_a_edit.txt | minhash_head={'rust': ['0111293c8fb8e178', '011043f7bb37a882', '02612af1384b6432', '0538f87e16778ad4'], 'py': ['0046286d38fb1037', '000a8a4e8726f608', '0020700b78951c1a', '007d0a11c3079a20']} · minhas |
| real/text_b.txt | minhash_head={'rust': ['001114f688981ea3', '035522d5f7b5b062', '07f0618f832306f6', '060052aac15a08a4'], 'py': ['00834b881a8edbe0', '0018268afa922320', '00d8adace445a722', '00f7ea435cbb0fee']} · minhas |
| real/v_clip.avi | video_chain=BY-DESIGN(fps/grid differ) |
| text/doc_a.txt | minhash_head={'rust': ['00e36ce8dcd483ec', '002b58f9cf81bd61', '0249e371fa78ea9b', '00ab9c3ac934caf9'], 'py': ['000f1e184cff6692', '0033692885bcf56e', '001ac371fe28fe92', '0016c74f9a80168e']} · minhas |
| text/doc_a_edit.txt | minhash_head={'rust': ['00e36ce8dcd483ec', '002b58f9cf81bd61', '0249e371fa78ea9b', '00ab9c3ac934caf9'], 'py': ['000f1e184cff6692', '0033692885bcf56e', '001ac371fe28fe92', '0016c74f9a80168e']} · minhas |
| text/doc_b.txt | minhash_head={'rust': ['01adcecfd7afabe1', '007135ff003b8144', '0381b7d88e68b0ca', '008ee61a3722d075'], 'py': ['002455d9d3bfd222', '000480e9bba33e8e', '000733496f8439bf', '005daf8d836babce']} · minhas |
| text/encrypted.pdf | minhash_status=BY-DESIGN(different perm families) |
| text/macroman.pdf | minhash_status=BY-DESIGN(different perm families) |
| text/objstm.pdf | minhash_status=BY-DESIGN(different perm families) |
| text/prose.txt | minhash_head={'rust': ['04cf73bf9d43ccef', '005908516233326d', '01ae75d4b9c923e1', '01b6c84a8f8aebcc'], 'py': ['006a313da700fb15', '006f2be51888318b', '006ec2ff7c60f2b2', '002b1f52deaacb75']} · minhas |
| text/punct_tags.txt | minhash_head={'rust': ['1fa62f5d462e6244', '1c5868a4b35434ab', '09186853edddea85', '15d4d2ec91e6a72f'], 'py': ['018ac74e5eae4a87', '01f063223fc67d69', '0051d0926d3e7099', '01ec8bd88ad1924d']} · minhas |
| text/short_two.txt | minhash_head={'rust': ['894b774d932fb23a', 'c838b08f4545f3e7', 'a73eb1fc18d5e903', 'a6a0a8d324a3fff0'], 'py': ['1955f6d8fbd8c7b3', '1519d6689a70bde0', '1f5ff9bf22bd507a', '0df669560b55489f']} · minhas |
| text/standard_default.pdf | minhash_status=BY-DESIGN(different perm families) |
| text/text_page.pdf | minhash_status=BY-DESIGN(different perm families) |
| text/tounicode_bfrange.pdf | minhash_status=BY-DESIGN(different perm families) |
| text/vi_nfc.txt | minhash_head={'rust': ['1665173b7a6441c5', '27fe74da8b9e7412', '02a50474f988cd96', '2d06cc6b8b509d17'], 'py': ['007149b400c7f6c3', '026a44c57ef63ef3', '0428b9dea1ee51d2', '01a1c9f2a7d100e7']} · minhas |
| text/vi_nfd.txt | minhash_head={'rust': ['1665173b7a6441c5', '27fe74da8b9e7412', '02a50474f988cd96', '2d06cc6b8b509d17'], 'py': ['007149b400c7f6c3', '026a44c57ef63ef3', '0428b9dea1ee51d2', '01a1c9f2a7d100e7']} · minhas |
| video/a_128x96.mp4 | video_chain=BY-DESIGN(fps/grid differ) |
| video/a_64x48.mov | video_chain=BY-DESIGN(fps/grid differ) |
| video/a_64x48.mp4 | video_chain=BY-DESIGN(fps/grid differ) |
| video/a_crf40.mp4 | video_chain=BY-DESIGN(fps/grid differ) |
| video/a_frag.mp4 | video_chain=BY-DESIGN(fps/grid differ) |
| video/audioonly.mp4 | video_chain=BY-DESIGN(fps/grid differ) |
| video/b_64x48.mp4 | video_chain=BY-DESIGN(fps/grid differ) |
| video/e_mp4v.mp4 | video_chain=BY-DESIGN(fps/grid differ) |
| video/high_64x48.mp4 | video_chain=BY-DESIGN(fps/grid differ) |
| video/minimal.mp4 | video_chain=BY-DESIGN(fps/grid differ) |

## Pair verdicts

| pair | rust | python | agree |
|---|---|---|---|
| audio/l3_short.mp3 ↔ audio/l3_mono.mp3 | {'matched': False, 'score': 'votes=2 delta_t=-5', 'distance': 4294967293, 'error': None, 'modality': 'audio'} | {'kind': 'different', 'score': 1.0, 'hamming_py': None, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': None, 'error': None} | YES |
| audio/stereo_441.wav ↔ audio/stereo_441.wav | {'matched': True, 'score': 'votes=44 delta_t=0', 'distance': 4294967251, 'error': None, 'modality': 'audio'} | {'kind': 'exact', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': None, 'error': None} | YES |
| audio/tone.wav ↔ audio/tone.flac | {'matched': True, 'score': 'votes=99 delta_t=0', 'distance': 4294967196, 'error': None, 'modality': 'audio'} | {'kind': 'exact', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': None, 'error': None} | YES |
| bin/f_a.bin ↔ bin/f_copy.bin | {'matched': True, 'score': 'jaccard=1.000000', 'distance': 0, 'error': None, 'modality': 'binary'} | {'kind': 'exact', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': 1.0, 'jaccard_exact_py': 1.0, 'video_py': None, 'error': None} | YES |
| bin/f_a.bin ↔ bin/f_mod.bin | {'matched': True, 'score': 'jaccard=0.953488', 'distance': 46512, 'error': None, 'modality': 'binary'} | {'kind': 'near', 'score': 0.9865771812080537, 'hamming_py': None, 'jaccard_est_py': 0.9765625, 'jaccard_exact_py': 0.9865771812080537, 'video_py': None, 'error': None} | YES |
| bin/f_a.bin ↔ bin/f_other.bin | {'matched': False, 'score': 'jaccard=0.000000', 'distance': 1000000, 'error': None, 'modality': 'binary'} | {'kind': 'different', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': 0.0, 'jaccard_exact_py': 0.0, 'video_py': None, 'error': None} | YES |
| bin/f_a.bin ↔ bin/f_tail.bin | {'matched': True, 'score': 'jaccard=0.837209', 'distance': 162791, 'error': None, 'modality': 'binary'} | {'kind': 'maybe', 'score': 0.87248322147651, 'hamming_py': None, 'jaccard_est_py': 0.9140625, 'jaccard_exact_py': 0.87248322147651, 'video_py': None, 'error': None} | YES |
| bin/pack_stored.zip ↔ bin/pack_deflated.zip | {'matched': False, 'score': 'jaccard=0.083333', 'distance': 916667, 'error': None, 'modality': 'binary'} | {'kind': 'near', 'score': 1.0, 'hamming_py': None, 'jaccard_est_py': 0.703125, 'jaccard_exact_py': 0.6190476190476191, 'video_py': None, 'error': None} | NO |
| img/base_444.png ↔ img/base_444.jpg | {'matched': True, 'score': 'hamming=0', 'distance': 0, 'error': None, 'modality': 'image'} | {'kind': 'exact', 'score': 0.0, 'hamming_py': 0, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': None, 'error': None} | YES |
| img/p24_top_down.bmp ↔ img/p24_bottom_up.bmp | {'matched': True, 'score': 'hamming=0', 'distance': 0, 'error': None, 'modality': 'image'} | {'kind': 'exact', 'score': 0.0, 'hamming_py': 0, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': None, 'error': None} | YES |
| real/audio_a.wav ↔ real/audio_a_quiet.wav | {'matched': None, 'score': None, 'distance': None, 'error': 'modhash match: corpus\\real\\audio_a.wav: audio: unsupported: audio sample rate â‰\xa0 44100 Hz\nmodhash match: corpus\\real\\audio_a_quiet.wav: audio: u
| real/img_a.png ↔ real/img_b.jpg | {'matched': True, 'score': 'hamming=0', 'distance': 0, 'error': None, 'modality': 'image'} | {'kind': 'near', 'score': 0.0, 'hamming_py': 0, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': None, 'error': None} | YES |
| real/img_a.png ↔ real/img_other.png | {'matched': False, 'score': 'hamming=32', 'distance': 32, 'error': None, 'modality': 'image'} | {'kind': 'different', 'score': 1.0, 'hamming_py': 34, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': None, 'error': None} | YES |
| real/text_a.txt ↔ real/text_a_edit.txt | {'matched': True, 'score': 'jaccard=0.820312', 'distance': 179688, 'error': None, 'modality': 'text'} | {'kind': 'maybe', 'score': 0.7982456140350878, 'hamming_py': None, 'jaccard_est_py': 0.8046875, 'jaccard_exact_py': 0.7982456140350878, 'video_py': None, 'error': None} | YES |
| real/text_a.txt ↔ real/text_b.txt | {'matched': False, 'score': 'jaccard=0.000000', 'distance': 1000000, 'error': None, 'modality': 'text'} | {'kind': 'different', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': 0.0, 'jaccard_exact_py': 0.0, 'video_py': None, 'error': None} | YES |
| text/doc_a.txt ↔ text/doc_a_edit.txt | {'matched': True, 'score': 'jaccard=0.984375', 'distance': 15625, 'error': None, 'modality': 'text'} | {'kind': 'near', 'score': 0.9538461538461539, 'hamming_py': None, 'jaccard_est_py': 0.9609375, 'jaccard_exact_py': 0.9538461538461539, 'video_py': None, 'error': None} | YES |
| text/doc_a.txt ↔ text/doc_b.txt | {'matched': False, 'score': 'jaccard=0.000000', 'distance': 1000000, 'error': None, 'modality': 'text'} | {'kind': 'different', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': 0.0, 'jaccard_exact_py': 0.0, 'video_py': None, 'error': None} | YES |
| text/text_page.pdf ↔ text/standard_default.pdf | {'matched': True, 'score': 'jaccard=1.000000', 'distance': 0, 'error': None, 'modality': 'text'} | {'kind': 'exact', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': 1.0, 'jaccard_exact_py': 1.0, 'video_py': None, 'error': None} | YES |
| text/vi_nfc.txt ↔ text/vi_nfd.txt | {'matched': True, 'score': 'jaccard=1.000000', 'distance': 0, 'error': None, 'modality': 'text'} | {'kind': 'exact', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': 1.0, 'jaccard_exact_py': 1.0, 'video_py': None, 'error': None} | YES |
| video/a_64x48.mp4 ↔ video/a_128x96.mp4 | {'matched': True, 'score': 'frames=1.000000 minhash=0.000000', 'distance': 0, 'error': None, 'modality': 'video'} | {'kind': 'near', 'score': 1.0, 'hamming_py': None, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': {'ratio': 1.0, 'offset': 0, 'mean_dist': 1.25, 'matched': 32, 'coverage': 1.0, 'speed': 1.0}, 'error': None} | YES |
| video/a_64x48.mp4 ↔ video/a_64x48.mov | {'matched': True, 'score': 'frames=1.000000 minhash=1.000000', 'distance': 0, 'error': None, 'modality': 'video'} | {'kind': 'exact', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': {'ratio': 1.0, 'offset': 0, 'mean_dist': 0.0, 'matched': 32, 'coverage': 1.0, 'speed': 1.0}, 'error': None} | YES |
| video/a_64x48.mp4 ↔ video/a_crf40.mp4 | {'matched': True, 'score': 'frames=1.000000 minhash=0.000000', 'distance': 0, 'error': None, 'modality': 'video'} | {'kind': 'near', 'score': 1.0, 'hamming_py': None, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': {'ratio': 1.0, 'offset': 0, 'mean_dist': 4.75, 'matched': 32, 'coverage': 1.0, 'speed': 1.0}, 'error': None} | YES |
| video/a_64x48.mp4 ↔ video/b_64x48.mp4 | {'matched': False, 'score': 'frames=0.000000 minhash=0.000000', 'distance': 1000000, 'error': None, 'modality': 'video'} | {'kind': 'different', 'score': 0.0, 'hamming_py': None, 'jaccard_est_py': None, 'jaccard_exact_py': None, 'video_py': {'ratio': 0.0, 'offset': 8, 'mean_dist': 29.75, 'matched': 0, 'coverage': 0.0, 'speed': 1.0}, 'error': None} | YES |