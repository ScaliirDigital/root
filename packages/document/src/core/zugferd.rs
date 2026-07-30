//! `ZUGFeRD` / Factur-X metadata.
//!
//! Typst already embeds the XML attachment and can produce valid PDF/A-3b,
//! but it cannot currently emit the Factur-X XMP extension properties required
//! by the standard.
//!
//! This module patches the existing XMP metadata through a PDF incremental
//! update. The original PDF bytes remain untouched.
//!
//! Re-serialising the complete document was deliberately avoided because doing
//! so can alter structures relevant to PDF/A conformance.
//!
//! The Factur-X schema is inserted into Typst's existing
//! `pdfaExtension:schemas` bag. Creating a second property with the same name
//! would make the XMP packet invalid.
//!
//! <https://github.com/typst/typst/issues/5667>

use crate::core::profile::Profile;
use lopdf::{Document, ObjectId};
use std::{fmt::Write, sync::LazyLock};

static FACTUR_X_SCHEMA: LazyLock<String> = LazyLock::new(schema_entry);

const DOCUMENT_TYPE: &str = "INVOICE";
const ATTACHMENT_FILENAME: &str = "factur-x.xml";
const DATA_MODEL_VERSION: &str = "1.0";

const RDF_END: &str = "</rdf:RDF>";
const EXTENSION_SCHEMA_END: &str = "</rdf:Bag></pdfaExtension:schemas>";
const FACTUR_X_NAMESPACE: &str = "urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#";

// TODO: Move Factur-X/XMP standard-specific definitions into a dedicated,
// versioned module once additional Factur-X versions/profiles are supported.
struct SchemaProperty {
    name: &'static str,
    description: &'static str,
}

const FACTUR_X_PROPERTIES: &[SchemaProperty] = &[
    SchemaProperty {
        name: "DocumentType",
        description: "INVOICE, ORDER or ORDER_RESPONSE",
    },
    SchemaProperty {
        name: "DocumentFileName",
        description: "name of the embedded XML document",
    },
    SchemaProperty {
        name: "Version",
        description: "version of the Factur-X data model",
    },
    SchemaProperty {
        name: "ConformanceLevel",
        description: "conformance level of the embedded data",
    },
];

fn schema_entry() -> String {
    let mut xml = format!(
        concat!(
            r#"<rdf:li rdf:parseType="Resource">"#,
            r#"<pdfaSchema:schema>Factur-X PDFA Extension Schema</pdfaSchema:schema>"#,
            r#"<pdfaSchema:namespaceURI>{namespace}</pdfaSchema:namespaceURI>"#,
            r#"<pdfaSchema:prefix>fx</pdfaSchema:prefix>"#,
            r#"<pdfaSchema:property><rdf:Seq>"#
        ),
        namespace = FACTUR_X_NAMESPACE,
    );

    for property in FACTUR_X_PROPERTIES {
        write!(
            xml,
            concat!(
                r#"<rdf:li rdf:parseType="Resource">"#,
                r#"<pdfaProperty:name>{name}</pdfaProperty:name>"#,
                r#"<pdfaProperty:valueType>Text</pdfaProperty:valueType>"#,
                r#"<pdfaProperty:category>external</pdfaProperty:category>"#,
                r#"<pdfaProperty:description>{description}</pdfaProperty:description>"#,
                r#"</rdf:li>"#
            ),
            name = property.name,
            description = property.description,
        )
        .expect("writing to String cannot fail");
    }

    xml.push_str(r"</rdf:Seq></pdfaSchema:property></rdf:li>");
    xml
}

#[derive(Clone, Debug)]
pub struct Zugferd {
    pub profile: Profile,
}

impl Zugferd {
    /// The attachment is always a Factur-X invoice at data model version 1.0 --
    /// only the profile varies, and nothing downstream can infer it from the
    /// PDF, so it has to be stated.
    #[must_use]
    pub const fn new(profile: Profile) -> Self {
        Self { profile }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ZugferdError {
    #[error("the rendered pdf could not be read")]
    Unreadable(#[from] lopdf::Error),

    #[error("the rendered pdf carries no xmp metadata stream")]
    MissingPacket,

    #[error("the xmp metadata is not the plain xml pdf/a requires")]
    MalformedPacket,

    #[error("the rendered pdf has no pdf/a extension schema")]
    MissingExtensionSchema,

    #[error("the rendered pdf trailer is malformed")]
    MalformedTrailer,
}

struct Metadata {
    object_id: ObjectId,
    packet: String,
}

/// Adds the Factur-X/ZUGFeRD XMP properties to an already rendered PDF/A-3
/// document without modifying its original bytes.
///
/// The existing metadata object is superseded through an incremental update.
///
/// # Errors
///
/// Returns an error if the PDF cannot be parsed, contains no suitable XMP
/// metadata stream, or its existing PDF/A metadata structure cannot be patched.
pub fn add_metadata(pdf: &[u8], zugferd: &Zugferd) -> Result<Vec<u8>, ZugferdError> {
    let document = Document::load_mem(pdf)?;
    let metadata = load_metadata(&document)?;

    let packet = patch_xmp(metadata.packet, zugferd)?;

    append_incremental_update(pdf, document.xref_start, metadata.object_id, &packet)
}

fn load_metadata(document: &Document) -> Result<Metadata, ZugferdError> {
    let catalog_id = document.trailer.get(b"Root")?.as_reference()?;

    let metadata_id = document
        .get_dictionary(catalog_id)?
        .get(b"Metadata")
        .map_err(|_| ZugferdError::MissingPacket)?
        .as_reference()?;

    let packet = document
        .get_object(metadata_id)?
        .as_stream()?
        .content
        .clone();

    let packet = String::from_utf8(packet).map_err(|_| ZugferdError::MalformedPacket)?;

    Ok(Metadata {
        object_id: metadata_id,
        packet,
    })
}

fn patch_xmp(mut packet: String, zugferd: &Zugferd) -> Result<String, ZugferdError> {
    insert_properties(&mut packet, zugferd)?;
    insert_extension_schema(&mut packet)?;

    Ok(packet)
}

fn insert_properties(packet: &mut String, zugferd: &Zugferd) -> Result<(), ZugferdError> {
    let offset = packet.rfind(RDF_END).ok_or(ZugferdError::MalformedPacket)?;

    packet.insert_str(offset, &properties(zugferd));

    Ok(())
}

fn insert_extension_schema(packet: &mut String) -> Result<(), ZugferdError> {
    let offset = packet
        .find(EXTENSION_SCHEMA_END)
        .ok_or(ZugferdError::MissingExtensionSchema)?;

    packet.insert_str(offset, &FACTUR_X_SCHEMA);

    Ok(())
}

fn append_incremental_update(
    original: &[u8],
    previous_xref: usize,
    metadata_id: ObjectId,
    packet: &str,
) -> Result<Vec<u8>, ZugferdError> {
    let trailer = incremental_trailer(original, previous_xref)?;
    let (number, generation) = metadata_id;

    let mut output = Vec::with_capacity(original.len() + packet.len() + 1024);
    output.extend_from_slice(original);

    if !output.ends_with(b"\n") {
        output.push(b'\n');
    }

    let object_offset = output.len();

    let object = metadata_object(number, generation, packet);
    output.extend_from_slice(object.as_bytes());

    let xref_offset = output.len();

    let revision = incremental_revision(number, generation, object_offset, xref_offset, &trailer);

    output.extend_from_slice(revision.as_bytes());

    Ok(output)
}

/// Serializes a replacement XMP metadata indirect object.
///
/// The object reuses the existing metadata object number and generation so the
/// newest incremental PDF revision supersedes the previous metadata stream.
fn metadata_object(number: u32, generation: u16, packet: &str) -> String {
    format!(
        "{number} {generation} obj\n\
         <</Type/Metadata/Subtype/XML/Length {}>>\n\
         stream\n\
         {packet}\n\
         endstream\n\
         endobj\n",
        packet.len(),
    )
}

/// Serializes the cross-reference table and trailer for the incremental revision.
///
/// The xref entry points to the replacement metadata object, while `startxref`
/// points to this revision's xref table. The trailer links to the previous
/// revision through `/Prev`.
fn incremental_revision(
    number: u32,
    generation: u16,
    object_offset: usize,
    xref_offset: usize,
    trailer: &str,
) -> String {
    format!(
        "xref\n\
         0 1\n\
         0000000000 65535 f \n\
         {number} 1\n\
         {object_offset:010} {generation:05} n \n\
         trailer\n\
         {trailer}\n\
         startxref\n\
         {xref_offset}\n\
         %%EOF\n"
    )
}

/// Reuses the previous trailer dictionary and links the new revision to the
/// preceding cross-reference section through `/Prev`.
fn incremental_trailer(pdf: &[u8], previous_xref: usize) -> Result<String, ZugferdError> {
    let trailer_at = rfind_bytes(pdf, b"trailer").ok_or(ZugferdError::MalformedTrailer)?;

    let startxref_at =
        find_bytes(pdf, b"startxref", trailer_at).ok_or(ZugferdError::MalformedTrailer)?;

    let trailer = std::str::from_utf8(&pdf[trailer_at + b"trailer".len()..startxref_at])
        .map_err(|_| ZugferdError::MalformedTrailer)?
        .trim();

    let closing = trailer.rfind(">>").ok_or(ZugferdError::MalformedTrailer)?;

    Ok(format!("{}/Prev {previous_xref}>>", &trailer[..closing]))
}

fn properties(zugferd: &Zugferd) -> String {
    format!(
        concat!(
            r#"<rdf:Description rdf:about="" xmlns:fx="{namespace}">"#,
            r#"<fx:DocumentType>{document_type}</fx:DocumentType>"#,
            r#"<fx:DocumentFileName>{filename}</fx:DocumentFileName>"#,
            r#"<fx:Version>{version}</fx:Version>"#,
            r#"<fx:ConformanceLevel>{conformance}</fx:ConformanceLevel>"#,
            r#"</rdf:Description>"#
        ),
        namespace = FACTUR_X_NAMESPACE,
        document_type = DOCUMENT_TYPE,
        filename = ATTACHMENT_FILENAME,
        version = DATA_MODEL_VERSION,
        conformance = zugferd.profile.conformance_level(),
    )
}

fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    use lopdf::{Dictionary, Object};

    /// The smallest packet that still has both anchors the patcher looks for.
    const PACKET: &str = concat!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">"#,
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#,
        r#"<rdf:Description rdf:about="">"#,
        r#"<pdfaExtension:schemas><rdf:Bag></rdf:Bag></pdfaExtension:schemas>"#,
        r#"</rdf:Description>"#,
        r#"</rdf:RDF></x:xmpmeta>"#
    );

    fn zugferd() -> Zugferd {
        Zugferd::new(Profile::En16931)
    }

    fn serialize(document: &mut Document) -> Vec<u8> {
        let mut buffer = Vec::new();
        document.save_to(&mut buffer).expect("serialize pdf");
        buffer
    }

    /// Asserting on the message rather than the variant keeps what the caller
    /// actually sees under test.
    fn message(pdf: &[u8]) -> String {
        add_metadata(pdf, &zugferd())
            .expect_err("patching must fail")
            .to_string()
    }

    fn trailer_error(pdf: &[u8]) -> String {
        incremental_trailer(pdf, 0)
            .expect_err("malformed trailer")
            .to_string()
    }

    /// A minimal PDF with a classic cross-reference table, because lopdf's own
    /// serializer emits an xref stream and the patcher needs `trailer`.
    fn pdf_with_packet(packet: &[u8]) -> Vec<u8> {
        let mut stream = format!(
            "<</Type/Metadata/Subtype/XML/Length {}>>\nstream\n",
            packet.len()
        )
        .into_bytes();

        stream.extend_from_slice(packet);
        stream.extend_from_slice(b"\nendstream");

        let objects: Vec<Vec<u8>> = vec![
            b"<</Type/Catalog/Pages 2 0 R/Metadata 3 0 R>>".to_vec(),
            b"<</Type/Pages/Kids[]/Count 0>>".to_vec(),
            stream,
        ];

        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();

        for (index, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());

            pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            pdf.extend_from_slice(body);
            pdf.extend_from_slice(b"\nendobj\n");
        }

        let xref_offset = pdf.len();
        let size = objects.len() + 1;

        pdf.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
        pdf.extend_from_slice(b"0000000000 65535 f \n");

        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }

        pdf.extend_from_slice(
            format!("trailer\n<</Size {size}/Root 1 0 R>>\nstartxref\n{xref_offset}\n%%EOF\n")
                .as_bytes(),
        );

        pdf
    }

    /// A document lopdf serializes itself, for the cases that fail before the
    /// trailer is ever read.
    fn pdf_with_catalog(catalog: Object) -> Vec<u8> {
        let mut document = Document::with_version("1.7");

        let catalog_id = document.add_object(catalog);
        document.trailer.set("Root", Object::Reference(catalog_id));

        serialize(&mut document)
    }

    // -----------------------------------------------------------------------
    // Happy path
    // -----------------------------------------------------------------------

    /// The original bytes have to survive byte for byte -- that is the whole
    /// reason for the incremental update.
    #[test]
    fn appends_without_touching_the_original() {
        let pdf = pdf_with_packet(PACKET.as_bytes());

        let patched = add_metadata(&pdf, &zugferd()).expect("patch metadata");

        assert!(patched.starts_with(&pdf));
        assert!(patched.len() > pdf.len());

        let patched = String::from_utf8_lossy(&patched);

        assert!(patched.contains("<fx:DocumentType>INVOICE</fx:DocumentType>"));
        assert!(patched.contains("Factur-X PDFA Extension Schema"));
        assert!(patched.contains("/Prev"));
        assert!(patched.ends_with("%%EOF\n"));
    }

    /// A document that already ends in a newline must not gain a second one,
    /// or every offset in the appended xref is off by a byte.
    #[test]
    fn normalizes_the_boundary_newline() {
        let mut pdf = pdf_with_packet(PACKET.as_bytes());

        assert!(pdf.ends_with(b"\n"), "fixture must end in a newline");

        let with = add_metadata(&pdf, &zugferd()).expect("patch metadata");

        pdf.pop();

        let without = add_metadata(&pdf, &zugferd()).expect("patch metadata");

        assert_eq!(with.len(), without.len());
    }

    // -----------------------------------------------------------------------
    // Document structure
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_an_unreadable_pdf() {
        assert_eq!(message(b"not a pdf"), "the rendered pdf could not be read");
    }

    #[test]
    fn rejects_a_document_without_a_catalog() {
        let mut document = Document::with_version("1.7");

        let pdf = serialize(&mut document);

        assert_eq!(message(&pdf), "the rendered pdf could not be read");
    }

    #[test]
    fn rejects_a_catalog_that_is_not_a_reference() {
        let mut document = Document::with_version("1.7");
        document.trailer.set("Root", Object::Integer(1));

        let pdf = serialize(&mut document);

        assert_eq!(message(&pdf), "the rendered pdf could not be read");
    }

    #[test]
    fn rejects_a_catalog_that_is_not_a_dictionary() {
        let pdf = pdf_with_catalog(Object::Integer(1));

        assert_eq!(message(&pdf), "the rendered pdf could not be read");
    }

    #[test]
    fn rejects_a_document_without_metadata() {
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));

        let pdf = pdf_with_catalog(Object::Dictionary(catalog));

        assert_eq!(
            message(&pdf),
            "the rendered pdf carries no xmp metadata stream"
        );
    }

    #[test]
    fn rejects_metadata_that_is_not_a_stream() {
        let mut document = Document::with_version("1.7");

        let metadata = document.add_object(Object::Integer(1));

        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Metadata", Object::Reference(metadata));

        let catalog_id = document.add_object(catalog);
        document.trailer.set("Root", Object::Reference(catalog_id));

        let pdf = serialize(&mut document);

        assert_eq!(message(&pdf), "the rendered pdf could not be read");
    }

    // -----------------------------------------------------------------------
    // Packet contents
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_a_packet_that_is_not_utf8() {
        let pdf = pdf_with_packet(&[0xff, 0xfe, 0xfd]);

        assert_eq!(
            message(&pdf),
            "the xmp metadata is not the plain xml pdf/a requires"
        );
    }

    #[test]
    fn rejects_a_packet_without_rdf() {
        let pdf = pdf_with_packet(b"<x:xmpmeta></x:xmpmeta>");

        assert_eq!(
            message(&pdf),
            "the xmp metadata is not the plain xml pdf/a requires"
        );
    }

    #[test]
    fn rejects_a_packet_without_an_extension_schema() {
        let pdf = pdf_with_packet(b"<rdf:RDF></rdf:RDF>");

        assert_eq!(
            message(&pdf),
            "the rendered pdf has no pdf/a extension schema"
        );
    }

    // -----------------------------------------------------------------------
    // Trailer
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_a_document_without_a_trailer() {
        assert_eq!(
            trailer_error(b"%PDF-1.7\n%%EOF\n"),
            "the rendered pdf trailer is malformed"
        );
    }

    #[test]
    fn rejects_a_trailer_without_startxref() {
        assert_eq!(
            trailer_error(b"trailer<</Root 1 0 R>>\n"),
            "the rendered pdf trailer is malformed"
        );
    }

    #[test]
    fn rejects_a_trailer_that_is_not_utf8() {
        assert_eq!(
            trailer_error(b"trailer\xff\xfe\nstartxref\n0\n"),
            "the rendered pdf trailer is malformed"
        );
    }

    #[test]
    fn rejects_a_trailer_without_a_closing_dictionary() {
        assert_eq!(
            trailer_error(b"trailer<</Root 1 0 R\nstartxref\n0\n"),
            "the rendered pdf trailer is malformed"
        );
    }

    #[test]
    fn links_the_previous_revision() {
        let trailer =
            incremental_trailer(b"trailer<</Root 1 0 R>>\nstartxref\n42\n", 42).expect("trailer");

        assert!(trailer.starts_with("<</Root 1 0 R"));
        assert!(trailer.ends_with("/Prev 42>>"));
    }

    // -----------------------------------------------------------------------
    // Byte search
    // -----------------------------------------------------------------------

    #[test]
    fn finds_a_needle_after_an_offset() {
        assert_eq!(find_bytes(b"aXbXc", b"X", 2), Some(3));
    }

    #[test]
    fn finds_nothing_past_the_end() {
        assert_eq!(find_bytes(b"abc", b"X", 99), None);
    }

    #[test]
    fn finds_no_needle_going_forward() {
        assert_eq!(find_bytes(b"abc", b"X", 0), None);
    }

    #[test]
    fn finds_no_needle_going_backward() {
        assert_eq!(rfind_bytes(b"abc", b"X"), None);
    }

    #[test]
    fn finds_the_last_needle() {
        assert_eq!(rfind_bytes(b"aXbXc", b"X"), Some(3));
    }

    // -----------------------------------------------------------------------
    // Serialization
    // -----------------------------------------------------------------------

    /// Every property the standard requires has to be declared, or veraPDF
    /// reports them all as missing.
    #[test]
    fn declares_every_required_property() {
        let schema = schema_entry();

        for property in FACTUR_X_PROPERTIES {
            assert!(schema.contains(property.name));
            assert!(schema.contains(property.description));
        }
    }

    #[test]
    fn states_the_conformance_level_of_the_profile() {
        for profile in [Profile::Minimum, Profile::Basic, Profile::En16931] {
            let properties = properties(&Zugferd::new(profile));

            assert!(properties.contains(&format!(
                "<fx:ConformanceLevel>{}</fx:ConformanceLevel>",
                profile.conformance_level()
            )));
        }
    }

    #[test]
    fn reuses_the_metadata_object_slot() {
        let object = metadata_object(7, 0, "packet");

        assert!(object.starts_with("7 0 obj\n"));
        assert!(object.contains("/Length 6"));
    }

    #[test]
    fn points_the_xref_at_the_replacement_object() {
        let revision = incremental_revision(7, 0, 1234, 5678, "<</Root 1 0 R>>");

        assert!(revision.contains("7 1\n0000001234 00000 n \n"));
        assert!(revision.contains("startxref\n5678\n"));
    }

    #[test]
    fn rejects_metadata_that_is_not_a_reference() {
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Metadata", Object::Integer(1));

        let pdf = pdf_with_catalog(Object::Dictionary(catalog));

        assert_eq!(message(&pdf), "the rendered pdf could not be read");
    }

    #[test]
    fn rejects_metadata_pointing_at_a_missing_object() {
        let mut catalog = Dictionary::new();
        catalog.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog.set("Metadata", Object::Reference((999, 0)));

        let pdf = pdf_with_catalog(Object::Dictionary(catalog));

        assert_eq!(message(&pdf), "the rendered pdf could not be read");
    }

    /// `add_metadata` cannot reach this: lopdf rejects a document whose trailer
    /// is unreadable long before the update is assembled. The guard stays
    /// because `append_incremental_update` does not know that.
    #[test]
    fn rejects_an_update_without_a_usable_trailer() {
        let result = append_incremental_update(b"%PDF-1.7\n%%EOF\n", 0, (3, 0), "packet");

        assert_eq!(
            result.expect_err("malformed trailer").to_string(),
            "the rendered pdf trailer is malformed"
        );
    }
}
