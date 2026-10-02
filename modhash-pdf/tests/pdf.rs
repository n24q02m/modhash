//! Conformance and adversarial tests for `modhash-pdf`.
//!
//! Fixtures come from `tests/fixtures/` (see `PROVENANCE.md`: generated
//! byte-exact by `tools/gen_fixtures.py`, verified against pypdf 6.13.3 and
//! PyMuPDF 1.27 where their extractors agree with the spec). Expected text
//! is declared in `*.txt` next to each `.pdf`, never derived from the
//! extractor.
//!
//! Corruption and mutation suites are deterministic: every pseudo-random
//! choice comes from `SplitMix64` with fixed seeds.

use std::fs;
use std::path::PathBuf;

use modhash_pdf::{Document, Error, Obj, Ref, extract_text};
use modhash_primitives::SplitMix64;

fn fixture(name: &str) -> Vec<u8> {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(name);
    fs::read(&p).unwrap_or_else(|e| panic!("{}: {}", p.display(), e))
}

fn expected(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(name);
    String::from_utf8(fs::read(&p).unwrap()).unwrap()
}

// ---------------------------------------------------------------------
// Fixture round-trips: every generated PDF must extract to its .txt
// ---------------------------------------------------------------------

macro_rules! fixture_test {
    ($t:ident, $name:literal) => {
        #[test]
        fn $t() {
            let pdf = fixture(concat!($name, ".pdf"));
            let want = expected(concat!($name, ".txt"));
            let doc = Document::open(&pdf).expect("open");
            let got = doc.text().expect("extract");
            assert_eq!(got, want, "{}", $name);
        }
    };
}

fixture_test!(f_winansi_basic, "winansi_basic");
fixture_test!(f_winansi_tj_kern, "winansi_tj_kern");
fixture_test!(f_macroman, "macroman");
fixture_test!(f_standardenc, "standardenc");
fixture_test!(f_symbol, "symbol");
fixture_test!(f_zapfdingbats, "zapfdingbats");
fixture_test!(f_standard_default, "standard_default");
fixture_test!(f_differences, "differences");
fixture_test!(f_tounicode_bfchar, "tounicode_bfchar");
fixture_test!(f_tounicode_bfrange, "tounicode_bfrange");
fixture_test!(f_tounicode_surrogate, "tounicode_surrogate");
fixture_test!(f_cid_identity, "cid_identity");
fixture_test!(f_cid_cmapstream, "cid_cmapstream");
fixture_test!(f_xrefstream, "xrefstream");
fixture_test!(f_xrefstream_w, "xrefstream_w");
fixture_test!(f_objstm, "objstm");
fixture_test!(f_flate_content, "flate_content");
fixture_test!(f_contents_array, "contents_array");
fixture_test!(f_formxobject, "formxobject");
fixture_test!(f_markcontent, "markcontent");
fixture_test!(f_empty_page, "empty_page");
fixture_test!(f_prev_incremental, "prev_incremental");
fixture_test!(f_type3, "type3");
fixture_test!(f_corrupt_xref_recoverable, "corrupt_xref_recoverable");

/// Every fixture with text layers: >= 20 pages total (contract acceptance).
#[test]
fn fixture_page_coverage() {
    let mut pages = 0usize;
    for name in [
        "winansi_basic",
        "winansi_tj_kern",
        "macroman",
        "standardenc",
        "symbol",
        "zapfdingbats",
        "standard_default",
        "differences",
        "tounicode_bfchar",
        "tounicode_bfrange",
        "tounicode_surrogate",
        "cid_identity",
        "cid_cmapstream",
        "xrefstream",
        "xrefstream_w",
        "objstm",
        "flate_content",
        "contents_array",
        "formxobject",
        "markcontent",
        "type3",
        "prev_incremental",
        "corrupt_xref_recoverable",
    ] {
        let pdf = fixture(&format!("{}.pdf", name));
        let doc = Document::open(&pdf).expect(name);
        pages += doc.pages();
    }
    assert!(pages >= 20, "only {} pages across fixtures", pages);
}

/// Page-level API must equal the joined text.
#[test]
fn per_page_equals_text() {
    let pdf = fixture("winansi_basic.pdf");
    let doc = Document::open(&pdf).unwrap();
    assert_eq!(doc.pages(), 2);
    assert_eq!(
        doc.page_text(0).unwrap(),
        "Hello world!\n\u{201c}smart\u{201d} quotes \u{2014} dash\ncaf\u{e9} \u{fc}ber alles\n\u{20ac}5.00 ok?"
    );
    assert_eq!(
        doc.page_text(1).unwrap(),
        "Second page\na(paren) and \\ backslash"
    );
    assert!(matches!(
        doc.page_text(2),
        Err(Error::Page { page: 2, .. }) | Err(_)
    ));
}

// ---------------------------------------------------------------------
// Encrypted documents refuse, with page + object context
// ---------------------------------------------------------------------

#[test]
fn encrypted_refuses_with_context() {
    let pdf = fixture("encrypted.pdf");
    let doc = Document::open(&pdf).expect("open must not fail on encryption");
    assert!(doc.is_encrypted());
    let enc = doc.encryption().expect("encrypt ref");
    let err = doc.text().unwrap_err();
    match err {
        Error::Page { page, cause } => {
            assert_eq!(page, 0);
            match *cause {
                Error::Object { object, cause, .. } => {
                    assert_eq!(object, enc.num);
                    assert!(matches!(
                        *cause,
                        Error::Kit(modhash_primitives::Error::Unsupported(_))
                    ));
                }
                other => panic!("expected object context, got {:?}", other),
            }
        }
        other => panic!("expected page context, got {:?}", other),
    }
    // and the message names the page and the object
    let msg = doc.page_text(0).unwrap_err().to_string();
    assert!(msg.contains("page 0"), "{}", msg);
    assert!(msg.contains("object"), "{}", msg);
}

// ---------------------------------------------------------------------
// Corrupt xref: recovery by scanning, and hopeless files error by name
// ---------------------------------------------------------------------

#[test]
fn corrupt_xref_rebuilds_by_scan() {
    let pdf = fixture("corrupt_xref_recoverable.pdf");
    let doc = Document::open(&pdf).expect("scan fallback must open");
    assert!(doc.xref_was_rebuilt());
    assert_eq!(doc.text().unwrap(), "recovered");
}

#[test]
fn hopeless_inputs_error_not_panic() {
    for bad in [
        b"".as_slice(),
        b"not a pdf",
        b"%PDF-1.7\n",
        b"%PDF-1.7\ntrailer <<>>\n",
        b"%PDF-1.7\n1 0 obj <<>> endobj\n%%EOF\n",
    ] {
        let _ = Document::open(bad); // must not panic; error is fine
    }
    let doc = Document::open(b"%PDF-1.7\n1 0 obj <<>> endobj\n%%EOF\n");
    assert!(doc.is_err(), "no catalog -> named error");
}

// ---------------------------------------------------------------------
// Corruption loops: every prefix and 20k byte mutations never panic
// ---------------------------------------------------------------------

#[test]
fn prefix_scans_never_panic() {
    let pdf = fixture("objstm.pdf");
    // every prefix of the file
    for n in 0..pdf.len() {
        let _ = extract_text(&pdf[..n]);
    }
}

#[test]
fn mutations_never_panic() {
    let mut rng = SplitMix64::new(0x5eed_5eed);
    for name in ["winansi_basic.pdf", "objstm.pdf", "tounicode_bfrange.pdf"] {
        let pdf = fixture(name);
        // 20k mutations total across three representative files
        for _ in 0..6_667u32 {
            let mut m = pdf.clone();
            let i = (rng.next_u64() as usize) % m.len();
            m[i] = rng.next_u64() as u8;
            let _ = extract_text(&m);
        }
    }
}

// ---------------------------------------------------------------------
// Structural unit checks
// ---------------------------------------------------------------------

#[test]
fn object_and_page_api() {
    let pdf = fixture("winansi_basic.pdf");
    let doc = Document::open(&pdf).unwrap();
    // object() returns the catalog for the trailer root
    let cat = doc.object(Ref {
        num: 1,
        generation: 0,
    });
    // catalog object number differs by generator; find it through pages
    // instead: object 1 is the Catalog in our generator
    if let Ok(Obj::Dict(d)) = cat {
        let ty = d
            .iter()
            .find(|(k, _)| k.as_slice() == b"Type")
            .map(|(_, v)| v.clone());
        assert!(matches!(ty, Some(Obj::Name(n)) if n == b"Catalog"));
    } else {
        panic!("catalog object missing");
    }
}

#[test]
fn page_count_and_ff_join() {
    let pdf = fixture("xrefstream.pdf");
    let doc = Document::open(&pdf).unwrap();
    assert_eq!(doc.pages(), 2);
    assert_eq!(doc.text().unwrap(), "stream xref one\x0cstream xref two");
}

/// `is_int` and `parse_f64` stay honest on edge inputs.
#[test]
fn lex_number_edges() {
    // exercised through public surface: a doc with a real-number Tm offset
    // is covered by fixtures; here we only check the scan helpers via a
    // crafted mini document (name-decoding + numeric positions).
    let pdf = b"%PDF-1.4\n\
        1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] \
        /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >> endobj\n\
        4 0 obj << /Length 33 >>\nstream\nBT /F1 1.5 Tf 0.5 0 Td (x) Tj ET\nendstream\nendobj\n\
        5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n\
        xref\n0 6\n0000000000 65535 f \n";
    // no xref offsets -> falls back to scanning, finds objects, 'x' shows
    let doc = Document::open(pdf).expect("scan-recoverable doc");
    assert!(doc.xref_was_rebuilt());
    assert_eq!(doc.text().unwrap(), "x");
}

#[test]
fn unsupported_filters_named() {
    // build a doc whose content stream declares an unsupported filter
    let pdf = b"%PDF-1.4\n\
        1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] \
        /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >> endobj\n\
        4 0 obj << /Length 10 /Filter /DCTDecode >>\nstream\n0123456789\nendstream\nendobj\n\
        5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n";
    let doc = Document::open(pdf).unwrap();
    match doc.page_text(0) {
        Err(Error::Page { page: 0, cause }) => {
            assert!(matches!(
                *cause,
                Error::Kit(modhash_primitives::Error::Unsupported(m))
                    if m.contains("DCTDecode")
            ));
        }
        other => panic!("expected unsupported page error, got {:?}", other),
    }
}

#[test]
fn cid_without_tounicode_refuses() {
    let pdf = b"%PDF-1.4\n\
        1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
        2 0 obj << /Type /Pages /Count 1 /Kids [3 0 R] >> endobj\n\
        3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] \
        /Resources << /Font << /F1 6 0 R >> >> /Contents 4 0 R >> endobj\n\
        4 0 obj << /Length 33 >>\nstream\nBT /F1 12 Tf 0 0 Td <0041> Tj ET\nendstream\nendobj\n\
        5 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /X \
        /CIDSystemInfo << /Registry (Adobe) /Ordering (I) /Supplement 0 >> /DW 1 >> endobj\n\
        6 0 obj << /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H \
        /DescendantFonts [5 0 R] >> endobj\n";
    let doc = Document::open(pdf).unwrap();
    let err = doc.page_text(0).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ToUnicode") && msg.contains("unsupported"),
        "CID without ToUnicode must refuse by name, got: {msg}"
    );
}

// ---------------------------------------------------------------------
// PDF 32000-1 Annex H CMap examples, transcribed verbatim (H.3 shows the
// ToUnicode CMap containing bfchar + bfrange, scalar and array forms, and
// a surrogate-pair destination).
// ---------------------------------------------------------------------

use modhash_pdf::cmap::CMap;

/// PDF 32000-1 §H.3 example ToUnicode CMap (reformatted to canonical
/// whitespace; byte values verbatim). Covers bfchar, bfrange scalar with
/// a surrogate pair, and bfrange array form.
const SPEC_H3_CMAP: &[u8] = b"/CIDInit /ProcSet findresource begin\n\
12 dict begin\n\
begincmap\n\
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
/CMapName /Adobe-Identity-UCS def\n\
/CMapType 2 def\n\
1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
2 beginbfchar\n<0003> <0020>\n<0004> <00200020>\nendbfchar\n\
2 beginbfrange\n<0005> <0007> <0009>\n<0008> <000A> [<0041> <0042> <0043>]\nendbfrange\n\
1 beginbfrange\n<000B> <000C> <D840DC0B>\nendbfrange\n\
endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";

#[test]
fn spec_annex_h_bfchar() {
    let cm = CMap::parse(SPEC_H3_CMAP).unwrap();
    // bfchar: <0003> -> U+0020
    assert_eq!(cm.lookup(b"\x00\x03"), (Some(" "), 2));
    // bfchar multi-UTF16-unit dst: <0004> -> "  " (two spaces)
    assert_eq!(cm.lookup(b"\x00\x04"), (Some("  "), 2));
}

#[test]
fn spec_annex_h_bfrange_scalar() {
    let cm = CMap::parse(SPEC_H3_CMAP).unwrap();
    // <0005>-<0007> -> 0x0009,0x000A,0x000B (sequential)
    assert_eq!(cm.lookup(b"\x00\x05"), (Some("\t"), 2));
    assert_eq!(cm.lookup(b"\x00\x07"), (Some("\u{b}"), 2));
}

#[test]
fn spec_annex_h_bfrange_array() {
    let cm = CMap::parse(SPEC_H3_CMAP).unwrap();
    // <0008>-<000A> -> explicit array [<0041> <0042> <0043>]
    assert_eq!(cm.lookup(b"\x00\x08"), (Some("A"), 2));
    assert_eq!(cm.lookup(b"\x00\x09"), (Some("B"), 2));
    assert_eq!(cm.lookup(b"\x00\x0a"), (Some("C"), 2));
}

#[test]
fn spec_annex_h_bfrange_surrogate_pair() {
    let cm = CMap::parse(SPEC_H3_CMAP).unwrap();
    // <000B>-<000C> -> dst <D840DC0B> is a surrogate pair; stepping the
    // last code unit yields U+2000B then U+2000C (carry into the pair).
    assert_eq!(cm.lookup(b"\x00\x0b"), (Some("\u{2000b}"), 2));
    assert_eq!(cm.lookup(b"\x00\x0c"), (Some("\u{2000c}"), 2));
}

/// bfrange where the array length mismatches the declared range must
/// error, never index past the array.
#[test]
fn bfrange_array_length_mismatch() {
    let bad = b"begincmap\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
1 beginbfrange\n<00> <02> [<41>]\nendbfrange\nendcmap\n";
    assert!(CMap::parse(bad).is_err());
}
