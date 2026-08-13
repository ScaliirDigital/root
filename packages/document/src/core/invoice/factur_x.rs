//! Factur-X XML generation.
//!
//! Serializes an [`Invoice`] into UN/CEFACT Cross Industry Invoice (CII) XML,
//! the format `ZUGFeRD` and Factur-X both carry. Written by hand rather than
//! through a serializer: CII is namespace-heavy, element order is fixed by the
//! schema, and which elements appear at all depends on the profile. A derive
//! would need more annotations than this file has lines.
//!
//! Element names follow the standard exactly. They are not readable, and
//! renaming them is not an option -- a validator matches on them literally.

use std::fmt::Write;

use rust_decimal::Decimal;

use super::{Invoice, Issuer, Line, Party, TaxBreakdown};
use crate::core::profile::Profile;

const RSM: &str = "urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100";
const RAM: &str =
    "urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100";
const UDT: &str = "urn:un:unece:uncefact:data:standard:UnqualifiedDataType:100";

/// Date format qualifier 102: `YYYYMMDD`.
const DATE_FORMAT: &str = "102";

/// Serializes an invoice as Factur-X CII XML.
///
/// The invoice is expected to have passed [`Invoice::validate`] for this
/// profile already -- this function writes what it is given.
#[must_use]
pub fn generate(issuer: &Issuer, invoice: &Invoice, profile: Profile) -> String {
    let mut xml = String::with_capacity(4096);

    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);

    write!(
        xml,
        concat!(
            r#"<rsm:CrossIndustryInvoice"#,
            r#" xmlns:rsm="{rsm}""#,
            r#" xmlns:ram="{ram}""#,
            r#" xmlns:udt="{udt}">"#
        ),
        rsm = RSM,
        ram = RAM,
        udt = UDT,
    )
    .expect("writing to String cannot fail");

    context(&mut xml, profile);
    header(&mut xml, invoice, profile);
    supply_chain_transaction(&mut xml, issuer, invoice, profile);

    xml.push_str("</rsm:CrossIndustryInvoice>");
    xml
}

/// Declares which profile the document claims to follow.
///
/// This is what a validator reads first: it decides which rule set the rest of
/// the document is checked against.
fn context(xml: &mut String, profile: Profile) {
    let _ = write!(
        xml,
        concat!(
            "<rsm:ExchangedDocumentContext>",
            "<ram:GuidelineSpecifiedDocumentContextParameter>",
            "<ram:ID>{urn}</ram:ID>",
            "</ram:GuidelineSpecifiedDocumentContextParameter>",
            "</rsm:ExchangedDocumentContext>"
        ),
        urn = profile.urn(),
    );
}

/// Invoice number, type and issue date.
fn header(xml: &mut String, invoice: &Invoice, profile: Profile) {
    let _ = write!(
        xml,
        concat!(
            "<rsm:ExchangedDocument>",
            "<ram:ID>{number}</ram:ID>",
            "<ram:TypeCode>{kind}</ram:TypeCode>",
            "<ram:IssueDateTime><udt:DateTimeString format=\"{format}\">{issued}\
             </udt:DateTimeString></ram:IssueDateTime>"
        ),
        number = escape(&invoice.number),
        kind = escape(&invoice.kind),
        format = DATE_FORMAT,
        issued = compact_date(&invoice.issued),
    );

    // MINIMUM has no room for free text: it forbids what it does not require.
    if profile >= Profile::Basic
        && let Some(note) = &invoice.note
    {
        let _ = write!(
            xml,
            "<ram:IncludedNote><ram:Content>{}</ram:Content></ram:IncludedNote>",
            escape(note)
        );
    }

    xml.push_str("</rsm:ExchangedDocument>");
}

/// The three sections that carry the actual invoice: lines, parties, totals.
///
/// Order is fixed by the schema. MINIMUM omits the lines entirely, which is
/// exactly why it is not a valid invoice.
fn supply_chain_transaction(
    xml: &mut String,
    issuer: &Issuer,
    invoice: &Invoice,
    profile: Profile,
) {
    xml.push_str("<rsm:SupplyChainTradeTransaction>");

    if profile >= Profile::Basic {
        for (index, line) in invoice.lines.iter().enumerate() {
            line_item(xml, index, line, &invoice.currency);
        }
    }

    agreement(xml, issuer, invoice, profile);
    delivery(xml, invoice, profile);
    settlement(xml, issuer, invoice, profile);

    xml.push_str("</rsm:SupplyChainTradeTransaction>");
}

fn line_item(xml: &mut String, index: usize, line: &Line, currency: &str) {
    let id = line.id.clone().unwrap_or_else(|| (index + 1).to_string());

    let _ = write!(
        xml,
        concat!(
            "<ram:IncludedSupplyChainTradeLineItem>",
            "<ram:AssociatedDocumentLineDocument><ram:LineID>{id}</ram:LineID>",
            "</ram:AssociatedDocumentLineDocument>",
            "<ram:SpecifiedTradeProduct><ram:Name>{name}</ram:Name>"
        ),
        id = escape(&id),
        name = escape(&line.name),
    );

    if let Some(description) = &line.description {
        let _ = write!(
            xml,
            "<ram:Description>{}</ram:Description>",
            escape(description)
        );
    }

    let _ = write!(
        xml,
        concat!(
            "</ram:SpecifiedTradeProduct>",
            "<ram:SpecifiedLineTradeAgreement>",
            "<ram:NetPriceProductTradePrice><ram:ChargeAmount>{price}</ram:ChargeAmount>",
            "</ram:NetPriceProductTradePrice>",
            "</ram:SpecifiedLineTradeAgreement>",
            "<ram:SpecifiedLineTradeDelivery>",
            "<ram:BilledQuantity unitCode=\"{unit}\">{quantity}</ram:BilledQuantity>",
            "</ram:SpecifiedLineTradeDelivery>",
            "<ram:SpecifiedLineTradeSettlement>",
            "<ram:ApplicableTradeTax>",
            "<ram:TypeCode>VAT</ram:TypeCode>",
            "<ram:CategoryCode>{category}</ram:CategoryCode>",
            "<ram:RateApplicablePercent>{rate}</ram:RateApplicablePercent>",
            "</ram:ApplicableTradeTax>",
            "<ram:SpecifiedTradeSettlementLineMonetarySummation>",
            "<ram:LineTotalAmount>{net}</ram:LineTotalAmount>",
            "</ram:SpecifiedTradeSettlementLineMonetarySummation>",
            "</ram:SpecifiedLineTradeSettlement>",
            "</ram:IncludedSupplyChainTradeLineItem>"
        ),
        price = amount(line.unit_price),
        unit = escape(&line.unit),
        quantity = quantity(line.quantity),
        category = escape(&line.tax.category),
        rate = amount(line.tax.rate),
        net = amount(line.net),
    );

    // `currency` is carried by the document totals, not per line.
    let _ = currency;
}

/// How much of a party the profile lets us state.
///
/// MINIMUM is not a reduced EN 16931: it forbids what it does not require, so
/// a full address on the buyer makes the document invalid rather than verbose.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PartyDetail {
    /// Name only -- the buyer under MINIMUM.
    NameOnly,
    /// Name, country and tax registration -- the seller under MINIMUM.
    Identified,
    /// Full postal address and tax registration.
    Full,
}

fn agreement(xml: &mut String, issuer: &Issuer, invoice: &Invoice, profile: Profile) {
    let (seller, buyer) = if profile >= Profile::Basic {
        (PartyDetail::Full, PartyDetail::Full)
    } else {
        (PartyDetail::Identified, PartyDetail::NameOnly)
    };

    xml.push_str("<ram:ApplicableHeaderTradeAgreement>");
    party(xml, "SellerTradeParty", &issuer.seller, seller);
    party(xml, "BuyerTradeParty", &invoice.buyer, buyer);
    xml.push_str("</ram:ApplicableHeaderTradeAgreement>");
}

fn party(xml: &mut String, element: &str, party: &Party, detail: PartyDetail) {
    let _ = write!(
        xml,
        "<ram:{element}><ram:Name>{name}</ram:Name>",
        name = escape(&party.name),
    );

    match detail {
        PartyDetail::NameOnly => {}
        PartyDetail::Identified => {
            let _ = write!(
                xml,
                concat!(
                    "<ram:PostalTradeAddress>",
                    "<ram:CountryID>{country}</ram:CountryID>",
                    "</ram:PostalTradeAddress>"
                ),
                country = escape(&party.address.country),
            );
        }
        PartyDetail::Full => {
            let _ = write!(
                xml,
                concat!(
                    "<ram:PostalTradeAddress>",
                    "<ram:PostcodeCode>{postcode}</ram:PostcodeCode>",
                    "<ram:LineOne>{street}</ram:LineOne>",
                    "<ram:CityName>{city}</ram:CityName>",
                    "<ram:CountryID>{country}</ram:CountryID>",
                    "</ram:PostalTradeAddress>"
                ),
                postcode = escape(&party.address.postcode),
                street = escape(&party.address.street),
                city = escape(&party.address.city),
                country = escape(&party.address.country),
            );
        }
    }

    // Scheme `VA` marks a VAT identifier, `FC` a national tax number. The
    // scheme is what distinguishes them; the element is the same.
    if detail != PartyDetail::NameOnly {
        if let Some(vat_id) = &party.vat_id {
            let _ = write!(
                xml,
                r#"<ram:SpecifiedTaxRegistration><ram:ID schemeID="VA">{}</ram:ID></ram:SpecifiedTaxRegistration>"#,
                escape(vat_id)
            );
        }

        if let Some(tax_number) = &party.tax_number {
            let _ = write!(
                xml,
                r#"<ram:SpecifiedTaxRegistration><ram:ID schemeID="FC">{}</ram:ID></ram:SpecifiedTaxRegistration>"#,
                escape(tax_number)
            );
        }
    }

    let _ = write!(xml, "</ram:{element}>");
}
/// Required by the schema even when nothing is delivered physically.
fn delivery(xml: &mut String, invoice: &Invoice, profile: Profile) {
    xml.push_str("<ram:ApplicableHeaderTradeDelivery>");

    // Empty under MINIMUM: the element is required, its content is not allowed.
    if profile >= Profile::Basic
        && let Some(period) = &invoice.period
    {
        let _ = write!(
            xml,
            concat!(
                "<ram:ActualDeliverySupplyChainEvent><ram:OccurrenceDateTime>",
                "<udt:DateTimeString format=\"{format}\">{date}</udt:DateTimeString>",
                "</ram:OccurrenceDateTime></ram:ActualDeliverySupplyChainEvent>"
            ),
            format = DATE_FORMAT,
            date = compact_date(&period.to),
        );
    }

    xml.push_str("</ram:ApplicableHeaderTradeDelivery>");
}

/// Currency, payment, tax breakdown and the totals.
fn settlement(xml: &mut String, issuer: &Issuer, invoice: &Invoice, profile: Profile) {
    let _ = write!(
        xml,
        concat!(
            "<ram:ApplicableHeaderTradeSettlement>",
            "<ram:InvoiceCurrencyCode>{currency}</ram:InvoiceCurrencyCode>"
        ),
        currency = escape(&invoice.currency),
    );

    if profile >= Profile::Basic
        && let Some(payment) = &issuer.payment
    {
        let _ = write!(
            xml,
            concat!(
                "<ram:SpecifiedTradeSettlementPaymentMeans>",
                "<ram:TypeCode>{method}</ram:TypeCode>"
            ),
            method = escape(&payment.method),
        );

        if let Some(iban) = &payment.iban {
            let _ = write!(
                xml,
                concat!(
                    "<ram:PayeePartyCreditorFinancialAccount>",
                    "<ram:IBANID>{iban}</ram:IBANID>",
                    "</ram:PayeePartyCreditorFinancialAccount>"
                ),
                iban = escape(iban),
            );
        }

        xml.push_str("</ram:SpecifiedTradeSettlementPaymentMeans>");
    }

    if profile >= Profile::Basic {
        for entry in &invoice.totals.breakdown {
            trade_tax(xml, entry);
        }

        if let Some(period) = &invoice.period {
            let _ = write!(
                xml,
                concat!(
                    "<ram:BillingSpecifiedPeriod>",
                    "<ram:StartDateTime><udt:DateTimeString format=\"{format}\">{from}\
                     </udt:DateTimeString></ram:StartDateTime>",
                    "<ram:EndDateTime><udt:DateTimeString format=\"{format}\">{to}\
                     </udt:DateTimeString></ram:EndDateTime>",
                    "</ram:BillingSpecifiedPeriod>"
                ),
                format = DATE_FORMAT,
                from = compact_date(&period.from),
                to = compact_date(&period.to),
            );
        }

        if let Some(terms) = &invoice.terms {
            let _ = write!(
                xml,
                "<ram:SpecifiedTradePaymentTerms><ram:Description>{}</ram:Description>",
                escape(terms)
            );

            if let Some(due) = &invoice.due {
                let _ = write!(
                    xml,
                    concat!(
                        "<ram:DueDateDateTime><udt:DateTimeString format=\"{format}\">{due}\
                         </udt:DateTimeString></ram:DueDateDateTime>"
                    ),
                    format = DATE_FORMAT,
                    due = compact_date(due),
                );
            }

            xml.push_str("</ram:SpecifiedTradePaymentTerms>");
        }
    }

    summation(xml, invoice, profile);

    xml.push_str("</ram:ApplicableHeaderTradeSettlement>");
}

fn trade_tax(xml: &mut String, entry: &TaxBreakdown) {
    let _ = write!(
        xml,
        concat!(
            "<ram:ApplicableTradeTax>",
            "<ram:CalculatedAmount>{tax}</ram:CalculatedAmount>",
            "<ram:TypeCode>VAT</ram:TypeCode>"
        ),
        tax = amount(entry.tax),
    );

    if let Some(reason) = &entry.exemption_reason {
        let _ = write!(
            xml,
            "<ram:ExemptionReason>{}</ram:ExemptionReason>",
            escape(reason)
        );
    }

    let _ = write!(
        xml,
        concat!(
            "<ram:BasisAmount>{net}</ram:BasisAmount>",
            "<ram:CategoryCode>{category}</ram:CategoryCode>",
            "<ram:RateApplicablePercent>{rate}</ram:RateApplicablePercent>",
            "</ram:ApplicableTradeTax>"
        ),
        net = amount(entry.net),
        category = escape(&entry.category),
        rate = amount(entry.rate),
    );
}

/// The totals. MINIMUM carries these and nothing else, which is the whole
/// reason it exists: enough to book against, not enough to be an invoice.
fn summation(xml: &mut String, invoice: &Invoice, profile: Profile) {
    let totals = &invoice.totals;

    xml.push_str("<ram:SpecifiedTradeSettlementHeaderMonetarySummation>");

    if profile >= Profile::Basic {
        let _ = write!(
            xml,
            "<ram:LineTotalAmount>{}</ram:LineTotalAmount>",
            amount(totals.net)
        );
    }

    let _ = write!(
        xml,
        concat!(
            "<ram:TaxBasisTotalAmount>{net}</ram:TaxBasisTotalAmount>",
            "<ram:TaxTotalAmount currencyID=\"{currency}\">{tax}</ram:TaxTotalAmount>",
            "<ram:GrandTotalAmount>{gross}</ram:GrandTotalAmount>"
        ),
        net = amount(totals.net),
        currency = escape(&invoice.currency),
        tax = amount(totals.tax),
        gross = amount(totals.gross),
    );

    if profile >= Profile::Basic && !totals.paid.is_zero() {
        let _ = write!(
            xml,
            "<ram:TotalPrepaidAmount>{}</ram:TotalPrepaidAmount>",
            amount(totals.paid)
        );
    }

    let _ = write!(
        xml,
        concat!(
            "<ram:DuePayableAmount>{due}</ram:DuePayableAmount>",
            "</ram:SpecifiedTradeSettlementHeaderMonetarySummation>"
        ),
        due = amount(totals.due),
    );
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Monetary amounts and percentages: exactly two decimals.
///
/// CII allows more, but validators and readers alike expect the cent, and a
/// varying number of decimals is a needless source of mismatch reports.
fn amount(value: Decimal) -> String {
    format!("{:.2}", value.round_dp(2))
}

/// Quantities keep up to four decimals with trailing zeroes removed -- an hour
/// billed as 0.25 should not read as 0.2500.
fn quantity(value: Decimal) -> String {
    value.round_dp(4).normalize().to_string()
}

/// `YYYY-MM-DD` becomes `YYYYMMDD`, which is what format qualifier 102 means.
fn compact_date(value: &str) -> String {
    value.replace('-', "")
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            character => escaped.push(character),
        }
    }

    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/templates/invoice/");

    fn example() -> (Issuer, Invoice) {
        let issuer = std::fs::read_to_string(format!("{BASE}fixture.json"))
            .expect("example fixture must exist");

        let request = std::fs::read_to_string(format!("{BASE}__data/request.json"))
            .expect("example request must exist");

        let request: serde_json::Value =
            serde_json::from_str(&request).expect("example request must be json");

        (
            serde_json::from_str(&issuer).expect("fixture must match the issuer schema"),
            serde_json::from_value(request["data"].clone())
                .expect("request data must match the invoice schema"),
        )
    }

    #[test]
    fn example_is_valid_for_en16931() {
        let (issuer, invoice) = example();

        let problems = invoice
            .validate(&issuer, Profile::En16931)
            .err()
            .map(|error| error.problems)
            .unwrap_or_default();

        assert_eq!(problems, Vec::<String>::new());
    }

    /// Writes the XML to the build directory so it can be fed to Mustang by
    /// hand. The real check is that validator, not an assertion here.
    #[test]
    fn writes_factur_x_xml() {
        let (issuer, invoice) = example();
        let xml = generate(&issuer, &invoice, Profile::En16931);

        std::fs::write(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/factur-x.xml"),
            &xml,
        )
        .expect("target directory must be writable");

        assert!(xml.contains("<ram:ID>2026-0042</ram:ID>"));
        assert!(xml.contains("&amp;"), "the ampersand must be escaped");
    }

    /// Writes one XML per profile for external validation.
    ///
    /// The profiles below EN 16931 have never been through a validator: the
    /// `>=` switches in this module are a reasonable reading of the spec, not a
    /// verified one, and MINIMUM in particular forbids elements rather than
    /// merely omitting them.
    #[test]
    fn writes_every_profile() {
        let (issuer, invoice) = example();

        for profile in [Profile::Minimum, Profile::Basic, Profile::En16931] {
            let name = profile.conformance_level().replace(' ', "");
            let xml = generate(&issuer, &invoice, profile);

            std::fs::write(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("target/factur-x-{name}.xml")),
                &xml,
            )
            .expect("target directory must be writable");
        }
    }

    use crate::core::invoice::{
        Payment, Period,
        tests::{invoice as base_invoice, issuer as base_issuer},
    };

    fn dec(mantissa: i64, scale: u32) -> Decimal {
        Decimal::new(mantissa, scale)
    }

    /// Every optional field set. The base fixture leaves them empty, so the two
    /// together cover both sides of each `Option`.
    fn maximal() -> (Issuer, Invoice) {
        let mut issuer = base_issuer();
        issuer.seller.tax_number = Some("013/815/00815".to_owned());
        issuer.payment = Some(Payment {
            method: "58".to_owned(),
            iban: Some("DE02120300000000202051".to_owned()),
            bic: Some("BYLADEM1001".to_owned()),
            holder: Some("Seller GmbH".to_owned()),
            reference: Some("RE 2026-001".to_owned()),
        });

        let mut invoice = base_invoice();
        invoice.note = Some("Delivered".to_owned());
        invoice.terms = Some("Net 30".to_owned());
        invoice.period = Some(Period {
            from: "2026-07-01".to_owned(),
            to: "2026-07-31".to_owned(),
        });
        invoice.buyer.vat_id = Some("DE987654321".to_owned());
        invoice.lines[0].id = Some("POS-1".to_owned());
        invoice.lines[0].description = Some("Two hours".to_owned());
        invoice.totals.paid = dec(1900, 2);
        invoice.totals.due = dec(10000, 2);
        invoice.totals.breakdown[0].exemption_reason = Some("n/a".to_owned());

        (issuer, invoice)
    }

    // -----------------------------------------------------------------------
    // Optional fields
    // -----------------------------------------------------------------------

    #[test]
    fn writes_every_optional_field() {
        let (issuer, invoice) = maximal();
        let xml = generate(&issuer, &invoice, Profile::En16931);

        assert!(xml.contains("<ram:IncludedNote><ram:Content>Delivered<"));
        assert!(xml.contains("<ram:LineID>POS-1</ram:LineID>"));
        assert!(xml.contains("<ram:Description>Two hours</ram:Description>"));
        assert!(xml.contains("<ram:IBANID>DE02120300000000202051</ram:IBANID>"));
        assert!(xml.contains("<ram:BillingSpecifiedPeriod>"));
        assert!(xml.contains("<ram:ActualDeliverySupplyChainEvent>"));
        assert!(xml.contains("<ram:SpecifiedTradePaymentTerms>"));
        assert!(xml.contains("<ram:DueDateDateTime>"));
        assert!(xml.contains("<ram:ExemptionReason>n/a</ram:ExemptionReason>"));
        assert!(xml.contains("<ram:TotalPrepaidAmount>19.00</ram:TotalPrepaidAmount>"));
        assert!(xml.contains(r#"schemeID="VA""#));
        assert!(xml.contains(r#"schemeID="FC""#));
    }

    /// The same document with nothing optional set: every element above has to
    /// be absent rather than empty.
    #[test]
    fn omits_every_optional_field() {
        let xml = generate(&base_issuer(), &base_invoice(), Profile::En16931);

        assert!(!xml.contains("<ram:IncludedNote>"));
        assert!(!xml.contains("<ram:Description>"));
        assert!(!xml.contains("<ram:IBANID>"));
        assert!(!xml.contains("<ram:BillingSpecifiedPeriod>"));
        assert!(!xml.contains("<ram:ActualDeliverySupplyChainEvent>"));
        assert!(!xml.contains("<ram:SpecifiedTradePaymentTerms>"));
        assert!(!xml.contains("<ram:ExemptionReason>"));
        assert!(!xml.contains("<ram:TotalPrepaidAmount>"));
        assert!(!xml.contains(r#"schemeID="FC""#));

        // A line without an identifier is numbered by its position.
        assert!(xml.contains("<ram:LineID>1</ram:LineID>"));
    }

    /// Payment terms without a due date: the terms element is written, the due
    /// date inside it is not.
    #[test]
    fn writes_terms_without_a_due_date() {
        let (issuer, mut invoice) = maximal();
        invoice.due = None;

        let xml = generate(&issuer, &invoice, Profile::En16931);

        assert!(xml.contains("<ram:SpecifiedTradePaymentTerms>"));
        assert!(!xml.contains("<ram:DueDateDateTime>"));
    }

    /// Payment means without an IBAN -- cash or card.
    #[test]
    fn writes_payment_means_without_an_iban() {
        let (mut issuer, invoice) = maximal();
        issuer.payment = Some(Payment {
            method: "10".to_owned(),
            iban: None,
            bic: None,
            holder: None,
            reference: None,
        });

        let xml = generate(&issuer, &invoice, Profile::En16931);

        assert!(xml.contains("<ram:SpecifiedTradeSettlementPaymentMeans>"));
        assert!(!xml.contains("<ram:IBANID>"));
    }

    // -----------------------------------------------------------------------
    // Profiles
    // -----------------------------------------------------------------------

    /// MINIMUM forbids what it does not require, so a fully populated invoice
    /// still has to come out stripped.
    #[test]
    fn strips_everything_minimum_forbids() {
        let (issuer, invoice) = maximal();
        let xml = generate(&issuer, &invoice, Profile::Minimum);

        assert!(!xml.contains("<ram:IncludedSupplyChainTradeLineItem>"));
        assert!(!xml.contains("<ram:IncludedNote>"));
        assert!(!xml.contains("<ram:SpecifiedTradeSettlementPaymentMeans>"));
        assert!(!xml.contains("<ram:BillingSpecifiedPeriod>"));
        assert!(!xml.contains("<ram:SpecifiedTradePaymentTerms>"));
        assert!(!xml.contains("<ram:LineTotalAmount>"));
        assert!(!xml.contains("<ram:TotalPrepaidAmount>"));

        // The delivery element is required but must stay empty.
        assert!(
            xml.contains("<ram:ApplicableHeaderTradeDelivery></ram:ApplicableHeaderTradeDelivery>")
        );
    }

    /// Under MINIMUM the seller keeps country and tax registration, the buyer
    /// is reduced to a name.
    #[test]
    fn reduces_both_parties_under_minimum() {
        let (issuer, invoice) = maximal();
        let xml = generate(&issuer, &invoice, Profile::Minimum);

        let seller = xml
            .split("<ram:SellerTradeParty>")
            .nth(1)
            .and_then(|rest| rest.split("</ram:SellerTradeParty>").next())
            .expect("seller party");

        assert!(seller.contains("<ram:CountryID>DE</ram:CountryID>"));
        assert!(!seller.contains("<ram:LineOne>"));
        assert!(seller.contains(r#"schemeID="VA""#));

        let buyer = xml
            .split("<ram:BuyerTradeParty>")
            .nth(1)
            .and_then(|rest| rest.split("</ram:BuyerTradeParty>").next())
            .expect("buyer party");

        assert!(!buyer.contains("<ram:PostalTradeAddress>"));
        assert!(!buyer.contains("<ram:SpecifiedTaxRegistration>"));
    }

    // -----------------------------------------------------------------------
    // Formatting
    // -----------------------------------------------------------------------

    #[test]
    fn escapes_every_reserved_character() {
        assert_eq!(
            escape(r#"A & B < C > D " E ' F"#),
            "A &amp; B &lt; C &gt; D &quot; E &apos; F"
        );
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        assert_eq!(escape("Müller-Lüdenscheidt"), "Müller-Lüdenscheidt");
    }

    #[test]
    fn writes_amounts_to_the_cent() {
        assert_eq!(amount(dec(19, 0)), "19.00");
        assert_eq!(amount(dec(100_006, 3)), "100.01");

        // rust_decimal rounds half to even, so an exact half stays down here.
        assert_eq!(amount(dec(100_005, 3)), "100.00");
    }

    /// A quarter hour reads as 0.25, not 0.2500.
    #[test]
    fn drops_trailing_zeroes_from_quantities() {
        assert_eq!(quantity(dec(2500, 4)), "0.25");
        assert_eq!(quantity(dec(2, 0)), "2");
        assert_eq!(quantity(dec(123_456, 5)), "1.2346");
    }

    #[test]
    fn compacts_dates_for_format_102() {
        assert_eq!(compact_date("2026-08-11"), "20260811");
    }
}
