"""Generate `tests/fixtures/*.pdf` + `*.txt` expectations for modhash-pdf.

Byte-exact PDF writer (no external PDF lib writes the files themselves):
every object is emitted with a computed xref, three xref shapes are
supported (classic table, xref stream, xref stream + object streams), and
the encrypted fixture uses real RC4-40 (V1/R2) so that a conforming
decryptor - pypdf - can still read it, proving the refusal is a policy,
not a parse failure.

Expected text (`*.txt`) is the canonical rendering of the generator's own
text operators under the documented extraction rules (Td/Tm newline and
space detection, TJ gap threshold -250). pypdf 6.13.3 is additionally run
as an oracle on the encrypted fixture: it must decrypt and see the same
words (its spacing heuristics differ, so equality is not asserted).

Determinism: fixed /ID, no timestamps, zlib level 6 (stable output).
Regenerate: `python tools/gen_fixtures.py` from the crate root.
"""

from __future__ import annotations

import hashlib
import struct
import zlib
from pathlib import Path

FIX = Path("tests/fixtures")
PAD = bytes(
    [
        0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56,
        0xFF, 0xFA, 0x01, 0x08, 0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80,
        0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
    ]
)

# ---------------------------------------------------------------------------
# RC4 + Standard-security-handler primitives (PDF 32000-1 7.6.3)
# ---------------------------------------------------------------------------


def rc4(key: bytes, data: bytes) -> bytes:
    s = list(range(256))
    j = 0
    for i in range(256):
        j = (j + s[i] + key[i % len(key)]) % 256
        s[i], s[j] = s[j], s[i]
    out = bytearray()
    i = j = 0
    for b in data:
        i = (i + 1) % 256
        j = (j + s[i]) % 256
        s[i], s[j] = s[j], s[i]
        out.append(b ^ s[(s[i] + s[j]) % 256])
    return bytes(out)


def md5(b: bytes) -> bytes:
    return hashlib.md5(b).digest()


class StandardCrypt:
    """V=1 R=2 40-bit RC4 standard handler, empty user password."""

    def __init__(self, file_id: bytes):
        self.id0 = file_id
        p = struct.pack("<i", -4)
        self.o = rc4(md5(PAD)[:5], PAD)  # owner == user == ""
        self.key = md5(PAD + self.o + p + self.id0)[:5]
        self.u = rc4(self.key, PAD)

    def obj_key(self, num: int, gen: int) -> bytes:
        extra = struct.pack("<I", num)[:3] + struct.pack("<I", gen)[:2]
        return md5(self.key + extra)[:10]

    def encrypt(self, num: int, gen: int, data: bytes) -> bytes:
        return rc4(self.obj_key(num, gen), data)


def unescape_literal(raw: bytes) -> bytes:
    """Decode a literal string body (between parens) to its data bytes."""
    out = bytearray()
    i = 0
    while i < len(raw):
        b = raw[i]
        if b == 0x5C and i + 1 < len(raw):
            n = raw[i + 1]
            if n in b"nrtbf":
                out.append({110: 10, 114: 13, 116: 9, 98: 8, 102: 12}[n])
                i += 2
            elif n in b"()\\":
                out.append(n)
                i += 2
            elif 0x30 <= n <= 0x37:
                j = i + 1
                v = 0
                while j < len(raw) and j < i + 4 and 0x30 <= raw[j] <= 0x37:
                    v = v * 8 + raw[j] - 0x30
                    j += 1
                out.append(v & 0xFF)
                i = j
            elif n in (0x0D, 0x0A):  # line continuation
                i += 2
                if n == 0x0D and i < len(raw) and raw[i] == 0x0A:
                    i += 1
            else:
                out.append(n)
                i += 2
        else:
            out.append(b)
            i += 1
    return bytes(out)


# ---------------------------------------------------------------------------
# byte-level PDF writer
# ---------------------------------------------------------------------------


class PdfWriter:
    """Emit a minimal but fully-correct PDF: exact xref offsets, classic
    tables or xref streams (with optional object streams), and optional
    Standard-handler encryption (V1/R2; strings and streams encrypted per
    spec, encrypted strings re-encoded as hex)."""

    def __init__(self, crypt: StandardCrypt | None = None):
        self.objs: dict[int, tuple[int, bytes]] = {}  # num -> (gen, body)
        self.crypt = crypt

    def add(self, body: bytes, gen: int = 0) -> int:
        num = max(self.objs, default=0) + 1
        self.objs[num] = (gen, body)
        return num

    def reserve(self) -> int:
        return self.add(b"null")

    def set(self, num: int, body: bytes, gen: int = 0) -> None:
        self.objs[num] = (gen, body)

    def stream(self, dict_extra: str, data: bytes, compress: bool = True) -> bytes:
        body = data
        filt = ""
        if compress:
            body = zlib.compress(data, 6)
            filt = " /Filter /FlateDecode"
        d = f"<< /Length {len(body)}{filt}{dict_extra} >>".encode()
        return d + b"\nstream\n" + body + b"\nendstream"

    # -- serialization -------------------------------------------------------

    def _crypt_body(self, num: int, gen: int, body: bytes) -> bytes:
        """Encrypt the bytes of a stream or the data of each literal string.

        Strings: unescape -> RC4 -> hex-encode (keeps the file syntactically
        valid). Streams: RC4 in place (RC4 is length-preserving, so the
        declared /Length stays correct).
        """
        c = self.crypt
        if c is None:
            return body
        si = body.find(b"\nstream\n")
        if si >= 0:
            si += len(b"\nstream\n")
            ei = body.rfind(b"\nendstream")
            if ei > si:
                return body[:si] + c.encrypt(num, gen, body[si:ei]) + body[ei:]
            return body
        out = bytearray()
        i = 0
        while i < len(body):
            if body[i] != 0x28:  # not '('
                out.append(body[i])
                i += 1
                continue
            depth = 1
            j = i + 1
            raw = bytearray()
            while j < len(body) and depth:
                b = body[j]
                if b == 0x5C:
                    raw.append(b)
                    j += 1
                    if j < len(body):
                        raw.append(body[j])
                elif b == 0x28:
                    depth += 1
                    raw.append(b)
                elif b == 0x29:
                    depth -= 1
                    if depth:
                        raw.append(b)
                else:
                    raw.append(b)
                j += 1
            enc = c.encrypt(num, gen, unescape_literal(bytes(raw)))
            out += b"<" + enc.hex().encode() + b">"
            i = j
        return bytes(out)

    def build(
        self,
        root: int,
        xref: str = "table",
        objstm: set[int] | None = None,
        w: tuple[int, int, int] = (1, 4, 2),
        corrupt_xref: bool = False,
    ) -> bytes:
        objs = dict(self.objs)

        # pack object-stream members (only reachable via xref streams) -------
        objstm_members: dict[int, tuple[int, int]] = {}
        if objstm:
            stm_num = max(objs) + 1
            header = bytearray()
            blob = bytearray()
            idx = 0
            for num in sorted(objstm):
                gen, body = objs.pop(num)
                header += f"{num} {len(blob)} ".encode()
                blob += self._crypt_body(num, gen, body)
                objstm_members[num] = (stm_num, idx)
                idx += 1
            first = len(header)
            comp = zlib.compress(bytes(header + blob), 6)
            objs[stm_num] = (
                0,
                f"<< /Type /ObjStm /N {len(objstm_members)} /First {first} "
                f"/Length {len(comp)} /Filter /FlateDecode >>".encode()
                + b"\nstream\n" + comp + b"\nendstream",
            )

        out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
        offsets: dict[int, int] = {}
        gens: dict[int, int] = {}
        for num in sorted(objs):
            gen, body = objs[num]
            offsets[num] = len(out)
            gens[num] = gen
            out += f"{num} {gen} obj\n".encode()
            out += self._crypt_body(num, gen, body)
            out += b"\nendobj\n"

        fid = md5(b"modhash-pdf fixture")
        idstr = b"<" + fid.hex().encode() + b">"
        trailer_extra = b""
        if self.crypt is not None:
            enc_num = max(objs) + 1
            enc_body = (
                b"<< /Filter /Standard /V 1 /R 2 /Length 40 /P -4 /O <"
                + self.crypt.o.hex().encode()
                + b"> /U <"
                + self.crypt.u.hex().encode()
                + b"> >>"
            )
            offsets[enc_num] = len(out)
            gens[enc_num] = 0
            out += f"{enc_num} 0 obj\n".encode() + enc_body + b"\nendobj\n"
            trailer_extra += f" /Encrypt {enc_num} 0 R".encode()

        size = max(offsets) + 1
        xref_off = len(out)

        if corrupt_xref:
            out += b"xref\n0 %d\n" % size
            for n in range(size):
                if n in offsets:
                    gen = objs[n][0]
                    out += b"%010d %05d n \n" % ((offsets[n] + 4001) % 9999999999, gens[n])
                else:
                    out += b"0000000000 65535 f \n"
            out += (
                b"trailer\n<< /Size %d /Root %d 0 R%s /ID [%s %s] >>\n"
                % (size, root, trailer_extra, idstr, idstr)
            )
            out += b"startxref\n77\n%%EOF\n"
            return bytes(out)

        if xref == "table":
            out += b"xref\n0 %d\n" % size
            for n in range(size):
                if n in offsets:
                    out += b"%010d %05d n \n" % (offsets[n], gens[n])
                else:
                    out += b"0000000000 65535 f \n"
            out += (
                b"trailer\n<< /Size %d /Root %d 0 R%s /ID [%s %s] >>\n"
                % (size, root, trailer_extra, idstr, idstr)
            )
            out += b"startxref\n%d\n%%%%EOF\n" % xref_off
            return bytes(out)

        # ---- xref stream: sparse /Index runs over present objects -----------
        w1, w2, w3 = w
        present = sorted(set(offsets) | set(objstm_members))
        idx: list[int] = []
        start = prev = None
        for n in present:
            if start is None:
                start = prev = n
            elif n == prev + 1:
                prev = n
            else:
                idx += [start, prev - start + 1]
                start = prev = n
        if start is not None:
            idx += [start, prev - start + 1]
        entries = bytearray()
        for n in present:
            if n in offsets:
                t, a, b = 1, offsets[n], gens[n]
            else:
                t, a, b = 2, *objstm_members[n]
            e = t.to_bytes(w1, "big") + a.to_bytes(w2, "big")
            if w3:
                e += b.to_bytes(w3, "big")
            entries += e
        comp = zlib.compress(bytes(entries), 6)
        xs = (
            f"<< /Type /XRef /Size {size} /W [{w1} {w2} {w3}] "
            f"/Index [{' '.join(map(str, idx))}] /Length {len(comp)} "
            f"/Filter /FlateDecode /Root {root} 0 R".encode()
            + trailer_extra
            + b" /ID [" + idstr + b" " + idstr + b"] >>"
            + b"\nstream\n"
            + comp
            + b"\nendstream"
        )
        out += f"{size} 0 obj\n".encode() + xs + b"\nendobj\n"
        out += b"startxref\n%d\n%%%%EOF\n" % xref_off
        return bytes(out)


# ---------------------------------------------------------------------------
# cmap + literal helpers
# ---------------------------------------------------------------------------


def to_unicode_cmap(
    bfchar: list[tuple[bytes, bytes]],
    bfrange: list[tuple[bytes, bytes, "bytes | list[bytes]"]],
    codespaces: list[tuple[bytes, bytes]] | None = None,
    wmode: int = 0,
) -> bytes:
    def h(b: bytes) -> str:
        return "<" + b.hex() + ">"

    if codespaces is None:
        codespaces = [(b"\x00", b"\xff")]
    lines = [
        "/CIDInit /ProcSet findresource begin",
        "12 dict begin",
        "begincmap",
        "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def",
        "/CMapName /Adobe-Identity-UCS def",
        "/CMapType 2 def",
        f"/WMode {wmode} def",
        f"{len(codespaces)} begincodespacerange",
    ]
    for lo, hi in codespaces:
        lines.append(f"{h(lo)} {h(hi)}")
    lines.append("endcodespacerange")
    if bfchar:
        lines.append(f"{len(bfchar)} beginbfchar")
        for src, dst in bfchar:
            lines.append(f"{h(src)} {h(dst)}")
        lines.append("endbfchar")
    if bfrange:
        lines.append(f"{len(bfrange)} beginbfrange")
        for lo2, hi2, dst in bfrange:
            if isinstance(dst, list):
                lines.append(f"{h(lo2)} {h(hi2)} [" + " ".join(h(d) for d in dst) + "]")
            else:
                lines.append(f"{h(lo2)} {h(hi2)} {h(dst)}")
        lines.append("endbfrange")
    lines += [
        "endcmap",
        "CMapName currentdict /CMap defineresource pop",
        "end",
        "end",
    ]
    return "\n".join(lines).encode()


def utf16(s: str) -> bytes:
    return s.encode("utf-16-be")


def lit(s: str, enc: str = "cp1252") -> bytes:
    return lit_bytes(s.encode(enc))


def lit_bytes(raw: bytes) -> bytes:
    out = []
    for b in raw:
        c = chr(b)
        if c in "()\\":
            out.append("\\" + c)
        elif 0x20 <= b < 0x7F:
            out.append(c)
        else:
            out.append("\\%03o" % b)
    return ("(" + "".join(out) + ")").encode("latin1")


# ---------------------------------------------------------------------------
# document skeletons
# ---------------------------------------------------------------------------


def font_simple(subtype: str, base: str, enc: str | None, touni=None) -> bytes:
    s = f"<< /Type /Font /Subtype /{subtype} /BaseFont /{base}"
    if enc:
        s += f" /Encoding {enc}"
    if touni is not None:
        s += f" /ToUnicode {touni} 0 R"
    return (s + " >>").encode()


def build_doc(
    pages: list[dict],
    xref: str = "table",
    objstm: set[int] | None = None,
    w=(1, 4, 2),
    crypt: StandardCrypt | None = None,
    corrupt_xref: bool = False,
) -> bytes:
    """pages[i] = {'fonts': {name: body|num}, 'xobjects': {..},
    'contents': [bytes|num]}."""
    wtr = PdfWriter(crypt)
    catalog = wtr.reserve()
    pages_id = wtr.reserve()
    kids: list[int] = []
    for p in pages:
        fonts = {
            name: spec if isinstance(spec, int) else wtr.add(spec)
            for name, spec in p["fonts"].items()
        }
        xobjs = {
            name: spec if isinstance(spec, int) else wtr.add(spec)
            for name, spec in p.get("xobjects", {}).items()
        }
        cids = [c if isinstance(c, int) else wtr.add(c) for c in p["contents"]]
        res = b"<< "
        if fonts:
            res += b"/Font << " + b" ".join(
                f"/{k} {v} 0 R".encode() for k, v in fonts.items()
            ) + b" >> "
        if xobjs:
            res += b"/XObject << " + b" ".join(
                f"/{k} {v} 0 R".encode() for k, v in xobjs.items()
            ) + b" >> "
        res += b">>"
        contents = (
            f"{cids[0]} 0 R".encode()
            if len(cids) == 1
            else b"[ " + b" ".join(f"{c} 0 R".encode() for c in cids) + b" ]"
            if cids
            else None
        )
        page = (
            f"<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 612 792] "
            f"/Resources {res.decode()}"
            + (f" /Contents {contents.decode()}" if contents else "")
            + " >>"
        ).encode()
        kids.append(wtr.add(page))
    wtr.set(
        pages_id,
        (
            f"<< /Type /Pages /Count {len(kids)} /Kids ["
            + " ".join(f"{k} 0 R" for k in kids)
            + "] >>"
        ).encode(),
    )
    wtr.set(catalog, f"<< /Type /Catalog /Pages {pages_id} 0 R >>".encode())
    return wtr.build(
        root=catalog, xref=xref, objstm=objstm, w=w, corrupt_xref=corrupt_xref
    )


def build_with_tounicode(
    touni_bytes: bytes, font_body: bytes, content: bytes, wmode_x: int = 700
) -> bytes:
    w = PdfWriter()
    tu = w.stream("", touni_bytes)
    tuid = w.add(tu)
    fid = w.add(font_body.replace(b"TOUNI", str(tuid).encode()))
    cat = w.reserve()
    tree = w.reserve()
    cont = w.add(w.stream("", content))
    page = w.add(
        f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] "
        f"/Resources << /Font << /F1 {fid} 0 R >> >> /Contents {cont} 0 R >>".encode()
    )
    w.set(tree, f"<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>".encode())
    w.set(cat, f"<< /Type /Catalog /Pages {tree} 0 R >>".encode())
    return w.build(cat)


FILES: dict[str, bytes] = {}
EXPECT: dict[str, str] = {}


def emit(name: str, pdf: bytes, expect: str | None):
    FILES[name + ".pdf"] = pdf
    if expect is not None:
        EXPECT[name + ".txt"] = expect


def content_stream(body: bytes, compress: bool = False) -> bytes:
    return PdfWriter().stream("", body, compress=compress)


HELV = font_simple("Type1", "Helvetica", "/WinAnsiEncoding", None)

# 1 ---------------------------------------------------------------- winansi
pages = [
    {
        "fonts": {"F1": HELV},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td " + lit("Hello world!") + b" Tj "
            b"0 -14 Td " + lit("\u201csmart\u201d quotes \u2014 dash") + b" Tj "
            b"0 -14 Td " + lit("caf\u00e9 \u00fcber alles") + b" Tj "
            b"0 -14 Td " + lit("\u20ac5.00 ok?") + b" Tj ET"
        )],
    },
    {
        "fonts": {"F1": HELV},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td " + lit("Second page") + b" Tj "
            b"0 -14 Td " + lit("a(paren) and \\ backslash") + b" Tj ET"
        )],
    },
]
emit(
    "winansi_basic",
    build_doc(pages),
    "Hello world!\n\u201csmart\u201d quotes \u2014 dash\ncaf\u00e9 \u00fcber alles\n\u20ac5.00 ok?\x0c"
    "Second page\na(paren) and \\ backslash",
)

# 2 ---------------------------------------------------------- TJ + escapes
pages = [
    {
        "fonts": {"F1": HELV},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td "
            b"[(K) -60 (erne) 40 (d) -300 (spaced) -300 (words)] TJ "
            # apostrophe op: newline + show (string/hex precedes the operator)
            b"<48657820676f6f64627965> ' "
            b"1 0.5 " + lit("quoted op") + b" \" "
            b"T* " + lit("after star") + b" Tj ET"
        )],
    },
]
emit(
    "winansi_tj_kern",
    build_doc(pages),
    "Kerned spaced words\nHex goodbye\nquoted op\nafter star",
)

# 3 -------------------------------------------------------------- macroman
# 8E=e' 9F=u" BD=Omega B0=infinity C4=florin
mac_txt = bytes([0x8E, 0x9F, 0x62, 0x65, 0x72, 0x20, 0xBD, 0xB0, 0xC4])
pages = [
    {
        "fonts": {"F1": font_simple("Type1", "Helvetica", "/MacRomanEncoding", None)},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td " + lit_bytes(mac_txt) + b" Tj ET"
        )],
    },
]
emit("macroman", build_doc(pages), "\u00e9\u00fcber \u03a9\u221e\u0192")

# 4 --------------------------------------------------------- standard enc
pages = [
    {
        "fonts": {"F1": font_simple("Type1", "Times-Roman", "/StandardEncoding", None)},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td " + lit_bytes(b"sh \xae\xad pa\xaf ne")
            + b" Tj 0 -14 Td " + lit_bytes(b"quote\x27s \x60open\x60")
            + b" Tj ET"
        )],
    },
]
# std: ae=fi, ad=guilsinglright, af=fl, 27=quoteright, 60=quoteleft
emit("standardenc", build_doc(pages), "sh \ufb01\u203a pa\ufb02 ne\nquote\u2019s \u2018open\u2018")

# 5 ---------------------------------------------------------------- symbol
pages = [
    {
        "fonts": {"F1": font_simple("Type1", "Symbol", None, None)},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td " + lit_bytes(b"abgD W l") + b" Tj ET"
        )],
    },
]
emit("symbol", build_doc(pages), "\u03b1\u03b2\u03b3\u0394 \u03a9 \u03bb")

# 6 ---------------------------------------------------------- zapfdingbats
pages = [
    {
        "fonts": {"F1": font_simple("Type1", "ZapfDingbats", None, None)},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td " + lit_bytes(bytes([0x61, 0xA1, 0x33, 0xD5]))
            + b" Tj ET"
        )],
    },
]
emit("zapfdingbats", build_doc(pages), "\u2741\u2761\u2713\u2192")

# 7 ---------------------------------------- default encoding (no /Encoding)
pages = [
    {
        "fonts": {"F1": font_simple("Type1", "Times-Roman", None, None)},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td " + lit_bytes(b"na\xefve na") + b" Tj ET"
        )],
    },
]
# std 0xEF is unmapped (.notdef) -> U+FFFD
emit("standard_default", build_doc(pages), "na\ufffdve na")

# 8 ----------------------------------------------------------- differences
pages = [
    {
        "fonts": {"F1": (
            b"<< /Type /Font /Subtype /Type1 /BaseFont /MyOdd "
            b"/Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding "
            b"/Differences [144 /fi 145 /fl 146 /Scommaaccent 158 /Euro] >> >>"
        )},
        "contents": [content_stream(
            b"BT /F1 12 Tf 72 720 Td " + lit_bytes(b"a\x90 b\x91 c\x92 \x9e")
            + b" Tj ET"
        )],
    },
]
emit("differences", build_doc(pages), "a\ufb01 b\ufb02 c\u0218 \u20ac")

# 9 ---------------------------------------------------- tounicode bfchar
emit(
    "tounicode_bfchar",
    build_with_tounicode(
        to_unicode_cmap(
            [(b"\x41", utf16("A")), (b"\x42", utf16("\u0e01")),
             (b"\x43", b"\xd8\x40\xdc\x0b")],  # surrogate pair -> U+2000B
            []),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /WingX /Encoding "
        b"<< /Type /Encoding /BaseEncoding /WinAnsiEncoding "
        b"/Differences [65 /gA 66 /gB 67 /gC] >> /ToUnicode TOUNI 0 R >>",
        b"BT /F1 12 Tf 72 720 Td (ABC) Tj ET"),
    "A\u0e01\U0002000b",
)

# 10 --------------------------------------------------- tounicode bfrange
emit(
    "tounicode_bfrange",
    build_with_tounicode(
        to_unicode_cmap(
            [],
            [
                (b"\x61", b"\x63", utf16("x")),  # single start, BMP
                # array dsts incl. astral
                (b"\x64", b"\x66", [utf16("\u00e9"), utf16("\u4e2d"), utf16("\U0001f600")]),
                (b"\x67", b"\x67", utf16("HI")),  # one code -> two chars
            ]),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Weird /Encoding "
        b"<< /Type /Encoding /BaseEncoding /WinAnsiEncoding "
        b"/Differences [97 /w1 98 /w2 99 /w3 100 /w4 101 /w5 102 /w6 103 /w7] >> "
        b"/ToUnicode TOUNI 0 R >>",
        b"BT /F1 12 Tf 72 720 Td <61626364656667> Tj ET"),
    "xyz\u00e9\u4e2d\U0001f600HI",
)

# 11 ----------------------------------------- tounicode surrogate bfrange
emit(
    "tounicode_surrogate",
    build_with_tounicode(
        to_unicode_cmap(
            [],
            [(b"\xb0", b"\xb3", b"\xd8\x40\xdc\x0b")]),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Astral /Encoding "
        b"<< /Type /Encoding /BaseEncoding /WinAnsiEncoding "
        b"/Differences [176 /s0 177 /s1 178 /s2 179 /s3] >> "
        b"/ToUnicode TOUNI 0 R >>",
        b"BT /F1 12 Tf 72 720 Td <b0b1b2b3> Tj ET"),
    "\U0002000b\U0002000c\U0002000d\U0002000e",
)


def build_cid_doc(enc: "str | int", touni: bytes, content: bytes) -> bytes:
    w = PdfWriter()
    tuid = w.add(w.stream("", touni))
    desc = w.add(
        b"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /ABCDEE+CidF "
        b"/CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) "
        b"/Supplement 0 >> /DW 1000 >>")
    enc_part = (
        f"/Encoding /{enc}" if isinstance(enc, str) else f"/Encoding {enc} 0 R"
    )
    fid = w.add(
        f"<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEE+CidF {enc_part} "
        f"/DescendantFonts [{desc} 0 R] /ToUnicode {tuid} 0 R >>".encode())
    cat = w.reserve()
    tree = w.reserve()
    cont = w.add(w.stream("", content))
    page = w.add(
        f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] "
        f"/Resources << /Font << /F1 {fid} 0 R >> >> /Contents {cont} 0 R >>".encode())
    w.set(tree, f"<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>".encode())
    w.set(cat, f"<< /Type /Catalog /Pages {tree} 0 R >>".encode())
    return w.build(cat)


# 12 ----------------------------------------------------- cid Identity-H
emit(
    "cid_identity",
    build_cid_doc(
        "Identity-H",
        to_unicode_cmap(
            [(b"\x00\x41", utf16("A")), (b"\x00\x42", utf16("\u4e16")),
             (b"\x00\x43", utf16("\U0001f642"))],
            [],
            codespaces=[(b"\x00\x00", b"\xff\xff")]),
        b"BT /F1 12 Tf 72 720 Td <004100420043> Tj ET"),
    "A\u4e16\U0001f642",
)

# 13 --------------------------------------- cid with embedded CMap stream
_w13 = PdfWriter()
cmap13 = to_unicode_cmap(
    [],
    [],
    codespaces=[(b"\x00", b"\x1f"), (b"\x00\x20", b"\xff\xff")],
    wmode=1,
)
emit(
    "cid_cmapstream",
    build_cid_doc(
        _w13.add(_w13.stream(" /Type /CMap", cmap13, compress=False)),
        to_unicode_cmap(
            [(b"\x00\x41", utf16("\u3042")), (b"\x00\x42", utf16("\u3044")),
             (b"\x00\x43", utf16("\u3046"))],
            [],
            codespaces=[(b"\x00\x00", b"\xff\xff")]),
        # vertical writing: T* moves sideways; ' = T* + Tj
        b"BT /F1 12 Tf 500 700 Td <0041> Tj T* <0042> Tj <0043> ' ET"),
    "\u3042\n\u3044\n\u3046",
)

# 14 ------------------------------------------------------------ xrefstream
emit(
    "xrefstream",
    build_doc(
        [
            {"fonts": {"F1": HELV},
             "contents": [content_stream(b"BT /F1 12 Tf 72 720 Td (stream xref one) Tj ET")]},
            {"fonts": {"F1": HELV},
             "contents": [content_stream(b"BT /F1 12 Tf 72 720 Td (stream xref two) Tj ET")]},
        ],
        xref="stream"),
    "stream xref one\x0cstream xref two",
)

# 15 ------------------------------------------------ xref stream, odd /W
emit(
    "xrefstream_w",
    build_doc(
        [{"fonts": {"F1": HELV},
          "contents": [content_stream(b"BT /F1 12 Tf 72 720 Td (wide fields) Tj ET")]}],
        xref="stream", w=(1, 5, 0)),
    "wide fields",
)

# 16 -------------------------------------------- xref stream + object stm
def build_objstm_doc() -> bytes:
    w = PdfWriter()
    cat = w.reserve()
    tree = w.reserve()
    fid = w.add(HELV)
    cont1 = w.add(w.stream("", b"BT /F1 12 Tf 72 720 Td (packed one) Tj ET"))
    cont2 = w.add(w.stream("", b"BT /F1 12 Tf 72 720 Td (packed two) Tj ET"))
    p1 = w.add(b"PAGE1")
    p2 = w.add(b"PAGE2")
    w.set(p1, f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] "
              f"/Resources << /Font << /F1 {fid} 0 R >> >> /Contents {cont1} 0 R >>".encode())
    w.set(p2, f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] "
              f"/Resources << /Font << /F1 {fid} 0 R >> >> /Contents {cont2} 0 R >>".encode())
    w.set(tree, f"<< /Type /Pages /Count 2 /Kids [{p1} 0 R {p2} 0 R] >>".encode())
    w.set(cat, f"<< /Type /Catalog /Pages {tree} 0 R >>".encode())
    # page dicts, font and pages-tree live inside the object stream
    return w.build(cat, xref="stream", objstm={fid, p1, p2, tree})

emit("objstm", build_objstm_doc(), "packed one\x0cpacked two")

# 17 ------------------------------------------------------ flate content
emit(
    "flate_content",
    build_doc(
        [{"fonts": {"F1": HELV},
          "contents": [content_stream(
              b"BT /F1 12 Tf 72 720 Td (compressed content) Tj 0 -14 Td "
              b"(second line) Tj ET", compress=True)]}]),
    "compressed content\nsecond line",
)

# 18 ------------------------------------------------------ contents array
pages = [
    {"fonts": {"F1": HELV},
     "contents": [
         content_stream(b"BT /F1 12 Tf 72 720 Td (part one) Tj ET"),
         content_stream(b"BT /F1 12 Tf 72 700 Td (part two) Tj ET"),
         content_stream(b"BT /F1 12 Tf 72 680 Td (part three) Tj ET"),
     ]},
]
emit("contents_array", build_doc(pages), "part one\npart two\npart three")

# 19 --------------------------------------------------------- form xobject
def build_form_doc() -> bytes:
    w = PdfWriter()
    fid = w.add(HELV)
    form = w.add(w.stream(
        " /Type /XObject /Subtype /Form /BBox [0 0 200 50] "
        f"/Resources << /Font << /F1 {fid} 0 R >> >>",
        b"BT /F1 12 Tf 10 10 Td (inside the form) Tj ET"))
    cat = w.reserve()
    tree = w.reserve()
    cont = w.add(w.stream(
        "",
        b"BT /F1 12 Tf 72 720 Td (before form) Tj ET "
        b"q 1 0 0 1 72 650 cm /Fm1 Do Q "
        b"BT /F1 12 Tf 72 600 Td (after form) Tj ET"))
    page = w.add(
        f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] "
        f"/Resources << /Font << /F1 {fid} 0 R >> "
        f"/XObject << /Fm1 {form} 0 R >> >> /Contents {cont} 0 R >>".encode())
    w.set(tree, f"<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>".encode())
    w.set(cat, f"<< /Type /Catalog /Pages {tree} 0 R >>".encode())
    return w.build(cat)

emit("formxobject", build_form_doc(), "before form\ninside the form\nafter form")

# 20 --------------------------------------------------- marked content etc
pages = [
    {"fonts": {"F1": HELV},
     "contents": [content_stream(
         b"BT /P <</MCID 0>> BDC /F1 12 Tf 2 Tc 1.5 Tw 72 720 Td "
         b"(marked) Tj EMC 0 -14 Td <54696d65> Tj "
         b"0 -14 Td (two) Tj ( halves) Tj ET")]},
]
emit("markcontent", build_doc(pages), "marked\nTime\ntwo halves")

# 21 ------------------------------------------------------------ empty page
emit(
    "empty_page",
    build_doc(
        [{"fonts": {"F1": HELV},
          "contents": [content_stream(b"BT /F1 12 Tf 72 720 Td (only page one) Tj ET")]},
         {"fonts": {}, "contents": []}]),
    "only page one\x0c",
)

# 22 ------------------------------------------------- incremental update
def build_incremental() -> bytes:
    w = PdfWriter()
    cat = w.reserve()
    tree = w.reserve()
    fid = w.add(HELV)
    cont1 = w.add(w.stream("", b"BT /F1 12 Tf 72 720 Td (old text) Tj ET"))
    page1 = w.add(
        f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] "
        f"/Resources << /Font << /F1 {fid} 0 R >> >> /Contents {cont1} 0 R >>".encode())
    w.set(tree, f"<< /Type /Pages /Count 1 /Kids [{page1} 0 R] >>".encode())
    w.set(cat, f"<< /Type /Catalog /Pages {tree} 0 R >>".encode())
    base = w.build(cat)
    new_body = w.stream("", b"BT /F1 12 Tf 72 720 Td (revised text) Tj ET")
    inc = bytearray(base)
    inc += f"{cont1} 0 obj\n".encode() + new_body + b"\nendobj\n"
    xoff = len(inc)
    sx = base.rfind(b"startxref")
    prev_off = int(base[sx + 9:base.find(b"%%EOF", sx)].strip())
    inc += f"xref\n{cont1} 1\n".encode() + b"%010d 00000 n \n" % (xoff - len(f"{cont1} 0 obj\n".encode()) - len(new_body) - len(b"\nendobj\n"))
    inc += (b"trailer\n<< /Size %d /Root %d 0 R /Prev %d >>\n"
            % (max(w.objs) + 1, cat, prev_off))
    inc += b"startxref\n%d\n%%%%EOF\n" % xoff
    return bytes(inc)

emit("prev_incremental", build_incremental(), "revised text")

# 23 ------------------------------------------------------------- encrypted
crypt = StandardCrypt(md5(b"modhash-pdf fixture"))
emit(
    "encrypted",
    build_doc(
        [{"fonts": {"F1": HELV},
          "contents": [content_stream(b"BT /F1 12 Tf 72 720 Td (secret text) Tj ET")]}],
        crypt=crypt),
    None,
)

# 24 -------------------------------------------------- corrupt but scanable
emit(
    "corrupt_xref_recoverable",
    build_doc(
        [{"fonts": {"F1": HELV},
          "contents": [content_stream(b"BT /F1 12 Tf 72 720 Td (recovered) Tj ET")]}],
        corrupt_xref=True),
    "recovered",
)

# 25 --------------------------------------------------------------- type3
def build_type3() -> bytes:
    w = PdfWriter()
    cp_a = w.add(w.stream("", b"500 0 0 0 500 700 d1 100 0 300 700 re f"))
    charprocs = w.add(f"<< /a {cp_a} 0 R >>".encode())
    font3 = w.add(
        f"<< /Type /Font /Subtype /Type3 /Name /T3F /FontBBox [0 0 500 700] "
        f"/FontMatrix [0.001 0 0 0.001 0 0] /CharProcs {charprocs} 0 R "
        f"/Encoding << /Type /Encoding /Differences [97 /a] >> "
        f"/FirstChar 97 /LastChar 97 /Widths [500] /Resources << >> >>".encode())
    cat = w.reserve()
    tree = w.reserve()
    cont = w.add(w.stream("", b"BT /T3F 12 Tf 72 720 Td (aa a) Tj ET"))
    page = w.add(
        f"<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] "
        f"/Resources << /Font << /T3F {font3} 0 R >> >> /Contents {cont} 0 R >>".encode())
    w.set(tree, f"<< /Type /Pages /Count 1 /Kids [{page} 0 R] >>".encode())
    w.set(cat, f"<< /Type /Catalog /Pages {tree} 0 R >>".encode())
    return w.build(cat)

emit("type3", build_type3(), "aa a")


# ---------------------------------------------------------------------------

def main() -> None:
    FIX.mkdir(parents=True, exist_ok=True)
    rows = []
    for name, pdf in sorted(FILES.items()):
        (FIX / name).write_bytes(pdf)
        rows.append((name, len(pdf), hashlib.sha256(pdf).hexdigest()[:16]))
    for name, txt in sorted(EXPECT.items()):
        raw = txt.encode("utf8")
        (FIX / name).write_bytes(raw)
        rows.append((name, len(raw), hashlib.sha256(raw).hexdigest()[:16]))
    for name, size, sha in rows:
        print("%-34s %7d %s" % (name, size, sha))
    print(f"{len(FILES)} pdfs, {len(EXPECT)} expected texts")

    # -- oracle check on the encrypted fixture ------------------------------
    try:
        import io

        import pypdf

        r = pypdf.PdfReader(io.BytesIO(FILES["encrypted.pdf"]))
        assert r.is_encrypted
        r.decrypt("")
        got = r.pages[0].extract_text()
        print("pypdf decrypted encrypted fixture ->", repr(got))
    except Exception as e:  # pragma: no cover
        print("pypdf oracle skipped:", e)


if __name__ == "__main__":
    main()
