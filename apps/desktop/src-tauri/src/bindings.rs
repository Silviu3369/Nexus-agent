//! TypeScript types for the webview, generated from the Rust contract so the
//! frontend never keeps a hand-written copy (`CONTRACTE_NEXUS_RO.md` §1).
//! Regenerate with: `NEXUS_UPDATE_BINDINGS=1 cargo test -p desktop`.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use specta::ts::{self, BigIntExportBehavior, ExportConfiguration};

    const HEADER: &str = "// Generated from Rust by `NEXUS_UPDATE_BINDINGS=1 cargo test -p desktop`. Do not edit.\n\n";

    fn bindings_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packages/contracts/src/bindings.ts")
    }

    fn render() -> String {
        // i64 fields crossing to the webview are unix-ms times, revisions and
        // event sequence numbers, all far below 2^53 (see nexus_protocol::runs);
        // identifiers and hashes are strings, so `number` is exact here.
        let conf = ExportConfiguration::default().bigint(BigIntExportBehavior::Number);
        let types = [
            ts::export::<nexus_protocol::RequestId>(&conf),
            ts::export::<nexus_protocol::OperationId>(&conf),
            ts::export::<nexus_protocol::TaskId>(&conf),
            ts::export::<nexus_protocol::RunId>(&conf),
            ts::export::<nexus_protocol::ApprovalId>(&conf),
            ts::export::<nexus_protocol::ArtifactId>(&conf),
            ts::export::<nexus_protocol::ErrorCode>(&conf),
            ts::export::<nexus_protocol::NexusError>(&conf),
            ts::export::<crate::host::HostStatus>(&conf),
            ts::export::<crate::host::BridgeError>(&conf),
            ts::export::<nexus_protocol::RunStatus>(&conf),
            ts::export::<nexus_protocol::NotePreview>(&conf),
            ts::export::<nexus_protocol::PendingApproval>(&conf),
            ts::export::<nexus_protocol::ArtifactSummary>(&conf),
            ts::export::<nexus_protocol::RunFailure>(&conf),
            ts::export::<nexus_protocol::RunOrigin>(&conf),
            ts::export::<nexus_protocol::RunView>(&conf),
            ts::export::<nexus_protocol::Snapshot>(&conf),
            ts::export::<nexus_protocol::SubmitNoteResponse>(&conf),
            ts::export::<nexus_protocol::BackupSummary>(&conf),
            ts::export::<nexus_protocol::EventView>(&conf),
            ts::export::<nexus_protocol::EventsPage>(&conf),
            ts::export::<nexus_protocol::SystemStatusView>(&conf),
            ts::export::<nexus_protocol::ConnectionId>(&conf),
            ts::export::<nexus_protocol::ConversationId>(&conf),
            ts::export::<nexus_protocol::MessageId>(&conf),
            ts::export::<nexus_protocol::ConnectionView>(&conf),
            ts::export::<nexus_protocol::ThinkingLevel>(&conf),
            ts::export::<nexus_protocol::ProviderChoice>(&conf),
            ts::export::<nexus_protocol::BackupModelView>(&conf),
            ts::export::<nexus_protocol::ModelView>(&conf),
            ts::export::<nexus_protocol::PersonaView>(&conf),
            ts::export::<nexus_protocol::ChatRole>(&conf),
            ts::export::<nexus_protocol::ChatStatus>(&conf),
            ts::export::<nexus_protocol::AgentStepId>(&conf),
            ts::export::<nexus_protocol::AgentStepStatus>(&conf),
            ts::export::<nexus_protocol::AgentStepDetail>(&conf),
            ts::export::<nexus_protocol::AgentStepView>(&conf),
            ts::export::<nexus_protocol::ChatMessageView>(&conf),
            ts::export::<nexus_protocol::ConversationSummary>(&conf),
            ts::export::<nexus_protocol::ConversationView>(&conf),
            ts::export::<nexus_protocol::SendMessageResponse>(&conf),
        ];
        let mut out = String::from(HEADER);
        for exported in types {
            out.push_str(&exported.expect("contract type must export to TypeScript"));
            out.push_str("\n\n");
        }
        out
    }

    #[test]
    fn typescript_bindings_are_up_to_date() {
        let path = bindings_path();
        let expected = render();

        if std::env::var_os("NEXUS_UPDATE_BINDINGS").is_some() {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &expected).unwrap();
            return;
        }

        let actual = fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            actual.replace("\r\n", "\n"),
            expected,
            "packages/contracts/src/bindings.ts is stale; run `NEXUS_UPDATE_BINDINGS=1 cargo test -p desktop`"
        );
    }
}
