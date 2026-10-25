//! What each AI provider's terms say about the data it receives
//! (ADR-PROVIDER-002): static, dated and with its sources, like the tool
//! registry. A fact not confirmed from the provider's own documents counts
//! as unknown, and a connection to it may then receive only public data.
//! Checked against the providers' own pages on 2026-10-25; check again
//! before a pilot.

use nexus_core::DataClass;
use nexus_core::data::{self, Declared, Dpa, Terms};
use nexus_providers::ProviderKind;

/// When the facts below were checked.
pub const CHECKED: &str = "2026-10-25";

/// A provider's data terms, by tier.
#[derive(Debug, Clone, Copy)]
pub struct ProviderFacts {
    pub provider: ProviderKind,
    /// The free tier, where there is one.
    pub free: Option<Terms>,
    pub paid: Terms,
    /// Its free tier follows the paid data terms for users in the EEA,
    /// Switzerland and the UK.
    pub eea_free_as_paid: bool,
    /// Where the terms are.
    pub source: &'static str,
}

pub static FACTS: &[ProviderFacts] = &[
    // Unpaid services: prompts and responses improve Google's products and
    // may be read by people; paid services: not used, under a DPA; in the
    // EEA, Switzerland and the UK the paid data terms apply to the free
    // quota too (terms effective 2026-03-23).
    ProviderFacts {
        provider: ProviderKind::Gemini,
        free: Some(Terms {
            trains_on_data: Some(true),
            dpa: Dpa::None,
        }),
        paid: Terms {
            trains_on_data: Some(false),
            dpa: Dpa::Included,
        },
        eea_free_as_paid: true,
        source: "https://ai.google.dev/gemini-api/terms",
    },
    // API data is not used for training by default; the DPA is signed
    // separately from the account.
    ProviderFacts {
        provider: ProviderKind::OpenAi,
        free: None,
        paid: Terms {
            trains_on_data: Some(false),
            dpa: Dpa::Offered,
        },
        eea_free_as_paid: false,
        source: "https://developers.openai.com/api/docs/guides/your-data",
    },
    // Anthropic may not train on customer content; the DPA is part of the
    // commercial terms.
    ProviderFacts {
        provider: ProviderKind::Anthropic,
        free: None,
        paid: Terms {
            trains_on_data: Some(false),
            dpa: Dpa::Included,
        },
        eea_free_as_paid: false,
        source: "https://www.anthropic.com/legal/commercial-terms",
    },
    // Not confirmed from DeepSeek's own documents (secondary sources say it
    // stores data in China and uses it to improve its models by default).
    ProviderFacts {
        provider: ProviderKind::DeepSeek,
        free: None,
        paid: Terms {
            trains_on_data: None,
            dpa: Dpa::Unknown,
        },
        eea_free_as_paid: false,
        source: "",
    },
];

pub fn facts(provider: ProviderKind) -> &'static ProviderFacts {
    FACTS
        .iter()
        .find(|f| f.provider == provider)
        .expect("every provider has its facts")
}

/// The terms that apply to a connection.
pub fn terms(provider: ProviderKind, free_tier: bool, in_eea: bool) -> Terms {
    let facts = facts(provider);
    match facts.free {
        Some(free) if free_tier && !(in_eea && facts.eea_free_as_paid) => free,
        _ => facts.paid,
    }
}

/// The highest class a connection may receive.
pub fn connection_class(provider: ProviderKind, declared: Declared, in_eea: bool) -> DataClass {
    data::connection_class(terms(provider, declared.free_tier, in_eea), declared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free() -> Declared {
        Declared {
            free_tier: true,
            ..Declared::default()
        }
    }

    #[test]
    fn every_provider_has_its_facts() {
        for provider in ProviderKind::ALL {
            let f = facts(provider);
            assert!(
                f.paid.trains_on_data.is_none() || f.source.starts_with("https://"),
                "a confirmed fact names its source: {provider:?}"
            );
        }
    }

    #[test]
    fn what_each_connection_may_receive() {
        use DataClass::*;
        let paid = Declared::default();
        // Gemini: free quota from the EEA gets the paid data terms, but a
        // free tier never receives someone else's data.
        assert_eq!(
            connection_class(ProviderKind::Gemini, free(), false),
            Public
        );
        assert_eq!(
            connection_class(ProviderKind::Gemini, free(), true),
            Personal
        );
        assert_eq!(
            connection_class(ProviderKind::Gemini, paid, false),
            Business
        );
        // OpenAI: business data once the user signed the DPA.
        assert_eq!(
            connection_class(ProviderKind::OpenAi, paid, false),
            Personal
        );
        let signed = Declared {
            dpa_signed: true,
            ..Declared::default()
        };
        assert_eq!(
            connection_class(ProviderKind::OpenAi, signed, false),
            Business
        );
        assert_eq!(
            connection_class(ProviderKind::Anthropic, paid, false),
            Business
        );
        // DeepSeek: not confirmed, so public only, unless the user lets it
        // use their own data.
        assert_eq!(connection_class(ProviderKind::DeepSeek, paid, true), Public);
        let consent = Declared {
            allows_training: true,
            ..Declared::default()
        };
        assert_eq!(
            connection_class(ProviderKind::DeepSeek, consent, false),
            Personal
        );
    }
}
