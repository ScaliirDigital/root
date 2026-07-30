//! Factur-X conformance profiles.
//!
//! The profile decides three things at once: which fields an invoice must
//! carry, how much of them ends up in the XML, and what the XMP block claims
//! the document is. One type, so those three cannot drift apart.

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter, Result};

/// Ordered by how much a profile requires: `Minimum < Basic < En16931`.
///
/// The ordering is what validation compares against, so a rule can be written
/// as "from EN 16931 upwards" instead of listing profiles.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Accounting aid only. Not a valid invoice under German VAT law
    /// (§ 14 `UStG`) and does not satisfy the e-invoicing obligation.
    Minimum,
    /// The lowest profile that is a legally valid invoice.
    Basic,
    /// Full EN 16931 semantic model. The recommended default.
    En16931,
}

impl Profile {
    /// The URN that identifies the profile in the XML and the XMP block.
    #[must_use]
    pub fn urn(self) -> &'static str {
        match self {
            Self::Minimum => "urn:factur-x.eu:1p0:minimum",
            Self::Basic => "urn:cen.eu:en16931:2017#compliant#urn:factur-x.eu:1p0:basic",
            Self::En16931 => "urn:cen.eu:en16931:2017",
        }
    }

    /// The `ConformanceLevel` written into the XMP metadata.
    #[must_use]
    pub fn conformance_level(self) -> &'static str {
        match self {
            Self::Minimum => "MINIMUM",
            Self::Basic => "BASIC",
            Self::En16931 => "EN 16931",
        }
    }
}

impl Display for Profile {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result {
        formatter.write_str(self.conformance_level())
    }
}
