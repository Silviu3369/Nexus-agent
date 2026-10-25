//! What data may leave for which provider, as one reply applies it
//! (ADR-PROVIDER-002). A connection may receive up to a data class; the
//! turn holds the user's own data from its first message and someone
//! else's once it reads it. A source above what the connection may receive
//! is refused before it is read, and a model call (the backup model's too)
//! is made only if its connection may receive what the turn holds.

use nexus_core::DataClass;
use nexus_core::data::{Declared, Dpa, may_receive};
use nexus_protocol::{
    ApprovalPath, ConnectionDataView, DataChange, DataPolicyView, ErrorCode, FolderId, McpServerId,
    NexusError, RegisterId, SetDataPolicyRequest, SourceDataView,
};
use nexus_providers::ProviderKind;
use nexus_storage::{ConnectionData, ConnectionRow, DataSettings, Source, now_ms};
use serde_json::{Value, json};

use super::{Runtime, to_nexus_error};
use crate::data_terms;

/// The highest class a connection may receive.
pub(super) fn allowed(row: &ConnectionRow, data: &DataSettings) -> DataClass {
    let Some(kind) = ProviderKind::parse(&row.provider) else {
        return DataClass::Public;
    };
    let said = data.connection(row.id);
    data_terms::connection_class(
        kind,
        Declared {
            free_tier: row.free_tier,
            dpa_signed: said.dpa_signed,
            allows_training: said.allows_training,
        },
        data.in_eea,
    )
}

/// What a turn holds: the user's own data, or someone else's too.
pub(super) fn held(business: bool) -> DataClass {
    if business {
        DataClass::Business
    } else {
        DataClass::Personal
    }
}

fn words(class: DataClass) -> &'static str {
    match class {
        DataClass::Public => "only public data",
        DataClass::Personal => "your own data, but not other people's",
        DataClass::Business => "other people's data too",
    }
}

/// The reply cannot go to this connection: it holds more than it may
/// receive.
pub(super) fn refused_reply(
    row: &ConnectionRow,
    data: &DataSettings,
    allowed: DataClass,
    held: DataClass,
) -> NexusError {
    let what = match held {
        DataClass::Business => "other people's data (a source you marked Business)",
        _ => "your own data (the conversation itself)",
    };
    NexusError::new(
        ErrorCode::Forbidden,
        format!(
            "This task holds {what}, and the connection “{}” may receive {}. {} (Models → Data sent to AI providers)",
            row.display_name,
            words(allowed),
            reason(row, data, allowed)
        ),
    )
}

/// Whether the reply may use a source of `class`.
pub(super) fn may_use(allowed: DataClass, class: DataClass) -> bool {
    may_receive(allowed, class)
}

/// What the model is told when a source is above what the connection may
/// receive.
pub(super) fn refused_source(name: &str, allowed: DataClass) -> Value {
    json!({
        "error": format!(
            "\"{name}\" holds other people's data (the user marked it Business), and the AI connection in use may receive {}. It was not read; tell the user.",
            words(allowed)
        )
    })
}

/// Why a connection may receive what it may, in plain words.
pub(super) fn reason(row: &ConnectionRow, data: &DataSettings, allowed: DataClass) -> String {
    let Some(kind) = ProviderKind::parse(&row.provider) else {
        return "Nexus does not know this provider's terms: only public data.".into();
    };
    let facts = data_terms::facts(kind);
    let said = data.connection(row.id);
    let eea_free = row.free_tier && facts.eea_free_as_paid && data.in_eea;
    match allowed {
        DataClass::Business => format!(
            "Paid, under a data processing addendum; {} does not use what it receives to improve its models.",
            kind.display_name()
        ),
        DataClass::Personal if said.allows_training && !eea_free && facts.paid.trains_on_data != Some(false) => format!(
            "You let {} use your own data to improve its models; other people's data needs a paid connection with a data processing addendum.",
            kind.display_name()
        ),
        DataClass::Personal if eea_free => "Free quota used from the EEA, Switzerland or the UK: Google applies its paid data terms, so it does not use your data to improve its products. A free tier never receives other people's data.".into(),
        DataClass::Personal if facts.paid.dpa == Dpa::Offered => format!(
            "{} does not use API data for training. To send other people's data, sign its data processing addendum from your account and say so here.",
            kind.display_name()
        ),
        DataClass::Personal => format!(
            "{} does not use what it receives to improve its models; other people's data needs a paid connection.",
            kind.display_name()
        ),
        DataClass::Public if row.free_tier && facts.eea_free_as_paid => "Free quota: Google may use what it receives to improve its products, and people may read it. If you use Nexus from the EEA, Switzerland or the UK, say so here: Google then applies its paid data terms. Or allow it to use your data.".into(),
        DataClass::Public => format!(
            "{}'s terms are not confirmed from its own documents: only public data, unless you let it use your data.",
            kind.display_name()
        ),
    }
}

fn invalid(message: &str) -> NexusError {
    NexusError::new(ErrorCode::InvalidRequest, message.to_string())
}

impl Runtime {
    /// What data may leave for which connection, and why.
    pub async fn data_policy(&self) -> Result<DataPolicyView, NexusError> {
        let data = self.store.data_settings().await.map_err(to_nexus_error)?;
        let connections = self
            .store
            .connections()
            .await
            .map_err(to_nexus_error)?
            .iter()
            .map(|row| {
                let can = allowed(row, &data);
                let facts = ProviderKind::parse(&row.provider).map(data_terms::facts);
                let said = data.connection(row.id);
                ConnectionDataView {
                    connection_id: row.id,
                    display_name: row.display_name.clone(),
                    provider: row.provider.clone(),
                    free_tier: row.free_tier,
                    can_receive: can.as_str().to_string(),
                    reason: reason(row, &data, can),
                    terms_url: facts.map(|f| f.source.to_string()).unwrap_or_default(),
                    dpa: format!("{:?}", facts.map_or(Dpa::Unknown, |f| f.paid.dpa)),
                    dpa_signed: said.dpa_signed,
                    allows_training: said.allows_training,
                }
            })
            .collect();
        let mut sources = Vec::new();
        for f in self.store.folders().await.map_err(to_nexus_error)? {
            sources.push(SourceDataView {
                kind: "Folder".into(),
                id: f.id.to_string(),
                name: f.label,
                data_class: data.class_of(Source::Folder(f.id)).as_str().into(),
            });
        }
        for r in self.store.registers().await.map_err(to_nexus_error)? {
            sources.push(SourceDataView {
                kind: "Register".into(),
                id: r.id.to_string(),
                name: r.name,
                data_class: data.class_of(Source::Register(r.id)).as_str().into(),
            });
        }
        for m in self.store.mcp_servers().await.map_err(to_nexus_error)? {
            sources.push(SourceDataView {
                kind: "McpServer".into(),
                id: m.id.to_string(),
                name: m.name,
                data_class: data.class_of(Source::McpServer(m.id)).as_str().into(),
            });
        }
        Ok(DataPolicyView {
            in_eea: data.in_eea,
            terms_checked: data_terms::CHECKED.to_string(),
            connections,
            sources,
        })
    }

    /// One change. Letting more data leave (the EEA said, a DPA signed,
    /// consent given, a source back to the user's own) is accepted only
    /// from the native Windows dialog; the window can only tighten.
    pub async fn set_data_policy(
        &self,
        request: SetDataPolicyRequest,
    ) -> Result<DataPolicyView, NexusError> {
        let data = self.store.data_settings().await.map_err(to_nexus_error)?;
        let trusted = request.via == ApprovalPath::TrustedDialog;
        let refuse = || {
            NexusError::new(
                ErrorCode::Forbidden,
                "letting more data leave for an AI provider is confirmed in the Windows dialog",
            )
        };
        let now = now_ms();
        match request.change {
            DataChange::InEea { in_eea } => {
                if in_eea && !data.in_eea && !trusted {
                    return Err(refuse());
                }
                self.store.set_in_eea(in_eea, now).await
            }
            DataChange::Connection {
                connection_id,
                dpa_signed,
                allows_training,
            } => {
                let before = data.connection(connection_id);
                if ((dpa_signed && !before.dpa_signed)
                    || (allows_training && !before.allows_training))
                    && !trusted
                {
                    return Err(refuse());
                }
                self.store
                    .set_connection_data(
                        connection_id,
                        ConnectionData {
                            dpa_signed,
                            allows_training,
                        },
                        now,
                    )
                    .await
            }
            DataChange::Source {
                kind,
                id,
                data_class,
            } => {
                let uuid = uuid::Uuid::parse_str(&id)
                    .map_err(|_| invalid("the source's id is unreadable"))?;
                let source = match kind.as_str() {
                    "Folder" => Source::Folder(FolderId(uuid)),
                    "Register" => Source::Register(RegisterId(uuid)),
                    "McpServer" => Source::McpServer(McpServerId(uuid)),
                    _ => return Err(invalid("a source is a Folder, a Register or an McpServer")),
                };
                let class: DataClass = data_class
                    .parse()
                    .ok()
                    .filter(|c| *c != DataClass::Public)
                    .ok_or_else(|| invalid("a source is Personal or Business"))?;
                if class == DataClass::Personal
                    && data.class_of(source) == DataClass::Business
                    && !trusted
                {
                    return Err(refuse());
                }
                self.store.set_source_class(source, class, now).await
            }
        }
        .map_err(to_nexus_error)?;
        self.data_policy().await
    }
}
