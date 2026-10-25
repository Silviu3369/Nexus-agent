//! What data may leave for which AI provider (ADR-PROVIDER-002). A
//! connection may receive up to a data class, decided from its provider's
//! terms (a fact Nexus keeps, dated, with its source) and what the user says
//! about it: a paid tier, a signed data processing addendum, consent to the
//! provider using their own data. Terms not confirmed allow only public data.

use crate::state::DataClass;

/// How a data class ranks: a connection allowed one may receive those below.
pub fn rank(class: DataClass) -> u8 {
    match class {
        DataClass::Public => 0,
        DataClass::Personal => 1,
        DataClass::Business => 2,
    }
}

/// The higher of two classes: what a turn holds once both are in it.
pub fn highest(a: DataClass, b: DataClass) -> DataClass {
    if rank(a) >= rank(b) { a } else { b }
}

/// Whether `held` may go to a connection allowed `allowed`.
pub fn may_receive(allowed: DataClass, held: DataClass) -> bool {
    rank(held) <= rank(allowed)
}

/// Whether the provider has a data processing addendum for this use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dpa {
    /// Part of the terms the user accepted.
    Included,
    /// Available; the user signs it from their account.
    Offered,
    None,
    /// Not confirmed from the provider's own documents.
    Unknown,
}

/// A provider's terms for one tier, as Nexus knows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Terms {
    /// Whether the provider uses what it receives to improve its models;
    /// `None` when not confirmed.
    pub trains_on_data: Option<bool>,
    pub dpa: Dpa,
}

/// What the user says about a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Declared {
    pub free_tier: bool,
    /// They signed the provider's data processing addendum.
    pub dpa_signed: bool,
    /// They let the provider use their own data to improve its models.
    pub allows_training: bool,
}

/// The highest class a connection may receive: other people's data needs a
/// paid tier, a data processing addendum and no training on it; the user's
/// own data needs no training on it, or their consent; anything else may
/// receive only public data.
pub fn connection_class(terms: Terms, declared: Declared) -> DataClass {
    let no_training = terms.trains_on_data == Some(false);
    let dpa = match terms.dpa {
        Dpa::Included => true,
        Dpa::Offered => declared.dpa_signed,
        Dpa::None | Dpa::Unknown => false,
    };
    if !declared.free_tier && no_training && dpa {
        DataClass::Business
    } else if no_training || declared.allows_training {
        DataClass::Personal
    } else {
        DataClass::Public
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAID: Terms = Terms {
        trains_on_data: Some(false),
        dpa: Dpa::Included,
    };
    const FREE_TRAINS: Terms = Terms {
        trains_on_data: Some(true),
        dpa: Dpa::None,
    };
    const UNKNOWN: Terms = Terms {
        trains_on_data: None,
        dpa: Dpa::Unknown,
    };

    #[test]
    fn a_paid_tier_with_a_dpa_and_no_training_may_receive_business_data() {
        assert_eq!(
            connection_class(PAID, Declared::default()),
            DataClass::Business
        );
        let offered = Terms {
            dpa: Dpa::Offered,
            ..PAID
        };
        assert_eq!(
            connection_class(offered, Declared::default()),
            DataClass::Personal,
            "a DPA one has to sign counts once signed"
        );
        let signed = Declared {
            dpa_signed: true,
            ..Declared::default()
        };
        assert_eq!(connection_class(offered, signed), DataClass::Business);
    }

    #[test]
    fn a_free_tier_never_receives_business_data() {
        // In the EEA, Google applies its paid data terms to the free quota.
        let eea_free = Declared {
            free_tier: true,
            ..Declared::default()
        };
        assert_eq!(connection_class(PAID, eea_free), DataClass::Personal);
        assert_eq!(connection_class(FREE_TRAINS, eea_free), DataClass::Public);
        let consent = Declared {
            free_tier: true,
            allows_training: true,
            ..Declared::default()
        };
        assert_eq!(
            connection_class(FREE_TRAINS, consent),
            DataClass::Personal,
            "the user's own data, with their consent"
        );
    }

    #[test]
    fn terms_not_confirmed_allow_only_public_data() {
        assert_eq!(
            connection_class(UNKNOWN, Declared::default()),
            DataClass::Public
        );
        assert!(may_receive(DataClass::Personal, DataClass::Public));
        assert!(!may_receive(DataClass::Personal, DataClass::Business));
        assert_eq!(
            highest(DataClass::Personal, DataClass::Business),
            DataClass::Business
        );
    }
}
