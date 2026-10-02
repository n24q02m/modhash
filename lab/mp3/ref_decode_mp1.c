#define DR_MP3_IMPLEMENTATION
#include "ref/dr_mp3.h"
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
int main(int argc, char **argv) {
    if (argc < 3) return 1;
    drmp3_config cfg; drmp3_uint64 frames;
    drmp3_int16 *pcm = drmp3_open_file_and_read_pcm_frames_s16(argv[1], &cfg, &frames, NULL);
    if (!pcm) { fprintf(stderr, "drmp3 fail\n"); return 1; }
    FILE *o = fopen(argv[2], "wb");
    for (drmp3_uint64 i = 0; i < frames * cfg.channels; i++) {
        int32_t v = (int32_t)pcm[i] << 16;
        fwrite(&v, 4, 1, o);
    }
    fprintf(stderr, "%llu frames ch%u sr%u\n", frames, cfg.channels, cfg.sampleRate);
    fclose(o); free(pcm);
    return 0;
}
