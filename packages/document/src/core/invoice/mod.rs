//! Canonical invoice data.
//!
//! The one shape a caller sends when `document.type` is `invoice`. Everything
//! Factur-X needs is derivable from here; the template renders from the same
//! data, so the visible document and the embedded XML cannot drift apart.
//!
//! Fields carry their EN 16931 business term (BT-/BG-) in the comment. That is
//! the vocabulary the norm, the validators and every integration partner use --
//! keeping it visible is cheaper than translating back and forth.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::profile::Profile;

pub mod factur_x;

/// What belongs to the template rather than to any one invoice.
///
/// The issuer's own details are fixed at publish time and covered by the
/// content hash: moving offices or changing bank means a new template version,
/// which is exactly what reproducing an old invoice requires.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Issuer {
    pub seller: Party,
    pub payment: Option<Payment>,
}

/// A complete invoice, independent of profile.
///
/// Profile-dependent fields are `Option` and checked in [`Invoice::validate`]
/// rather than split across one struct per profile: a caller that moves from
/// BASIC to EN 16931 adds fields, it does not change shape.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Invoice {
    /// BT-1: invoice number, unique for the seller.
    pub number: String,
    /// BT-3: UNTDID 1001. `380` commercial invoice, `381` credit note.
    #[serde(default = "commercial_invoice")]
    pub kind: String,
    /// BT-2: issue date, `YYYY-MM-DD`.
    pub issued: String,
    /// BT-9: due date, `YYYY-MM-DD`.
    pub due: Option<String>,
    /// BT-5: ISO 4217 currency, e.g. `EUR`.
    pub currency: String,
    /// BG-14: billing period covered by this invoice.
    pub period: Option<Period>,

    pub buyer: Party,

    /// BG-25: at least one line.
    pub lines: Vec<Line>,
    pub totals: Totals,
    pub payment: Option<Payment>,

    /// BT-20: payment terms, free text.
    pub terms: Option<String>,
    /// BT-22: note shown on the document.
    pub note: Option<String>,
}

/// BG-14: `from` and `to` inclusive, `YYYY-MM-DD`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Period {
    pub from: String,
    pub to: String,
}

/// BG-4 (seller) / BG-7 (buyer).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Party {
    /// BT-27 / BT-44: registered legal name.
    pub name: String,
    pub address: Address,
    /// BT-31 / BT-48: VAT identifier, e.g. `DE123456789`.
    pub vat_id: Option<String>,
    /// BT-32: tax number, for a seller without a VAT identifier.
    pub tax_number: Option<String>,
    /// BT-41 / BT-56: contact name.
    pub contact: Option<String>,
    /// BT-43 / BT-58.
    pub email: Option<String>,
    /// BT-42 / BT-57.
    pub phone: Option<String>,
}

/// BG-5 / BG-8.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Address {
    /// BT-35 / BT-50.
    pub street: String,
    /// BT-38 / BT-53.
    pub postcode: String,
    /// BT-37 / BT-52.
    pub city: String,
    /// BT-40 / BT-55: ISO 3166-1 alpha-2, e.g. `DE`.
    pub country: String,
}

/// BG-25: one invoice line.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    /// BT-126: line identifier. Positional index when omitted.
    pub id: Option<String>,
    /// BT-153: item name.
    pub name: String,
    /// BT-154: item description.
    pub description: Option<String>,
    /// BT-129: quantity, may be fractional.
    pub quantity: Decimal,
    /// BT-130: UN/ECE Rec 20 code. `H87` piece, `HUR` hour, `DAY` day,
    /// `KGM` kilogram, `MTR` metre, `LS` lump sum.
    pub unit: String,
    /// BT-146: net price per unit.
    pub unit_price: Decimal,
    /// BT-131: net line amount, `quantity * unit_price`.
    pub net: Decimal,
    pub tax: Tax,
}

/// BT-151/152: how a line is taxed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tax {
    /// BT-151: UNTDID 5305. `S` standard rate, `AE` reverse charge,
    /// `Z` zero rated, `E` exempt, `K` intra-community supply,
    /// `G` export, `O` outside scope.
    pub category: String,
    /// BT-152: percentage, e.g. `19`. `0` for every category but `S`.
    pub rate: Decimal,
    /// BT-120: reason. Required whenever the category is not `S`.
    pub exemption_reason: Option<String>,
}

/// BG-22: document totals. Sent rather than derived -- the invoice states what
/// the seller claims, and validation checks the arithmetic against the lines.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Totals {
    /// BT-106: sum of line net amounts.
    pub net: Decimal,
    /// BT-110: total VAT.
    pub tax: Decimal,
    /// BT-112: net plus tax.
    pub gross: Decimal,
    /// BT-113: already paid.
    #[serde(default)]
    pub paid: Decimal,
    /// BT-115: amount due for payment, `gross - paid`.
    pub due: Decimal,
    /// BG-23: one entry per category and rate in use.
    pub breakdown: Vec<TaxBreakdown>,
}

/// BG-23: VAT broken down by category and rate.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaxBreakdown {
    /// BT-118: UNTDID 5305, as on the lines.
    pub category: String,
    /// BT-119: percentage.
    pub rate: Decimal,
    /// BT-116: taxable amount at this rate.
    pub net: Decimal,
    /// BT-117: tax at this rate.
    pub tax: Decimal,
    /// BT-121: exemption reason, when the category is not `S`.
    pub exemption_reason: Option<String>,
}

/// BG-16: how the buyer is meant to pay.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Payment {
    /// BT-81: UNTDID 4461. `58` SEPA credit transfer, `30` credit transfer,
    /// `10` cash, `48` card, `49` direct debit.
    pub method: String,
    /// BT-84: account identifier, IBAN for a transfer.
    pub iban: Option<String>,
    /// BT-86: BIC.
    pub bic: Option<String>,
    /// BT-85: account holder.
    pub holder: Option<String>,
    /// BT-83: remittance reference the seller wants quoted.
    pub reference: Option<String>,
}

fn commercial_invoice() -> String {
    "380".to_owned()
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Everything wrong with an invoice, not just the first thing.
///
/// A caller fixing an integration wants the whole list in one response;
/// reporting one field at a time turns that into a guessing game.
#[derive(Debug, thiserror::Error)]
#[error("invoice is not valid for profile `{profile}`")]
pub struct InvalidInvoice {
    pub profile: Profile,
    pub problems: Vec<String>,
}

/// Tax categories that do not charge VAT and therefore need a reason.
const NEEDS_REASON: &[&str] = &["AE", "Z", "E", "K", "G", "O"];

/// Every category this service accepts.
const CATEGORIES: &[&str] = &["S", "AE", "Z", "E", "K", "G", "O"];

impl Invoice {
    /// Checks the invoice against a profile.
    ///
    /// Structural requirements -- a field being present at all -- are already
    /// enforced by deserialization. What remains is what serde cannot see:
    /// codes from closed lists, arithmetic that has to add up, and fields the
    /// norm makes conditional on a value elsewhere.
    ///
    /// # Errors
    ///
    /// [`InvalidInvoice`] carrying every problem found.
    pub fn validate(&self, issuer: &Issuer, profile: Profile) -> Result<(), InvalidInvoice> {
        let mut problems = Vec::new();

        self.check_header(&mut problems);
        self.check_parties(issuer, profile, &mut problems);
        self.check_lines(&mut problems);
        self.check_totals(&mut problems);

        if profile >= Profile::En16931 {
            self.check_en16931(&mut problems);
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(InvalidInvoice { profile, problems })
        }
    }

    fn check_header(&self, problems: &mut Vec<String>) {
        if self.number.trim().is_empty() {
            problems.push("`number` is empty".to_owned());
        }

        if !is_date(&self.issued) {
            problems.push(format!(
                "`issued` is not a YYYY-MM-DD date: `{}`",
                self.issued
            ));
        }

        if let Some(due) = &self.due
            && !is_date(due)
        {
            problems.push(format!("`due` is not a YYYY-MM-DD date: `{due}`"));
        }

        if let Some(period) = &self.period {
            if !is_date(&period.from) {
                problems.push(format!("`period.from` is not a date: `{}`", period.from));
            }
            if !is_date(&period.to) {
                problems.push(format!("`period.to` is not a date: `{}`", period.to));
            }
        }

        if self.currency.len() != 3 || !self.currency.bytes().all(|b| b.is_ascii_uppercase()) {
            problems.push(format!(
                "`currency` is not an ISO 4217 code: `{}`",
                self.currency
            ));
        }

        if self.lines.is_empty() {
            problems.push("`lines` is empty".to_owned());
        }
    }

    fn check_parties(&self, issuer: &Issuer, profile: Profile, problems: &mut Vec<String>) {
        for (side, party) in [("seller", &issuer.seller), ("buyer", &self.buyer)] {
            if party.name.trim().is_empty() {
                problems.push(format!("`{side}.name` is empty"));
            }

            let country = &party.address.country;

            if country.len() != 2 || !country.bytes().all(|b| b.is_ascii_uppercase()) {
                problems.push(format!(
                    "`{side}.address.country` is not an ISO 3166-1 alpha-2 code: `{country}`"
                ));
            }

            if party.address.city.trim().is_empty() {
                problems.push(format!("`{side}.address.city` is empty"));
            }
        }

        // A seller charging VAT has to be identifiable for it.
        if profile >= Profile::En16931
            && issuer.seller.vat_id.is_none()
            && issuer.seller.tax_number.is_none()
        {
            problems.push("`seller` needs either `vat_id` or `tax_number`".to_owned());
        }

        // Reverse charge shifts the liability to the buyer, which only works if
        // the buyer is identified.
        let reverse_charge = self.lines.iter().any(|line| line.tax.category == "AE");

        if reverse_charge && self.buyer.vat_id.is_none() {
            problems.push("`buyer.vat_id` is required when a line uses category `AE`".to_owned());
        }
    }

    fn check_lines(&self, problems: &mut Vec<String>) {
        for (index, line) in self.lines.iter().enumerate() {
            let at = line.id.as_deref().map_or_else(
                || format!("lines[{index}]"),
                |id| format!("lines[{index}] (`{id}`)"),
            );

            if line.name.trim().is_empty() {
                problems.push(format!("`{at}.name` is empty"));
            }

            if line.unit.trim().is_empty() {
                problems.push(format!("`{at}.unit` is empty"));
            }

            if !CATEGORIES.contains(&line.tax.category.as_str()) {
                problems.push(format!(
                    "`{at}.tax.category` is not a UNTDID 5305 code we support: `{}`",
                    line.tax.category
                ));
            }

            if NEEDS_REASON.contains(&line.tax.category.as_str())
                && line.tax.exemption_reason.is_none()
            {
                problems.push(format!(
                    "`{at}.tax.exemption_reason` is required for category `{}`",
                    line.tax.category
                ));
            }

            if line.tax.category != "S" && !line.tax.rate.is_zero() {
                problems.push(format!(
                    "`{at}.tax.rate` must be 0 for category `{}`",
                    line.tax.category
                ));
            }

            let expected = line.quantity * line.unit_price;

            if !close(line.net, expected) {
                problems.push(format!(
                    "`{at}.net` is {} but quantity * unit_price is {expected}",
                    line.net
                ));
            }
        }
    }

    fn check_totals(&self, problems: &mut Vec<String>) {
        let lines_net: Decimal = self.lines.iter().map(|line| line.net).sum();

        if !close(self.totals.net, lines_net) {
            problems.push(format!(
                "`totals.net` is {} but the lines sum to {lines_net}",
                self.totals.net
            ));
        }

        let breakdown_tax: Decimal = self.totals.breakdown.iter().map(|entry| entry.tax).sum();

        if !close(self.totals.tax, breakdown_tax) {
            problems.push(format!(
                "`totals.tax` is {} but the breakdown sums to {breakdown_tax}",
                self.totals.tax
            ));
        }

        if !close(self.totals.gross, self.totals.net + self.totals.tax) {
            problems.push(format!(
                "`totals.gross` is {} but net plus tax is {}",
                self.totals.gross,
                self.totals.net + self.totals.tax
            ));
        }

        if !close(self.totals.due, self.totals.gross - self.totals.paid) {
            problems.push(format!(
                "`totals.due` is {} but gross minus paid is {}",
                self.totals.due,
                self.totals.gross - self.totals.paid
            ));
        }
    }

    /// Rules EN 16931 adds on top of BASIC.
    fn check_en16931(&self, problems: &mut Vec<String>) {
        if self.due.is_none() && self.terms.is_none() {
            problems.push("either `due` or `terms` is required".to_owned());
        }

        if self.totals.breakdown.is_empty() {
            problems.push("`totals.breakdown` is empty".to_owned());
        }

        // Every rate charged on a line has to appear in the breakdown, and the
        // taxable amounts have to match. This is what a validator checks first.
        for line in &self.lines {
            let matching: Decimal = self
                .lines
                .iter()
                .filter(|other| {
                    other.tax.category == line.tax.category && other.tax.rate == line.tax.rate
                })
                .map(|other| other.net)
                .sum();

            let entry =
                self.totals.breakdown.iter().find(|entry| {
                    entry.category == line.tax.category && entry.rate == line.tax.rate
                });

            match entry {
                None => problems.push(format!(
                    "`totals.breakdown` has no entry for category `{}` at {}%",
                    line.tax.category, line.tax.rate
                )),
                Some(entry) if !close(entry.net, matching) => problems.push(format!(
                    "`totals.breakdown` for `{}` at {}% is {} but the lines sum to {matching}",
                    entry.category, entry.rate, entry.net
                )),
                Some(_) => {}
            }
        }

        for entry in &self.totals.breakdown {
            if NEEDS_REASON.contains(&entry.category.as_str()) && entry.exemption_reason.is_none() {
                problems.push(format!(
                    "`totals.breakdown` for category `{}` needs an `exemption_reason`",
                    entry.category
                ));
            }
        }
    }
}

/// Rounds both sides to the cent before comparing.
///
/// Callers compute totals in whatever their system uses; insisting on exact
/// equality would reject invoices that are correct to the cent, which is the
/// only precision that gets printed or paid.
fn close(left: Decimal, right: Decimal) -> bool {
    left.round_dp(2) == right.round_dp(2)
}

/// Shape check only -- `2026-02-31` passes here and fails in the XML.
fn is_date(value: &str) -> bool {
    let bytes = value.as_bytes();

    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(mantissa: i64, scale: u32) -> Decimal {
        Decimal::new(mantissa, scale)
    }

    fn address() -> Address {
        Address {
            street: "Hauptstrasse 1".to_owned(),
            postcode: "60311".to_owned(),
            city: "Frankfurt am Main".to_owned(),
            country: "DE".to_owned(),
        }
    }

    pub(crate) fn issuer() -> Issuer {
        Issuer {
            seller: Party {
                name: "Seller GmbH".to_owned(),
                address: address(),
                vat_id: Some("DE123456789".to_owned()),
                tax_number: None,
                contact: None,
                email: None,
                phone: None,
            },
            payment: None,
        }
    }

    /// Valid at every profile, including EN 16931. Each test breaks exactly one
    /// thing, so a failure names the rule that fired.
    pub(crate) fn invoice() -> Invoice {
        Invoice {
            number: "2026-001".to_owned(),
            kind: commercial_invoice(),
            issued: "2026-08-11".to_owned(),
            due: Some("2026-09-10".to_owned()),
            currency: "EUR".to_owned(),
            period: None,
            buyer: Party {
                name: "Buyer AG".to_owned(),
                address: address(),
                vat_id: None,
                tax_number: None,
                contact: None,
                email: None,
                phone: None,
            },
            lines: vec![Line {
                id: None,
                name: "Consulting".to_owned(),
                description: None,
                quantity: dec(2, 0),
                unit: "HUR".to_owned(),
                unit_price: dec(5000, 2),
                net: dec(10000, 2),
                tax: Tax {
                    category: "S".to_owned(),
                    rate: dec(19, 0),
                    exemption_reason: None,
                },
            }],
            totals: Totals {
                net: dec(10000, 2),
                tax: dec(1900, 2),
                gross: dec(11900, 2),
                paid: Decimal::ZERO,
                due: dec(11900, 2),
                breakdown: vec![TaxBreakdown {
                    category: "S".to_owned(),
                    rate: dec(19, 0),
                    net: dec(10000, 2),
                    tax: dec(1900, 2),
                    exemption_reason: None,
                }],
            },
            payment: None,
            terms: None,
            note: None,
        }
    }

    fn problems(invoice: &Invoice, profile: Profile) -> Vec<String> {
        invoice
            .validate(&issuer(), profile)
            .expect_err("invoice should be rejected")
            .problems
    }

    /// Asserts the invoice is rejected for exactly the reason we broke.
    #[track_caller]
    fn rejects(invoice: &Invoice, needle: &str) {
        let problems = problems(invoice, Profile::En16931);

        assert!(
            problems.iter().any(|problem| problem.contains(needle)),
            "expected a problem containing `{needle}`, got {problems:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Happy path
    // -----------------------------------------------------------------------

    #[test]
    fn accepts_a_complete_invoice() {
        for profile in [Profile::Minimum, Profile::Basic, Profile::En16931] {
            invoice()
                .validate(&issuer(), profile)
                .expect("fixture should be valid at every profile");
        }
    }

    /// The extra EN 16931 rules must not fire below that profile.
    #[test]
    fn skips_en16931_rules_below_the_profile() {
        let mut invoice = invoice();
        invoice.due = None;
        invoice.totals.breakdown.clear();
        invoice.totals.tax = Decimal::ZERO;
        invoice.totals.gross = dec(10000, 2);
        invoice.totals.due = dec(10000, 2);

        invoice
            .validate(&issuer(), Profile::Basic)
            .expect("breakdown is only required from EN 16931 on");
    }

    #[test]
    fn reports_every_problem_at_once() {
        let mut invoice = invoice();
        invoice.number = String::new();
        invoice.currency = "eur".to_owned();

        assert_eq!(problems(&invoice, Profile::En16931).len(), 2);
    }

    #[test]
    fn names_the_profile_it_failed_against() {
        let mut invoice = invoice();
        invoice.number = String::new();

        let error = invoice
            .validate(&issuer(), Profile::Basic)
            .expect_err("empty number");

        assert_eq!(error.profile, Profile::Basic);
    }

    // -----------------------------------------------------------------------
    // Header
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_an_empty_number() {
        let mut invoice = invoice();
        invoice.number = "   ".to_owned();

        rejects(&invoice, "`number` is empty");
    }

    #[test]
    fn rejects_an_unparseable_issue_date() {
        let mut invoice = invoice();
        invoice.issued = "11.08.2026".to_owned();

        rejects(&invoice, "`issued` is not a YYYY-MM-DD date");
    }

    #[test]
    fn rejects_an_unparseable_due_date() {
        let mut invoice = invoice();
        invoice.due = Some("next week".to_owned());

        rejects(&invoice, "`due` is not a YYYY-MM-DD date");
    }

    #[test]
    fn rejects_an_unparseable_period() {
        let mut invoice = invoice();
        invoice.period = Some(Period {
            from: "2026-08".to_owned(),
            to: "whenever".to_owned(),
        });

        let problems = problems(&invoice, Profile::En16931);

        assert!(problems.iter().any(|p| p.contains("`period.from`")));
        assert!(problems.iter().any(|p| p.contains("`period.to`")));
    }

    #[test]
    fn accepts_a_valid_period() {
        let mut invoice = invoice();
        invoice.period = Some(Period {
            from: "2026-07-01".to_owned(),
            to: "2026-07-31".to_owned(),
        });

        invoice
            .validate(&issuer(), Profile::En16931)
            .expect("a well-formed period is fine");
    }

    #[test]
    fn rejects_a_currency_that_is_not_iso_4217() {
        for currency in ["EURO", "eur"] {
            let mut invoice = invoice();
            invoice.currency = currency.to_owned();

            rejects(&invoice, "`currency` is not an ISO 4217 code");
        }
    }

    #[test]
    fn rejects_an_invoice_without_lines() {
        let mut invoice = invoice();
        invoice.lines.clear();
        invoice.totals.net = Decimal::ZERO;
        invoice.totals.tax = Decimal::ZERO;
        invoice.totals.gross = Decimal::ZERO;
        invoice.totals.due = Decimal::ZERO;
        invoice.totals.breakdown.clear();

        rejects(&invoice, "`lines` is empty");
    }

    // -----------------------------------------------------------------------
    // Parties
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_a_buyer_without_a_name() {
        let mut invoice = invoice();
        invoice.buyer.name = " ".to_owned();

        rejects(&invoice, "`buyer.name` is empty");
    }

    #[test]
    fn rejects_a_seller_without_a_name() {
        let mut issuer = issuer();
        issuer.seller.name = String::new();

        let problems = invoice()
            .validate(&issuer, Profile::En16931)
            .expect_err("empty seller name")
            .problems;

        assert!(
            problems
                .iter()
                .any(|p| p.contains("`seller.name` is empty"))
        );
    }

    #[test]
    fn rejects_a_country_that_is_not_iso_3166() {
        for country in ["DEU", "de"] {
            let mut invoice = invoice();
            invoice.buyer.address.country = country.to_owned();

            rejects(&invoice, "`buyer.address.country` is not an ISO 3166-1");
        }
    }

    #[test]
    fn rejects_an_empty_city() {
        let mut invoice = invoice();
        invoice.buyer.address.city = "\t".to_owned();

        rejects(&invoice, "`buyer.address.city` is empty");
    }

    #[test]
    fn requires_a_seller_tax_identifier_from_en16931() {
        let mut issuer = issuer();
        issuer.seller.vat_id = None;
        issuer.seller.tax_number = None;

        let problems = invoice()
            .validate(&issuer, Profile::En16931)
            .expect_err("seller needs a tax identifier")
            .problems;

        assert!(
            problems
                .iter()
                .any(|p| p.contains("`vat_id` or `tax_number`"))
        );

        // Below EN 16931 the same seller is acceptable.
        invoice()
            .validate(&issuer, Profile::Basic)
            .expect("only EN 16931 demands a tax identifier");
    }

    #[test]
    fn accepts_a_seller_identified_by_tax_number() {
        let mut issuer = issuer();
        issuer.seller.vat_id = None;
        issuer.seller.tax_number = Some("013/815/00815".to_owned());

        invoice()
            .validate(&issuer, Profile::En16931)
            .expect("a tax number identifies the seller too");
    }

    /// Reverse charge moves the liability to the buyer, so the buyer has to be
    /// identifiable for VAT.
    #[test]
    fn requires_a_buyer_vat_id_for_reverse_charge() {
        let mut invoice = invoice();
        invoice.lines[0].tax = Tax {
            category: "AE".to_owned(),
            rate: Decimal::ZERO,
            exemption_reason: Some("Reverse charge".to_owned()),
        };
        invoice.totals.tax = Decimal::ZERO;
        invoice.totals.gross = dec(10000, 2);
        invoice.totals.due = dec(10000, 2);
        invoice.totals.breakdown = vec![TaxBreakdown {
            category: "AE".to_owned(),
            rate: Decimal::ZERO,
            net: dec(10000, 2),
            tax: Decimal::ZERO,
            exemption_reason: Some("Reverse charge".to_owned()),
        }];

        rejects(&invoice, "`buyer.vat_id` is required");

        invoice.buyer.vat_id = Some("DE987654321".to_owned());

        invoice
            .validate(&issuer(), Profile::En16931)
            .expect("an identified buyer can take the liability");
    }

    // -----------------------------------------------------------------------
    // Lines
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_a_line_without_a_name() {
        let mut invoice = invoice();
        invoice.lines[0].name = String::new();

        rejects(&invoice, "`lines[0].name` is empty");
    }

    #[test]
    fn rejects_a_line_without_a_unit() {
        let mut invoice = invoice();
        invoice.lines[0].unit = "  ".to_owned();

        rejects(&invoice, "`lines[0].unit` is empty");
    }

    /// A line that carries its own identifier is reported by that identifier.
    #[test]
    fn names_a_line_by_its_identifier() {
        let mut invoice = invoice();
        invoice.lines[0].id = Some("POS-7".to_owned());
        invoice.lines[0].name = String::new();

        rejects(&invoice, "`lines[0] (`POS-7`).name` is empty");
    }

    #[test]
    fn rejects_an_unsupported_tax_category() {
        let mut invoice = invoice();
        invoice.lines[0].tax.category = "X".to_owned();

        rejects(&invoice, "is not a UNTDID 5305 code we support");
    }

    #[test]
    fn requires_a_reason_for_untaxed_categories() {
        for category in NEEDS_REASON {
            let mut invoice = invoice();
            invoice.lines[0].tax = Tax {
                category: (*category).to_owned(),
                rate: Decimal::ZERO,
                exemption_reason: None,
            };

            rejects(&invoice, ".tax.exemption_reason` is required");
        }
    }

    #[test]
    fn rejects_a_nonzero_rate_outside_the_standard_category() {
        let mut invoice = invoice();
        invoice.lines[0].tax = Tax {
            category: "Z".to_owned(),
            rate: dec(19, 0),
            exemption_reason: Some("Zero rated".to_owned()),
        };

        rejects(&invoice, ".tax.rate` must be 0 for category `Z`");
    }

    #[test]
    fn rejects_a_line_total_that_does_not_multiply_out() {
        let mut invoice = invoice();
        invoice.lines[0].net = dec(9900, 2);

        rejects(&invoice, "but quantity * unit_price is");
    }

    // -----------------------------------------------------------------------
    // Totals
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_a_net_total_that_disagrees_with_the_lines() {
        let mut invoice = invoice();
        invoice.totals.net = dec(9000, 2);

        rejects(&invoice, "but the lines sum to");
    }

    #[test]
    fn rejects_a_tax_total_that_disagrees_with_the_breakdown() {
        let mut invoice = invoice();
        invoice.totals.tax = dec(2000, 2);

        rejects(&invoice, "but the breakdown sums to");
    }

    #[test]
    fn rejects_a_gross_total_that_is_not_net_plus_tax() {
        let mut invoice = invoice();
        invoice.totals.gross = dec(12000, 2);

        rejects(&invoice, "but net plus tax is");
    }

    #[test]
    fn rejects_an_amount_due_that_ignores_what_was_paid() {
        let mut invoice = invoice();
        invoice.totals.paid = dec(1900, 2);

        rejects(&invoice, "but gross minus paid is");
    }

    #[test]
    fn accepts_a_partially_paid_invoice() {
        let mut invoice = invoice();
        invoice.totals.paid = dec(1900, 2);
        invoice.totals.due = dec(10000, 2);

        invoice
            .validate(&issuer(), Profile::En16931)
            .expect("due is gross minus paid");
    }

    // -----------------------------------------------------------------------
    // EN 16931
    // -----------------------------------------------------------------------

    #[test]
    fn requires_a_due_date_or_payment_terms() {
        let mut invoice = invoice();
        invoice.due = None;

        rejects(&invoice, "either `due` or `terms` is required");

        invoice.terms = Some("Net 30".to_owned());

        invoice
            .validate(&issuer(), Profile::En16931)
            .expect("payment terms stand in for a due date");
    }

    #[test]
    fn requires_a_tax_breakdown() {
        let mut invoice = invoice();
        invoice.totals.breakdown.clear();
        invoice.totals.tax = Decimal::ZERO;
        invoice.totals.gross = dec(10000, 2);
        invoice.totals.due = dec(10000, 2);

        rejects(&invoice, "`totals.breakdown` is empty");
    }

    #[test]
    fn requires_a_breakdown_entry_for_every_rate_charged() {
        let mut invoice = invoice();
        invoice.totals.breakdown[0].rate = dec(7, 0);

        rejects(&invoice, "has no entry for category `S` at 19%");
    }

    #[test]
    fn rejects_a_breakdown_that_disagrees_with_the_lines() {
        let mut invoice = invoice();
        invoice.totals.breakdown[0].net = dec(9000, 2);

        rejects(&invoice, "but the lines sum to");
    }

    #[test]
    fn requires_a_reason_on_untaxed_breakdown_entries() {
        let mut invoice = invoice();
        invoice.lines[0].tax = Tax {
            category: "E".to_owned(),
            rate: Decimal::ZERO,
            exemption_reason: Some("Exempt".to_owned()),
        };
        invoice.buyer.vat_id = Some("DE987654321".to_owned());
        invoice.totals.tax = Decimal::ZERO;
        invoice.totals.gross = dec(10000, 2);
        invoice.totals.due = dec(10000, 2);
        invoice.totals.breakdown = vec![TaxBreakdown {
            category: "E".to_owned(),
            rate: Decimal::ZERO,
            net: dec(10000, 2),
            tax: Decimal::ZERO,
            exemption_reason: None,
        }];

        rejects(&invoice, "needs an `exemption_reason`");
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    #[test]
    fn compares_to_the_cent() {
        assert!(close(dec(100_004, 3), dec(10000, 2)));
        assert!(!close(dec(100_006, 3), dec(10000, 2)));
    }

    #[test]
    fn accepts_a_well_formed_date() {
        assert!(is_date("2026-08-11"));
    }

    /// Shape only: an impossible day passes here and fails in the XML.
    #[test]
    fn accepts_an_impossible_but_well_shaped_date() {
        assert!(is_date("2026-02-31"));
    }

    #[test]
    fn rejects_malformed_dates() {
        assert!(!is_date("2026-08-1"));
        assert!(!is_date("2026/08-11"));
        assert!(!is_date("2026-08/11"));
        assert!(!is_date("2026-08-1x"));
    }

    #[test]
    fn defaults_to_a_commercial_invoice() {
        assert_eq!(commercial_invoice(), "380");
    }
}
