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
            ts::export::<crate::autostart::AutostartView>(&conf),
            ts::export::<nexus_protocol::RunStatus>(&conf),
            ts::export::<nexus_protocol::NotePreview>(&conf),
            ts::export::<nexus_protocol::McpCallPreview>(&conf),
            ts::export::<nexus_protocol::PendingApproval>(&conf),
            ts::export::<nexus_protocol::ArtifactSummary>(&conf),
            ts::export::<nexus_protocol::RunFailure>(&conf),
            ts::export::<nexus_protocol::RunOrigin>(&conf),
            ts::export::<nexus_protocol::RunKind>(&conf),
            ts::export::<nexus_protocol::McpCallView>(&conf),
            ts::export::<nexus_protocol::ActorView>(&conf),
            ts::export::<nexus_protocol::RecordChangePreview>(&conf),
            ts::export::<nexus_protocol::PermissionView>(&conf),
            ts::export::<nexus_protocol::ConnectionDataView>(&conf),
            ts::export::<nexus_protocol::SourceDataView>(&conf),
            ts::export::<nexus_protocol::DataPolicyView>(&conf),
            ts::export::<nexus_protocol::DataChange>(&conf),
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
            ts::export::<nexus_protocol::DeletedView>(&conf),
            ts::export::<nexus_protocol::ProjectId>(&conf),
            ts::export::<nexus_protocol::MemoryId>(&conf),
            ts::export::<nexus_protocol::MemoryScope>(&conf),
            ts::export::<nexus_protocol::MemoryStatus>(&conf),
            ts::export::<nexus_protocol::MemorySource>(&conf),
            ts::export::<nexus_protocol::ProjectView>(&conf),
            ts::export::<nexus_protocol::MemoryItemView>(&conf),
            ts::export::<nexus_protocol::MemoryContextView>(&conf),
            ts::export::<nexus_protocol::McpServerId>(&conf),
            ts::export::<nexus_protocol::McpToolView>(&conf),
            ts::export::<nexus_protocol::McpServerView>(&conf),
            ts::export::<nexus_protocol::ModelPrice>(&conf),
            ts::export::<nexus_protocol::ModelUsageView>(&conf),
            ts::export::<nexus_protocol::UsageView>(&conf),
            ts::export::<nexus_protocol::AutomationId>(&conf),
            ts::export::<nexus_protocol::RegisterId>(&conf),
            ts::export::<nexus_protocol::RecordId>(&conf),
            ts::export::<nexus_protocol::FieldKind>(&conf),
            ts::export::<nexus_protocol::FieldDef>(&conf),
            ts::export::<nexus_protocol::RegisterView>(&conf),
            ts::export::<nexus_protocol::RecordActor>(&conf),
            ts::export::<nexus_protocol::RecordChangeKind>(&conf),
            ts::export::<nexus_protocol::RecordView>(&conf),
            ts::export::<nexus_protocol::FieldChange>(&conf),
            ts::export::<nexus_protocol::RecordChangeView>(&conf),
            ts::export::<nexus_protocol::RecordDetailView>(&conf),
            ts::export::<nexus_protocol::RecordQuery>(&conf),
            ts::export::<nexus_protocol::SaveRecordRequest>(&conf),
            ts::export::<nexus_protocol::FolderId>(&conf),
            ts::export::<nexus_protocol::FolderView>(&conf),
            ts::export::<nexus_protocol::SaveFolderRequest>(&conf),
            ts::export::<nexus_protocol::FolderEntryView>(&conf),
            ts::export::<nexus_protocol::SkillId>(&conf),
            ts::export::<nexus_protocol::SkillView>(&conf),
            ts::export::<nexus_protocol::SkillFileView>(&conf),
            ts::export::<nexus_protocol::SkillContentView>(&conf),
            ts::export::<nexus_protocol::SkillFileText>(&conf),
            ts::export::<nexus_protocol::SetSkillEnabledRequest>(&conf),
            ts::export::<nexus_protocol::ExportView>(&conf),
            ts::export::<nexus_protocol::ImportPreviewView>(&conf),
            ts::export::<nexus_protocol::ImportRequest>(&conf),
            ts::export::<nexus_protocol::ImportSkipView>(&conf),
            ts::export::<nexus_protocol::ImportResultView>(&conf),
            ts::export::<nexus_protocol::InboxFileStatus>(&conf),
            ts::export::<nexus_protocol::InboxFileView>(&conf),
            ts::export::<nexus_protocol::InboxView>(&conf),
            ts::export::<nexus_protocol::WeekCountView>(&conf),
            ts::export::<nexus_protocol::ScoreBand>(&conf),
            ts::export::<nexus_protocol::GroupCountView>(&conf),
            ts::export::<nexus_protocol::ScoreBandView>(&conf),
            ts::export::<nexus_protocol::SourceCountView>(&conf),
            ts::export::<nexus_protocol::CauseCountView>(&conf),
            ts::export::<nexus_protocol::Acceptance>(&conf),
            ts::export::<nexus_protocol::TurnView>(&conf),
            ts::export::<nexus_protocol::AdsHandlingView>(&conf),
            ts::export::<nexus_protocol::JobResultsView>(&conf),
            ts::export::<nexus_protocol::SaveInboxRequest>(&conf),
            ts::export::<nexus_protocol::Schedule>(&conf),
            ts::export::<nexus_protocol::AutomationRunStatus>(&conf),
            ts::export::<nexus_protocol::AutomationRunView>(&conf),
            ts::export::<nexus_protocol::AutomationView>(&conf),
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
