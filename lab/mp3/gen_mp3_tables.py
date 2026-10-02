#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Generate `modhash-mp3/src/hufftab.rs` Huffman tables for Layer III.

Provenance chain (documented in the generated file):
  ISO/IEC 11172-3 Table 3-B.7 (parsed from the OCR'd annex text in
  lab/mp3/ref/iso11172-3/ANNEX_AB.txt) is the normative source.
  minimp3.h's nested lookup tables are walked and REQUIRED to agree with
  the spec canonical map for every table; disagreement aborts generation.
  (libmad's huffman.c is a second reference but uses the same data.)
"""
import re, sys, pathlib

HERE = pathlib.Path(__file__).parent
SPEC = HERE / "ref" / "iso11172-3" / "ANNEX_AB.txt"
MINIMP3 = HERE / "ref" / "minimp3.h"
OUT = HERE.parent.parent / "modhash-mp3" / "src" / "hufftab.rs"

def parse_spec():
    t = SPEC.read_text(encoding="latin1")
    i = t.index("Huffman code table for quadruples (A)")
    seg = t[i:t.index("Table 3-B.8", i)]
    pair = {}
    quad = {}
    # quad tables A/B
    for name in "AB":
        m = re.search(rf"quadruples \({name}\)\s*\r?\n?\s*Value hlen hcod\r?\n(.*?)(?=\r?\n\s*Huffman|\Z)", seg, re.S)
        body = m.group(1)
        for line in body.splitlines():
            line = line.strip()
            if not line:
                continue
            mm = re.match(r"([01]{4})\s+(\d+)\s+([01]+)\s*$", line)
            if not mm:
                print("QUAD parse fail:", repr(line), file=sys.stderr); sys.exit(1)
            vbits, ln, code = mm.groups()
            v = tuple(int(b) for b in vbits)
            quad[(name, v)] = (int(ln), int(code, 2))
    # pair tables
    for m in re.finditer(r"Huffman code table (\d+)\s*\r?\n(?:x  y hlen hcod\r?\n)?(.*?)(?=\r?\nHuffman code table|\r?\n\s*Table 3-B.8|\Z)", seg, re.S):
        num = int(m.group(1))
        body = m.group(2)
        if "not used" in body or "same as table" in body:
            continue
        tab = {}
        for line in body.splitlines():
            line = line.strip()
            if not line:
                continue
            if re.match(r"^x\s+y\s+hlen", line) or "ESC table" in line:
                continue
            mm = re.match(r"(\d+)\s+(\d+)\s+(\d+)(?:\s+([01]+))?\s*$", line)
            if not mm:
                print(f"T{num} parse fail: {line!r}", file=sys.stderr); sys.exit(1)
            x, y, ln, code = mm.groups()
            tab[(int(code, 2) if code else 0, int(ln))] = (int(x), int(y))
        pair[num] = tab
    return pair, quad

# tables 17-23 share table 16's codes; 25-31 share 24's
for t in range(17, 24):
    pass

def walk_minimp3(tabs, off):
    """minimp3 nested table -> {(code,len):(x,y)}. leaf>=0 packs
    (total_len<<8 | y<<4 | x); leaf<0 descends with leaf&7 more bits."""
    result = {}
    def rec(base, prefix, plen, w):
        for i in range(1 << w):
            leaf = tabs[base + i]
            code = (prefix << w) | i
            if leaf >= 0:
                # leaf>>8 = bits consumed at THIS level (can exceed w: the
                # leaf then acts as the level-2 lookup over the extra bits)
                total = plen + (leaf >> 8)
                if leaf >> 8 <= w:
                    trunc = code >> (w - (leaf >> 8))
                else:
                    # the leaf consumed more bits than this level peeked:
                    # recover them by replaying a `leaf>>8`-wide index from
                    # the same base (minimp3 layouts repeat the leaf across
                    # the covered span, so truncating differently breaks
                    # keys). Instead store the full peek + note overhang.
                    trunc = code
                result[(trunc, total)] = (leaf & 0xF, (leaf >> 4) & 0xF)
            else:
                # subtable base is relative to the TABLE's codebook base:
                # `codebook[PEEK_BITS(w) - (leaf>>3)]` -> rec at
                # off + (-(leaf>>3)) with the next bits as index
                rec(off - (leaf >> 3), code, plen + w, leaf & 7)
    rec(off, 0, 0, 5)
    return result

def minimp3_tabs():
    src = MINIMP3.read_text()
    i = src.index("static const int16_t tabs[]")
    body = src[i:src.index("};", i)].split("{", 1)[1]
    return [int(x) for x in re.findall(r"-?\d+", body)]

def minimp3_quad(name):
    src = MINIMP3.read_text()
    i = src.index("static const uint8_t " + name)
    body = src[i:src.index("};", i)].split("{", 1)[1]
    tab = [int(x) for x in re.findall(r"\d+", body)]
    res = {}
    for i in range(16):
        leaf = tab[i]
        if leaf & 8:
            n = leaf & 7
            res[(i >> (4 - n), n)] = tuple(1 if leaf & (128 >> s) else 0 for s in range(4))
        else:
            sub, nw = leaf >> 3, leaf & 3
            for j in range(1 << nw):
                leaf2 = tab[sub + j]
                assert leaf2 & 8, (name, i, j)
                n = leaf2 & 7
                res[(((i << nw) | j) >> (4 + nw - n), n)] = tuple(
                    1 if leaf2 & (128 >> s) else 0 for s in range(4))
    return res

TABINDEX = [0,32,64,98,0,132,180,218,292,364,426,538,648,746,0,1126,
            1460,1460,1460,1460,1460,1460,1460,1460,1842,1842,1842,1842,
            1842,1842,1842,1842]
# spec table number -> shared base table
SHARE = {t: 16 for t in range(17, 24)}
SHARE.update({t: 24 for t in range(25, 32)})

def main():
    pair, quad = parse_spec()
    tabs = minimp3_tabs()
    for t, m in sorted(pair.items()):
        m3 = walk_minimp3(tabs, TABINDEX[t])
        if m3 != m:
            print(f"table {t} MISMATCH", file=sys.stderr)
            for k in sorted(set(m) | set(m3)):
                if m.get(k) != m3.get(k):
                    print(f"  {k}: spec={m.get(k)} mp3={m3.get(k)}", file=sys.stderr)
            sys.exit(1)
    for name, q in (("A", "tab32"), ("B", "tab33")):
        m3 = minimp3_quad(q)
        sq = {(code, ln): v for (tbl, v), (ln, code) in quad.items()
              if tbl == name}
        if m3 != sq:
            print(f"quad {name} MISMATCH", file=sys.stderr)
            for k in sorted(set(m3) | set(sq)):
                if m3.get(k) != sq.get(k):
                    print(f"  {k}: spec={sq.get(k)} mp3={m3.get(k)}", file=sys.stderr)
            sys.exit(1)
    emit(pair, quad)

LINBITS = {16:1,17:2,18:3,19:4,20:6,21:8,22:10,23:13,
           24:4,25:5,26:6,27:7,28:8,29:9,30:11,31:13}

def emit(pair, quad):
    out = []
    out.append("//! Generated Layer III Huffman tables. DO NOT EDIT.")
    out.append("//!")
    out.append("//! Source: ISO/IEC 11172-3 Table 3-B.7, transcribed by")
    out.append("//! `lab/mp3/gen_mp3_tables.py` from the normative annex text and")
    out.append("//! cross-checked bit-exact against minimp3's decoder tables.")
    out.append("//! `code` is the hcod value right-aligned; `len` is hlen. For the")
    out.append("//! quad tables, `x` packs (v,w,x,y) as v<<3|w<<2|x<<1|y.")
    out.append("")
    out.append("/// One entry: `code` over `len` bits decodes to (`x`, `y`).")
    out.append("#[derive(Clone, Copy)]")
    out.append("pub(crate) struct HuffEntry {")
    out.append("    /// Huffman code bits, MSB-first.")
    out.append("    pub code: u32,")
    out.append("    /// Code length.")
    out.append("    pub len: u8,")
    out.append("    /// First value (or packed nibbles for quad tables).")
    out.append("    pub x: u8,")
    out.append("    /// Second value (0 for quad tables).")
    out.append("    pub y: u8,")
    out.append("}")
    out.append("")
    out.append("/// A pair table with its `linbits` escape-field width.")
    out.append("#[derive(Clone, Copy)]")
    out.append("pub(crate) struct HuffTable {")
    out.append("    /// Entries sorted by length then code.")
    out.append("    pub entries: &'static [HuffEntry],")
    out.append("    /// Extra bits after a `15` value (ESC).")
    out.append("    pub linbits: u8,")
    out.append("}")
    out.append("")
    for t in range(32):
        base = SHARE.get(t, t)
        m = pair.get(base)
        if m is None:
            out.append(f"const HUFF_{t}: &[HuffEntry] = &[];")
            continue
        items = sorted((ln, code, x, y) for (code, ln), (x, y) in m.items())
        out.append(f"const HUFF_{t}: &[HuffEntry] = &[")
        for ln, code, x, y in items:
            out.append(f"    HuffEntry {{ code: 0b{code:0{ln}b}, len: {ln}, x: {x}, y: {y} }},")
        out.append("];")
        out.append("")
    for name, q in (("HUFF_QUAD_A", "A"), ("HUFF_QUAD_B", "B")):
        items = sorted((ln, code, v) for (tbl, v), (ln, code) in quad.items()
                       if tbl == q)
        out.append(f"pub(crate) static {name}: &[HuffEntry] = &[")
        for ln, code, v in items:
            packed = v[0] << 3 | v[1] << 2 | v[2] << 1 | v[3]
            out.append(f"    HuffEntry {{ code: 0b{code:0{ln}b}, len: {ln}, x: {packed}, y: 0 }},")
        out.append("];")
        out.append("")
    out.append("/// Pair tables indexed by `table_select` (0..=31).")
    out.append("pub(crate) static HUFF_TABLES: [HuffTable; 32] = [")
    for t in range(32):
        out.append(f"    HuffTable {{ entries: HUFF_{t}, linbits: {LINBITS.get(t, 0)} }},")
    out.append("];")
    out.append("")
    OUT.write_text("\n".join(out) + "\n")
    npair = sum(len(pair.get(SHARE.get(t, t), {})) for t in range(32) if pair.get(SHARE.get(t, t)))
    print(f"wrote {OUT}: {npair} pair entries, {len(quad)//2} quad entries each")

if __name__ == "__main__":
    main()
