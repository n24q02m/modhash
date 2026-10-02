"""Emit `src/tables.rs`: the five predefined 8-bit encodings and the glyph-name
lookup used for `/Differences` arrays.

Sources (installed wheels, printed inline so the output is self-checking):

- StandardEncoding / WinAnsiEncoding / MacRomanEncoding / SymbolEncoding /
  ZapfDingbatsEncoding name tables: reportlab 5.0.1 `_fontdata_enc_*` modules
  (which transcribe PDF 32000-1:2008 Annex D tables D.1/D.2) — used only to
  validate that every emitted name resolves in `adobe_glyphs`.
- Direct code->Unicode tables: pypdf 6.13.3 `_codecs/{std,symbol,zapfding,pdfdoc}.py`,
  where the module headers cite unicode.org VENDORS/ADOBE mapping files
  (zdingbat.txt et al). WinAnsi = cp1252 and MacRoman = mac_roman are cross
  checked against the Python built-in codecs.
- Glyph-name -> Unicode (`GLYPH_UNICODE`): pypdf 6.13.3
  `_codecs/adobe_glyphs.py` (Adobe Glyph List, 14k entries), reduced to the
  union of names used by the five predefined encodings plus a curated set of
  common `/Differences` names (ligatures, quotes, dashes, math, latin
  supplement) to keep the table near ~1k entries.
"""

from __future__ import annotations

import codecs

import pypdf._codecs.adobe_glyphs as AGL  # glyph name -> str
import pypdf._codecs.pdfdoc as pdfdoc
import pypdf._codecs.std as std
import pypdf._codecs.symbol as symbol
import pypdf._codecs.zapfding as zapfding

# reportlab name tables (PDF 32000 Annex D)
from reportlab.pdfbase import (
    _fontdata_enc_macroman as rmac,
    _fontdata_enc_pdfdoc as rp,
    _fontdata_enc_standard as rstd,
    _fontdata_enc_symbol as rsym,
    _fontdata_enc_winansi as rwin,
    _fontdata_enc_zapfdingbats as rzap,
)
OUT = "src/tables.rs"


def rust_str(s: str, *, name: bool = False) -> str:
    # values are short strings; names are ordinary identifiers
    assert name or len(s) <= 2, s
    out = '"'
    for ch in s:
        cp = ord(ch)
        if ch == '"':
            out += '\\"'
        elif ch == "\\":
            out += "\\\\"
        elif 0x20 <= cp < 0x7F:
            out += ch
        else:
            out += "\\u{%x}" % cp
    return out + '"'


def emit_enc_table(names, direct, fname, prefer):
    """names[i] = glyph name or None; direct[i] = str or '' (pypdf direct table).

    `prefer` selects the primary source: 'names' resolves Annex D glyph names
    through the Adobe Glyph List (correct for Standard/WinAnsi/MacRoman/
    PDFDoc); 'direct' uses pypdf's code->unicode table (required for Symbol
    and ZapfDingbats, where the a## glyph names follow a different convention
    than Annex D.5/D.6 codepoints). The other source fills leftover slots."""
    rows = []
    used = 0
    for i in range(256):
        by_name = (
            AGL["/" + names[i]]
            if names[i] and ("/" + names[i]) in AGL
            else None
        )
        if prefer == "names":
            # AGL-resolved names win; unnamed slots fall back to `direct`
            # only when direct is a real codec value, not pypdf's chr(i)
            # placeholder (which would map every .notdef byte to itself)
            d = direct[i] or None
            if d and len(d) == 1 and ord(d) == i and names[i] is None:
                d = None
            cand = (by_name, d)
        else:
            # pypdf pads unassigned slots with chr(i); those are placeholders,
            # not mappings -- drop them when Annex D has no name for the slot
            d = direct[i] or None
            if d and len(d) == 1 and ord(d) == i and names[i] is None:
                d = None
            cand = (d, by_name)
        ch = next((c for c in cand if c and len(c) == 1), None)
        if ch is not None:
            rows.append((i, rust_str(ch)))
            used += 1
    body = "\n".join(
        "    (0x%02X, %s)," % (i, s) for i, s in rows
    )
    return f"pub(crate) static {fname}: &[(u8, &str)] = &[\n{body}\n];\n", used


def main() -> None:
    # --- validate every predefined-encoding name resolves in AGL -------------
    bad = []
    for enc in (rstd.StandardEncoding, rwin.WinAnsiEncoding, rmac.MacRomanEncoding,
                rsym.SymbolEncoding, rzap.ZapfDingbatsEncoding):
        for i, n in enumerate(enc):
            if n is not None and ("/" + n) not in AGL:
                bad.append((i, n))
    assert not bad, bad

    # --- direct tables -------------------------------------------------------
    def tab(mod, attr):
        return getattr(mod, attr)

    # Codecs are fallbacks only: where reportlab's Annex D name table has a
    # name, AGL decides. cp1252 0x7F is DEL but PDF WinAnsi 0x7F is bullet,
    # so the codec must never override a named slot.
    winansi = []
    for i in range(256):
        n = rwin.WinAnsiEncoding[i]
        if n:
            winansi.append(AGL["/" + n])
        else:
            try:
                winansi.append(codecs.decode(bytes([i]), "cp1252"))
            except Exception:
                winansi.append("")
    macroman = []
    for i in range(256):
        n = rmac.MacRomanEncoding[i]
        if n:
            macroman.append(AGL["/" + n])
        else:
            try:
                macroman.append(codecs.decode(bytes([i]), "mac_roman"))
            except Exception:
                macroman.append("")

    enc_tables = {}
    enc_tables["STANDARD_ENCODING"] = emit_enc_table(
        rstd.StandardEncoding, tab(std, "_std_encoding"), "STANDARD_ENCODING",
        "names")
    enc_tables["WINANSI_ENCODING"] = emit_enc_table(
        rwin.WinAnsiEncoding, winansi, "WINANSI_ENCODING", "names")
    enc_tables["MACROMAN_ENCODING"] = emit_enc_table(
        rmac.MacRomanEncoding, macroman, "MACROMAN_ENCODING", "names")
    enc_tables["SYMBOL_ENCODING"] = emit_enc_table(
        rsym.SymbolEncoding, tab(symbol, "_symbol_encoding"), "SYMBOL_ENCODING",
        "direct")
    enc_tables["ZAPFDINGBATS_ENCODING"] = emit_enc_table(
        rzap.ZapfDingbatsEncoding, tab(zapfding, "_zapfding_encoding"),
        "ZAPFDINGBATS_ENCODING", "direct")
    enc_tables["PDFDOC_ENCODING"] = emit_enc_table(
        rp.PDFDocEncoding, tab(pdfdoc, "_pdfdoc_encoding"), "PDFDOC_ENCODING",
        "names")

    # --- GLYPH_UNICODE: names used by the encodings + common extras ----------
    used_names = set()
    for enc in (rstd.StandardEncoding, rwin.WinAnsiEncoding, rmac.MacRomanEncoding,
                rsym.SymbolEncoding, rzap.ZapfDingbatsEncoding, rp.PDFDocEncoding):
        used_names.update(n for n in enc if n)
    extras = """fi fl ffi ffl ff ij IJ OE oe Ldot ldot Zcaron zcaron
        nbspace exclamdbl hungarumlaut centereddot breve brevespace caron ogonek
        smalltilde dotlessi dotlessj scaron Scommaaccent scommaaccent Tcommaaccent
        tcommaaccent Ccedilla s_f_raquo l l o g i c n t e r daggerdbl dagger
        perthousand bullet quotesinglbase quotedblbase guilsinglright
        guilsinglleft endash emdash minus plusminus numbersign percent thousand
        logicalnot integral summation product partialdiff infinity approxequal
        notequal lessequal greaterequal element suchthat proportional radical
        radicalspace ring angle and or intersection union therefore thereforesm
        existential universal emptyset gradient nabla equalorequal membership
        reflexsubset reflexsuperset notsubset orthogonal lozenge spade club heart
        diamond angleleft angleright arrowleft arrowright arrowup arrowdown
        arrowdblleft arrowdblright arrowdblup arrowdbldown arrowsimple
        arrowvertex circleplus circlemultiply relation aleph Ifraktur Rfraktur
        weierstrass image real SF100000 SF110000 greaterequaloverequal
        coloncurrency cruzeiro franc lira mill peseta cent sterling currency yen
        brokenbar section dieresis copyright ord feminine guillemotleft
        guillemotright registered macron degree cedilla onesuperior
        masculine acute multiplication division onequarter onehalf
        threequarters questiondown Agrave Aacute Acircumflex Atilde Adieresis
        Aring AE Ccedilla Egrave Eacute Ecircumflex Edieresis Igrave Iacute
        Icircumflex Idieresis Eth Ntilde Ograve Oacute Ocircumflex Otilde
        Odieresis Oslash Ugrave Uacute Ucircumflex Udieresis Yacute Thorn
        germandbls agrave aacute acircumflex atilde adieresis aring ae ccedilla
        egrave eacute ecircumflex edieresis igrave iacute icircumflex idieresis
        eth ntilde ograve oacute ocircumflex otilde odieresis divide oslash
        ugrave uacute ucircumflex udieresis yacute thorn ydieresis
        quotesingle quotedblleft quotedblright quoteleft quoteright
        quotereversed comma period ellipsis trademark asciicircum asciitilde
        braceleft braceright bracketleft bracketright parenleft parenright
        backslash bar asterisk slash at numbersign dollar Euro florin
        Cyrillic afii10017 afii10018 afii10019 afii10020 afii10021 afii10022
        afii10023 afii10024 afii10025 afii10026 afii10027 afii10028 afii10029
        afii10030 afii10031 afii10032 afii10033 afii10034 afii10035 afii10036
        afii10037 afii10038 afii10039 afii10040 afii10041 afii10042 afii10043
        afii10044 afii10045 afii10046 afii10047 afii10048 afii10049 afii10050
        afii10051 afii10052 afii10053 afii10054 afii10055 afii10056 afii10057
        afii10058 afii10059 afii10060 afii10061 afii10062 afii10063 afii10064
        afii10065 afii10066 afii10067 afii10068 afii10069 afii10070 afii10071
        afii10072 afii10073 afii10074 afii10075 afii10076 afii10077 afii10078
        afii10079 afii10080 afii10081 afii10082 afii10083 afii10084 afii10085
        afii10086 afii10087 afii10088 afii10089 afii10090 afii10091 afii10092
        afii10093 afii10094 afii10095 afii10096 afii10097 afii10098 afii10099
        afii10100 afii10101 afii10102 afii10103 afii10104 afii10105 afii10106
        afii10107 afii10108 afii10109 afii10110 afii10145 afii10146 afii10147
        afii10148 afii10193 afii10194 afii10195 afii10196 afii10846 afii20841
        uniFB01 uniFB02 uniFB03 uniFB04 Wcircumflex wcircumflex Wdieresis
        wdieresis Ydieresis circumflexacute circumflexgrave""".split()
    used_names.update(extras)

    glyph_rows = []
    missing = []
    for n in sorted(used_names):
        v = AGL.get("/" + n)
        if v is None:
            missing.append(n)
            continue
        glyph_rows.append((n, rust_str(v)))
    if missing:
        print("NOTE names with no AGL entry (kept out):", missing[:40])

    glyph_table = "pub(crate) static GLYPH_UNICODE: &[(&str, &str)] = &[\n" + "\n".join(
        '    (%s, %s),' % (rust_str(n, name=True), s) for n, s in glyph_rows) + "\n];\n"

    header = """//! Predefined encodings and the glyph-name table.
//!
//! GENERATED by `tools/gen_tables.py` from installed wheels of
//! `pypdf` 6.13.3 (`_codecs/*`, `_codecs/adobe_glyphs.py` -- the Adobe Glyph
//! List) and `reportlab` 5.0.1 (`_fontdata_enc_*`, transcribing PDF 32000-1:2008
//! Annex D). Do not edit by hand; regenerate after changing the generator.
//!
//! Each encoding table maps an 8-bit code to the Unicode string the PDF
//! spec assigns to that slot; absent slots are simply missing entries. The
//! `GLYPH_UNICODE` table maps PostScript glyph names (as they appear in
//! `/Differences` arrays) to Unicode; `/uniXXXX`/`uXXXXXX` names are decoded
//! before this table is consulted.

#![allow(clippy::missing_docs_in_private_items)]

"""

    with open(OUT, "w", encoding="utf8", newline="\n") as f:
        f.write(header)
        for src in enc_tables.values():
            f.write(src[0])
            f.write("\n")
        f.write(glyph_table)
        f.write("\n")
        f.write(
            "/// AGL name -> Unicode for Differences entries.\n"
            "pub(crate) fn glyph_unicode(name: &[u8]) -> Option<&'static str> {\n"
            "    GLYPH_UNICODE\n"
            "        .binary_search_by(|(n, _)| n.as_bytes().cmp(name))\n"
            "        .ok()\n"
            "        .map(|i| GLYPH_UNICODE[i].1)\n"
            "}\n"
        )
    for k, (_, n) in enc_tables.items():
        print(k, n, "assigned slots")
    print("GLYPH_UNICODE:", len(glyph_rows), "names")
    print("wrote", OUT)


if __name__ == "__main__":
    main()
