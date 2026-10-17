//! Which provider an API key belongs to, read from the key's own format,
//! locally. A key is never "tried" against several providers to find out:
//! that would hand, say, an OpenAI key to DeepSeek. When the format does not
//! settle it, the user chooses; the chosen provider then verifies the key.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProviderKind {
    Gemini,
    OpenAi,
    Anthropic,
    DeepSeek,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 4] = [
        ProviderKind::Gemini,
        ProviderKind::OpenAi,
        ProviderKind::Anthropic,
        ProviderKind::DeepSeek,
    ];

    /// The stored value (`provider_connections.provider`).
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Gemini => "Gemini",
            ProviderKind::OpenAi => "OpenAI",
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::DeepSeek => "DeepSeek",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == value)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ProviderKind::Gemini => "Google Gemini",
            ProviderKind::OpenAi => "OpenAI",
            ProviderKind::Anthropic => "Anthropic (Claude)",
            ProviderKind::DeepSeek => "DeepSeek",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyDetection {
    Known(ProviderKind),
    /// The format fits more than one provider.
    Ambiguous(Vec<ProviderKind>),
    Unknown,
}

/// Reads the provider from the published key formats: Google API keys
/// start with `AIza`; Anthropic keys with `sk-ant-`; OpenAI project,
/// service-account and admin keys with `sk-proj-`, `sk-svcacct-`,
/// `sk-admin-`, and its older user keys carry `T3BlbkFJ`; DeepSeek keys
/// are `sk-` followed by 32 hex digits. Any other `sk-` key could be
/// either OpenAI or DeepSeek.
pub fn detect_provider(key: &str) -> KeyDetection {
    let key = key.trim();
    if key.starts_with("AIza") {
        return KeyDetection::Known(ProviderKind::Gemini);
    }
    if key.starts_with("sk-ant-") {
        return KeyDetection::Known(ProviderKind::Anthropic);
    }
    if ["sk-proj-", "sk-svcacct-", "sk-admin-"]
        .iter()
        .any(|p| key.starts_with(p))
        || (key.starts_with("sk-") && key.contains("T3BlbkFJ"))
    {
        return KeyDetection::Known(ProviderKind::OpenAi);
    }
    if let Some(rest) = key.strip_prefix("sk-") {
        if rest.len() == 32 && rest.chars().all(|c| c.is_ascii_hexdigit()) {
            return KeyDetection::Known(ProviderKind::DeepSeek);
        }
        return KeyDetection::Ambiguous(vec![ProviderKind::OpenAi, ProviderKind::DeepSeek]);
    }
    KeyDetection::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_formats_are_recognised() {
        use KeyDetection::Known;
        assert_eq!(
            detect_provider("AIzaSyD-example_123"),
            Known(ProviderKind::Gemini)
        );
        assert_eq!(
            detect_provider("sk-ant-api03-abc"),
            Known(ProviderKind::Anthropic)
        );
        assert_eq!(detect_provider("sk-proj-abc"), Known(ProviderKind::OpenAi));
        assert_eq!(
            detect_provider("sk-svcacct-abc"),
            Known(ProviderKind::OpenAi)
        );
        assert_eq!(
            detect_provider("sk-abcT3BlbkFJxyz"),
            Known(ProviderKind::OpenAi)
        );
        assert_eq!(
            detect_provider("sk-0123456789abcdef0123456789abcdef"),
            Known(ProviderKind::DeepSeek)
        );
        assert_eq!(detect_provider("  AIzaX  "), Known(ProviderKind::Gemini));
    }

    #[test]
    fn unclear_formats_are_left_to_the_user() {
        assert_eq!(
            detect_provider("sk-something-else"),
            KeyDetection::Ambiguous(vec![ProviderKind::OpenAi, ProviderKind::DeepSeek])
        );
        assert_eq!(detect_provider("hello"), KeyDetection::Unknown);
        assert_eq!(detect_provider(""), KeyDetection::Unknown);
    }

    #[test]
    fn stored_names_round_trip() {
        for kind in ProviderKind::ALL {
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ProviderKind::parse("gemini"), None);
    }
}
