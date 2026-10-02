#!/usr/bin/env python3
"""Generate `modhash-unicode/data/ucd.bin` from the UCD files in `lab/ucd/`.

Inputs (download URLs and SHA-256 digests live in `lab/ucd/PROVENANCE.md`):
  - UnicodeData.txt              canonical decompositions + combining classes
  - DerivedNormalizationProps.txt Full_Composition_Exclusion + NFC_QC
  - CompositionExclusions.txt     cross-check of the explicit exclusion set
  - NormalizationTest.txt         *self-verification*: the generator runs its
                                  own slow NFC over every column and refuses
                                  to write a blob that fails conformance.

Output blob layout (all integers little-endian, see the crate docs):
  offset 0   magic "UCD1"  (4 bytes)
  offset 4   u16 major | u16 minor | u16 update   (UCD version, 6 bytes)
  offset 10  u16 max decomposition length        (2 bytes)
  offset 12  u32 ccc_count | u32 decomp_count | u32 pair_count
  offset 24  ccc table:     cp u32, ccc u8, nfc_qc u8          (6 B/record)
  ...        decomp table:  cp u32, len u8, elems[3] u32       (17 B/record)
  ...        pair table:    first u32, second u32, comp u32    (12 B/record)

Tables are sorted by their keys so the decoder binary-searches them.
Hangul is NOT in the tables: its decompositions and compositions are
algorithmic (UAX #15 §3.12). nfc_qc is 0=Yes(default), 1=No, 2=Maybe.
"""

import struct
import sys
from pathlib import Path

UCD_DIR = Path(__file__).resolve().parent / "ucd"
OUT = Path(__file__).resolve().parent.parent / "modhash-unicode" / "data" / "ucd.bin"
UCD_VERSION = (15, 1, 0)
SBASE, LBASE, VBASE, TBASE = 0xAC00, 0x1100, 0x1161, 0x11A7
LCOUNT, VCOUNT, TCOUNT = 19, 21, 28
NCOUNT, SCOUNT = VCOUNT * TCOUNT, LCOUNT * VCOUNT * TCOUNT

# Spec-pinned table sizes for UCD 15.1.0 (spec §4.1). A different UCD drop
# changes them; the generator refuses to emit a blob it cannot explain.
EXPECTED_CANONICAL_DECOMP = 13233  # 2061 explicit + 11172 algorithmic Hangul
EXPECTED_NONZERO_CCC = 922
EXPECTED_PRECOMPOSED = 12113  # 941 explicit pairs + 11172 Hangul


def cp_range(field):
    """Expand a `start..end` or single-code-point UCD range field."""
    if ".." in field:
        a, b = field.split("..")
        return range(int(a, 16), int(b, 16) + 1)
    return range(int(field, 16), int(field, 16) + 1)


def rows(path):
    """Yield `;`-split fields of every non-comment, non-blank line."""
    for line in open(path, encoding="utf-8"):
        line = line.split("#")[0].strip()
        if line:
            yield [part.strip() for part in line.split(";")]


def load_ucd():
    ccc, decomp, qc = {}, {}, {}
    for f in rows(UCD_DIR / "UnicodeData.txt"):
        cp = int(f[0], 16)
        if int(f[3]):
            ccc[cp] = int(f[3])
        if f[5] and not f[5].startswith("<"):
            decomp[cp] = [int(x, 16) for x in f[5].split()]

    fce = set()
    for f in rows(UCD_DIR / "DerivedNormalizationProps.txt"):
        if f[1] == "Full_Composition_Exclusion":
            fce.update(cp_range(f[0]))
        elif f[1] == "NFC_QC":
            flag = {"N": 1, "M": 2}[f[2]]
            for cp in cp_range(f[0]):
                qc[cp] = flag

    explicit = set()
    for f in rows(UCD_DIR / "CompositionExclusions.txt"):
        explicit.update(cp_range(f[0]))

    # Full_Composition_Exclusion must be exactly the explicit list plus the
    # implicit classes (singleton / non-starter decompositions); verify the
    # union instead of trusting either file alone.
    implicit = {
        cp
        for cp, s in decomp.items()
        if len(s) == 1 or ccc.get(s[0], 0) != 0
    }
    if fce != explicit | implicit:
        raise SystemExit(
            f"FCE mismatch: |FCE|={len(fce)} |explicit|={len(explicit)} "
            f"|implicit|={len(implicit)}"
        )
    return ccc, decomp, fce, qc


def build_tables(ccc, decomp, fce, qc):
    hangul = [cp for cp in decomp if SBASE <= cp < SBASE + SCOUNT]
    explicit_decomp = {cp: s for cp, s in decomp.items() if cp not in hangul and not (SBASE <= cp < SBASE + SCOUNT)}

    # Spec numbers: canonical decompositions include algorithmic Hangul.
    if len(explicit_decomp) + SCOUNT != EXPECTED_CANONICAL_DECOMP:
        raise SystemExit(
            f"canonical decompositions {len(explicit_decomp) + SCOUNT} "
            f"!= spec {EXPECTED_CANONICAL_DECOMP}"
        )
    if len(ccc) != EXPECTED_NONZERO_CCC:
        raise SystemExit(f"nonzero ccc {len(ccc)} != spec {EXPECTED_NONZERO_CCC}")

    pairs = {}
    for cp, seq in explicit_decomp.items():
        if len(seq) == 2 and cp not in fce:
            pairs[(seq[0], seq[1])] = cp
    if len(pairs) + SCOUNT != EXPECTED_PRECOMPOSED:
        raise SystemExit(
            f"precomposed {len(pairs) + SCOUNT} != spec {EXPECTED_PRECOMPOSED}"
        )

    maxlen = max(len(s) for s in explicit_decomp.values())
    if maxlen > 3:
        raise SystemExit(f"decomposition longer than 3 elems: {maxlen}")

    prop = sorted({*ccc, *qc})
    ccc_tab = [(cp, ccc.get(cp, 0), qc.get(cp, 0)) for cp in prop]
    decomp_tab = sorted(
        (cp, len(s), s + [0] * (3 - len(s))) for cp, s in explicit_decomp.items()
    )
    pair_tab = sorted((a, b, c) for (a, b), c in pairs.items())
    return ccc_tab, decomp_tab, pair_tab

def emit(ccc_tab, decomp_tab, pair_tab):
    blob = bytearray()
    blob += b"UCD1"
    blob += struct.pack("<HHH", *UCD_VERSION)
    blob += struct.pack("<H", 3)
    blob += struct.pack("<III", len(ccc_tab), len(decomp_tab), len(pair_tab))
    for cp, cc, flag in ccc_tab:
        blob += struct.pack("<IBB", cp, cc, flag)
    for cp, ln, elems in decomp_tab:
        blob += struct.pack("<IB3I", cp, ln, *elems)
    for a, b, c in pair_tab:
        blob += struct.pack("<III", a, b, c)
    return bytes(blob)



def canon_order(s, ccc):
    """Canonical ordering: stable sort each non-starter run by ccc."""
    out, run = [], []
    for cp in s:
        if ccc.get(cp, 0) == 0:
            out.extend(sorted(run, key=lambda c: ccc[c]))
            run = []
            out.append(cp)
        else:
            run.append(cp)
    out.extend(sorted(run, key=lambda c: ccc[c]))
    return out



# --- self-verification: a slow, obviously-correct NFC over NormalizationTest -



def make_nfc(ccc, decomp, pair_tab):
    pair_map = {(a, b): c for a, b, c in pair_tab}
    memo = {}

    def nfd_char(cp):
        if cp in memo:
            return memo[cp]
        if SBASE <= cp < SBASE + SCOUNT:
            i = cp - SBASE
            l, v, t = LBASE + i // NCOUNT, VBASE + (i % NCOUNT) // TCOUNT, i % TCOUNT
            res = [l, v] + ([TBASE + t] if t else [])
        elif cp in decomp:
            res = []
            for e in decomp[cp]:
                res.extend(nfd_char(e))
        else:
            res = [cp]
        memo[cp] = res
        return res

    def nfc(text):
        seq = []
        for ch in text:
            seq.extend(nfd_char(ord(ch)))
        seq = canon_order(seq, ccc)
        out, starter, last_cc = [], -1, 0
        for cp in seq:
            cc = ccc.get(cp, 0)
            comp = -1
            if starter >= 0 and (last_cc < cc or last_cc == 0):
                s = out[starter]
                if LBASE <= s < LBASE + LCOUNT and VBASE <= cp < VBASE + VCOUNT:
                    comp = SBASE + (s - LBASE) * NCOUNT + (cp - VBASE) * TCOUNT
                elif SBASE <= s < SBASE + SCOUNT and (s - SBASE) % TCOUNT == 0 and TBASE < cp < TBASE + TCOUNT:
                    comp = s + cp - TBASE
                else:
                    comp = pair_map.get((s, cp), -1)
            if comp > 0:
                out[starter] = comp
            else:
                out.append(cp)
                if cc == 0:
                    starter = len(out) - 1
                last_cc = cc
        return "".join(chr(cp) for cp in out)

    return nfc


def selfcheck(nfc):
    ran = 0
    for raw in open(UCD_DIR / "NormalizationTest.txt", encoding="utf-8"):
        line = raw.split("#")[0].strip()
        if not line or line.startswith("@"):
            continue
        cols = [c.strip() for c in line.split(";")]
        if len(cols) < 5:
            continue
        seqs = [
            "".join(chr(int(h, 16)) for h in col.split()) for col in cols[:5]
        ]
        # UAX #15 conformance: toNFC(c1..c3) == c2; toNFC(c4..c5) == c4.
        for s, want in zip(seqs, (seqs[1], seqs[1], seqs[1], seqs[3], seqs[3])):
            got = nfc(s)
            if got != want:
                raise SystemExit(
                    f"selfcheck FAIL: {cols[0]!r} -> {got!r}, expected {want!r}"
                )
        ran += 1
    return ran


def main():
    ccc, decomp, fce, qc = load_ucd()
    ccc_tab, decomp_tab, pair_tab = build_tables(ccc, decomp, fce, qc)

    nfc = make_nfc(ccc, decomp, pair_tab)
    lines = selfcheck(nfc)
    print(f"selfcheck: {lines} NormalizationTest lines OK", file=sys.stderr)

    blob = emit(ccc_tab, decomp_tab, pair_tab)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(blob)
    print(
        f"wrote {OUT}: {len(blob)} bytes "
        f"(ccc {len(ccc_tab)}, decomp {len(decomp_tab)}, pairs {len(pair_tab)})\n"
        f"sha256 {hashlib.sha256(blob).hexdigest()}"
    )


if __name__ == "__main__":
    main()
