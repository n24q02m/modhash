#!/usr/bin/env python3
"""Emit `reservoir_pair.mp3`: a two-frame MPEG-1 Layer III stream that
exercises the bit reservoir's `main_data_begin` back-pointer.

Frame 1 (donor): the same 32-bit header as the target, a side-info
block of zeroed granule fields (main_data_begin = 0, both granules
empty), and a main-data payload holding the last 511 bytes of the
history stream `l3_mono.mp3` had accumulated before its frame 4.

Frame 2 (target): `l3_mono.mp3`'s frame 4 verbatim, whose
`main_data_begin = 67` points 67 bytes back into the donor's
contribution — its granules cannot decode from its own frame bytes
alone.

Run from the worktree root:  python lab/mp3/make_reservoir_pair.py
"""

SRC = "modhash-mp3/tests/fixtures/l3_mono.mp3"
OUT = "modhash-mp3/tests/fixtures/reservoir_pair.mp3"

BR3 = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0]
SR = [44100, 48000, 32000]


def frames(d):
    pos = 44  # past the ID3v2 tag
    out = []
    while pos + 4 < len(d):
        h = int.from_bytes(d[pos:pos + 4], "big")
        if h >> 21 != 0x7FF:
            break
        fl = 144 * BR3[(h >> 12) & 15] * 1000 // SR[(h >> 10) & 3] + ((h >> 9) & 1)
        out.append((pos, fl))
        pos += fl
    return out


def main():
    d = open(SRC, "rb").read()
    fr = frames(d)
    # Mono, unprotected: side info = 17 bytes, so main_data starts at
    # frame_offset + 21.
    history = b"".join(d[p + 21:p + fl] for p, fl in fr[:4])
    tpos, tfl = fr[4]
    hdr = d[tpos:tpos + 4]
    donor_main = history[-511:]
    donor = (hdr + bytes(17) + donor_main
             + bytes(max(0, tfl - 4 - 17 - len(donor_main))))[:tfl]
    assert len(donor) == tfl
    open(OUT, "wb").write(donor + d[tpos:tpos + tfl])
    print(f"wrote {OUT}: {tfl * 2} bytes")


if __name__ == "__main__":
    main()
