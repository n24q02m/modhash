/* Reference decode via minimp3 (public-domain), outputs s32le PCM
   scaled like ffmpeg's default (clamped to [-1,1) then *2^31). */
#define MINIMP3_IMPLEMENTATION
#define MINIMP3_ONLY_MP3
#include "ref/minimp3.h"
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
int main(int argc, char **argv) {
    if (argc < 3) return 1;
    FILE *f = fopen(argv[1], "rb");
    fseek(f, 0, SEEK_END); long n = ftell(f); fseek(f, 0, SEEK_SET);
    uint8_t *buf = malloc(n); fread(buf, 1, n, f); fclose(f);
    mp3dec_t dec; mp3dec_init(&dec);
    mp3dec_frame_info_t info;
    FILE *o = fopen(argv[2], "wb");
    int off = 0, frames = 0;
    while (off < n) {
        mp3d_sample_t pcm[MINIMP3_MAX_SAMPLES_PER_FRAME];
        int smp = mp3dec_decode_frame(&dec, buf + off, n - off, pcm, &info);
        if (info.frame_bytes <= 0) break;
        off += info.frame_bytes;
        if (smp > 0) {
            frames++;
            for (int i = 0; i < smp * info.channels; i++) {
                double x = pcm[i] / 32768.0;
                if (x >= 1.0) x = 32767.0/32768.0;
                int32_t v = (int32_t)(x * 2147483648.0);
                fwrite(&v, 4, 1, o);
            }
        }
    }
    fprintf(stderr, "%d frames\n", frames);
    fclose(o); free(buf);
    return 0;
}
