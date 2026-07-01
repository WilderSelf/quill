//! Minimal XMP metadata packet carrying the PDF/X-1a:2001 identification schema.
//!
//! `pdf-writer` has no XMP builder, so the packet bytes are constructed here. veraPDF's 1a
//! profile enforces the presence of `pdfxid:GTS_PDFXVersion` under the NPES namespace
//! (`http://www.npes.org/pdfx/ns/id/`); the packet is left uncompressed so the identification
//! block stays locatable as plain text.

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

/// Build the XMP packet for a PDF/X-1a:2001 document.
///
/// `doc_id` / `instance_id` are hex strings used for `xmpMM:DocumentID` / `InstanceID`; keeping
/// them deterministic (derived from document content) makes the exported bytes reproducible for
/// golden-file tests.
pub fn build_xmp(title: &str, doc_id: &str, instance_id: &str) -> Vec<u8> {
    let title = xml_escape(title);
    let bom = '\u{FEFF}';
    format!(
        r#"<?xpacket begin="{bom}" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:pdf="http://ns.adobe.com/pdf/1.3/"
    xmlns:pdfx="http://ns.adobe.com/pdfx/1.3/"
    xmlns:pdfxid="http://www.npes.org/pdfx/ns/id/">
   <pdfxid:GTS_PDFXVersion>PDF/X-1a:2001</pdfxid:GTS_PDFXVersion>
   <pdfx:GTS_PDFXConformance>PDF/X-1a:2001</pdfx:GTS_PDFXConformance>
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
        let xmp = build_xmp("Tom & Jerry <the> \"Adventure\"", "aa", "bb");
        let s = String::from_utf8(xmp).unwrap();
        assert!(s.contains("Tom &amp; Jerry &lt;the&gt; &quot;Adventure&quot;"));
        // The raw ampersand must never appear unescaped inside the packet body.
        assert!(!s.contains("Tom & Jerry"));
    }

    #[test]
    fn carries_pdfx_identification() {
        let s = String::from_utf8(build_xmp("T", "d", "i")).unwrap();
        assert!(s.contains("http://www.npes.org/pdfx/ns/id/"));
        assert!(s.contains("<pdfxid:GTS_PDFXVersion>PDF/X-1a:2001</pdfxid:GTS_PDFXVersion>"));
        assert!(s.contains("uuid:d"));
        assert!(s.contains("uuid:i"));
    }
}
