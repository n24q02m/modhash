//! Tests for the minimal XML reader: elements, attributes, decoded
//! entities and numeric character references, comments and processing
//! instructions, CDATA, and the refusal paths (mismatched end tags,
//! unknown entities, text outside the root, a second root element).

use modhash_primitives::Error;
use modhash_zip::{XmlEvent, XmlReader};

/// Collects the whole event stream, failing the test on the first error.
fn events(input: &str) -> Vec<XmlEvent<'_>> {
    XmlReader::new(input)
        .collect::<Result<Vec<_>, _>>()
        .expect("well-formed input must not error")
}

/// The first error a reader produces, for the refusal tests.
fn first_error(input: &str) -> Error {
    let mut reader = XmlReader::new(input);
    loop {
        match reader.next() {
            Some(Err(e)) => return e,
            Some(Ok(_)) => {}
            None => panic!("input {input:?} produced no error"),
        }
    }
}

/// Nested elements with attributes and text arrive in document order,
/// attributes keep their order, and the reader terminates cleanly.
#[test]
fn nested_elements_and_attributes() {
    let doc = "<w:document xmlns:w=\"http://x\"><w:body><w:p w:id=\"42\"><w:t>hi</w:t></w:p></w:body></w:document>";
    let ev = events(doc);
    assert_eq!(
        ev,
        vec![
            XmlEvent::Start {
                name: "w:document",
                attrs: vec![("xmlns:w", "http://x".to_string())],
            },
            XmlEvent::Start {
                name: "w:body",
                attrs: vec![],
            },
            XmlEvent::Start {
                name: "w:p",
                attrs: vec![("w:id", "42".to_string())],
            },
            XmlEvent::Start {
                name: "w:t",
                attrs: vec![],
            },
            XmlEvent::Text("hi".to_string()),
            XmlEvent::End { name: "w:t" },
            XmlEvent::End { name: "w:p" },
            XmlEvent::End { name: "w:body" },
            XmlEvent::End { name: "w:document" },
        ]
    );
}

/// All five predefined entities plus decimal and hex character
/// references decode in text and in attribute values.
#[test]
fn entities_and_character_references_decode() {
    let ev = events("<a b=\"&quot;x&apos;&#65;&#x42;\">1 &amp; 2 &lt;3&gt; &#8364;&#x20AC;</a>");
    assert_eq!(
        ev[0],
        XmlEvent::Start {
            name: "a",
            attrs: vec![("b", "\"x'AB".to_string())],
        }
    );
    assert_eq!(ev[1], XmlEvent::Text("1 & 2 <3> €€".to_string()));
    assert_eq!(ev[2], XmlEvent::End { name: "a" });
}

/// Comments, processing instructions and the XML declaration produce no
/// events; CDATA arrives as raw text; self-closing elements emit a Start
/// then an End.
#[test]
fn comments_pi_cdata_and_self_closing() {
    let doc = "<?xml version=\"1.0\"?><!-- a -- b --><?pi <not>?><r><e/><![CDATA[<raw> &]]></r>";
    assert_eq!(
        events(doc),
        vec![
            XmlEvent::Start {
                name: "r",
                attrs: vec![],
            },
            XmlEvent::Start {
                name: "e",
                attrs: vec![],
            },
            XmlEvent::End { name: "e" },
            XmlEvent::Text("<raw> &".to_string()),
            XmlEvent::End { name: "r" },
        ]
    );
}

/// A `<!DOCTYPE` with an internal subset containing `>` inside `[…]` is
/// skipped whole, not terminated at the first bracket.
#[test]
fn doctype_with_internal_subset_is_skipped() {
    let doc = "<!DOCTYPE r [ <!ELEMENT r (#PCDATA)> <!ATTLIST r a CDATA \"x>\"> ]><r>ok</r>";
    assert_eq!(
        events(doc),
        vec![
            XmlEvent::Start {
                name: "r",
                attrs: vec![],
            },
            XmlEvent::Text("ok".to_string()),
            XmlEvent::End { name: "r" },
        ]
    );
}

/// A mismatched end tag is a named error.
#[test]
fn mismatched_end_tag_is_rejected() {
    assert_eq!(
        first_error("<a><b></a></b>"),
        Error::BadValue("xml end tag mismatch")
    );
}

/// End of input inside an open element is a truncation, not a hang.
#[test]
fn unclosed_element_is_truncated() {
    assert!(matches!(first_error("<a><b>"), Error::Truncated { .. }));
    assert!(matches!(first_error("<a"), Error::Truncated { .. }));
    assert!(matches!(first_error("<a></a"), Error::Truncated { .. }));
}

/// Unknown entities and malformed references are refused, never
/// silently passed through.
#[test]
fn bad_entities_are_rejected() {
    assert_eq!(
        first_error("<a>&nbsp;</a>"),
        Error::BadValue("xml unknown entity")
    );
    assert_eq!(
        first_error("<a>&amp</a>"),
        Error::BadValue("xml unterminated entity")
    );
    assert_eq!(
        first_error("<a>&#xD800;</a>"),
        Error::BadValue("xml numeric character reference")
    );
}

/// Text outside the root element and a second root element are both
/// malformed documents.
#[test]
fn document_shape_is_enforced() {
    assert_eq!(
        first_error("<a/>tail"),
        Error::BadValue("xml text outside root element")
    );
    assert_eq!(
        first_error("<a/><b/>"),
        Error::BadValue("xml second root element")
    );
    assert_eq!(
        first_error("<a x=\"1\" x=\"2\"/>"),
        Error::BadValue("xml duplicate attribute")
    );
}

/// After an error the reader stays finished: events up to the error
/// still arrive, then one `Err`, then `None` forever.
#[test]
fn errors_are_terminal() {
    let mut reader = XmlReader::new("<a></b>");
    assert!(matches!(reader.next(), Some(Ok(XmlEvent::Start { .. }))));
    assert!(matches!(reader.next(), Some(Err(_))));
    assert!(reader.next().is_none());
    assert!(reader.next().is_none());
}

/// Whitespace between the declaration and the root is not a text event.
#[test]
fn prolog_and_epilog_whitespace_is_skipped() {
    assert_eq!(
        events("  <?xml version=\"1.0\"?>\n<a/>\n  "),
        vec![
            XmlEvent::Start {
                name: "a",
                attrs: vec![],
            },
            XmlEvent::End { name: "a" },
        ]
    );
}
