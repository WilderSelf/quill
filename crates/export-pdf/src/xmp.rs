//! Minimal XMP metadata packet carrying the PDF/X identification schema (X-1a:2001 / X-3:2002),
//! or no identification at all for the screen profile (spec 0052).
//!
//! `pdf-writer` has no XMP builder, so the packet bytes are constructed here. The PDF/X
//! identification requires `pdfxid:GTS_PDFXVersion` under the NPES namespace
//! (`http://www.npes.org/pdfx/ns/id/`); the packet is left uncompressed so the identification
//! block stays locatable as plain text — which is also what lets CI assert on its *absence*.

use crate::PdfxVersion;

/// Escape the five XML predefined entities so a title/author can't corrupt the packet.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Build the XMP packet for an export at the given conformance `version`, or for one that claims
/// **no** PDF/X conformance when `version` is `None` (the screen profile, spec 0052).
///
/// The `GTS_PDFXVersion` string tracks `version`; the `GTS_PDFXConformance` line is emitted only
/// when the level defines one (X-1a does; X-3:2002 does not — see [`PdfxVersion::conformance`]).
/// With `None`, both lines *and* the two PDF/X namespace declarations that exist only to carry them
/// are omitted: a packet that still declared the NPES identification namespace while claiming
/// nothing under it would read as a half-made claim.
///
/// `doc_id` / `instance_id` are hex strings used for `xmpMM:DocumentID` / `InstanceID`; keeping
/// them deterministic (derived from document content) makes the exported bytes reproducible for
/// golden-file tests.
pub fn build_xmp(
    version: Option<PdfxVersion>,
    title: &str,
    doc_id: &str,
    instance_id: &str,
) -> Vec<u8> {
    let title = xml_escape(title);
    let bom = '\u{FEFF}';
    let pdfx_namespaces = match version {
        Some(_) => {
            "\n    xmlns:pdfx=\"http://ns.adobe.com/pdfx/1.3/\"\
             \n    xmlns:pdfxid=\"http://www.npes.org/pdfx/ns/id/\""
        }
        None => "",
    };
    let identification = match version {
        Some(v) => {
            let conformance_line = match v.conformance() {
                Some(conf) => {
                    format!("\n   <pdfx:GTS_PDFXConformance>{conf}</pdfx:GTS_PDFXConformance>")
                }
                None => String::new(),
            };
            format!(
                "\n   <pdfxid:GTS_PDFXVersion>{}</pdfxid:GTS_PDFXVersion>{conformance_line}",
                v.identifier()
            )
        }
        None => String::new(),
    };
    format!(
        r#"<?xpacket begin="{bom}" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:pdf="http://ns.adobe.com/pdf/1.3/"{pdfx_namespaces}>{identification}
   <pdf:Trapped>False</pdf:Trapped>
   <pdf:Producer>Quill export-pdf</pdf:Producer>
   <xmpMM:DocumentID>uuid:{doc_id}</xmpMM:DocumentID>
   <xmpMM:InstanceID>uuid:{instance_id}</xmpMM:InstanceID>
   <dc:title><rdf:Alt><rdf:li xml:lang="x-default">{title}</rdf:li></rdf:Alt></dc:title>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_special_characters_in_title() {
        let xmp = build_xmp(
            Some(PdfxVersion::X1a2001),
            "Tom & Jerry <the> \"Adventure\"",
            "aa",
            "bb",
        );
        let s = String::from_utf8(xmp).unwrap();
        assert!(s.contains("Tom &amp; Jerry &lt;the&gt; &quot;Adventure&quot;"));
        // The raw ampersand must never appear unescaped inside the packet body.
        assert!(!s.contains("Tom & Jerry"));
    }

    #[test]
    fn x1a_carries_version_and_conformance() {
        let s = String::from_utf8(build_xmp(Some(PdfxVersion::X1a2001), "T", "d", "i")).unwrap();
        assert!(s.contains("http://www.npes.org/pdfx/ns/id/"));
        assert!(s.contains("<pdfxid:GTS_PDFXVersion>PDF/X-1a:2001</pdfxid:GTS_PDFXVersion>"));
        assert!(s.contains("<pdfx:GTS_PDFXConformance>PDF/X-1a:2001</pdfx:GTS_PDFXConformance>"));
        assert!(s.contains("uuid:d"));
        assert!(s.contains("uuid:i"));
    }

    #[test]
    fn x3_carries_version_and_omits_conformance() {
        let s = String::from_utf8(build_xmp(Some(PdfxVersion::X3_2002), "T", "d", "i")).unwrap();
        assert!(s.contains("<pdfxid:GTS_PDFXVersion>PDF/X-3:2002</pdfxid:GTS_PDFXVersion>"));
        // X-3:2002 defines only GTS_PDFXVersion — no conformance key, and no stray X-1a string.
        assert!(!s.contains("GTS_PDFXConformance"));
        assert!(!s.contains("PDF/X-1a"));
    }

    #[test]
    fn a_screen_packet_claims_no_conformance_and_still_carries_its_metadata() {
        // Spec 0052. The absence has to be total: no version, no conformance, and not even the
        // namespaces that exist only to carry them — a packet declaring the NPES identification
        // namespace while asserting nothing under it reads as a half-made claim. The rest of the
        // packet is unchanged, so the screen file is still identifiable and still has a title.
        let s = String::from_utf8(build_xmp(None, "Screen Book", "d", "i")).unwrap();
        assert!(!s.contains("GTS_PDFXVersion"), "no PDF/X version: {s}");
        assert!(!s.contains("GTS_PDFXConformance"), "no conformance: {s}");
        assert!(!s.contains("npes.org"), "no identification namespace: {s}");
        assert!(!s.contains("pdfx"), "no PDF/X namespace at all: {s}");
        // Still a well-formed packet with everything that is not a conformance claim.
        assert!(s.contains("<pdf:Producer>Quill export-pdf</pdf:Producer>"));
        assert!(s.contains("uuid:d") && s.contains("uuid:i"));
        assert!(s.contains("Screen Book"));
        assert!(s.contains("</x:xmpmeta>"));
    }
}
