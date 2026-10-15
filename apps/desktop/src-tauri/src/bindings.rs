//! TypeScript types for the webview, generated from the Rust contract so the
//! frontend never keeps a hand-written copy (`CONTRACTE_NEXUS_RO.md` §1).
//! Regenerate with: `NEXUS_UPDATE_BINDINGS=1 cargo test -p desktop`.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use specta::ts::{self, ExportConfiguration};

    const HEADER: &str = "// Generated from Rust by `NEXUS_UPDATE_BINDINGS=1 cargo test -p desktop`. Do not edit.\n\n";

    fn bindings_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../packages/contracts/src/bindings.ts")
    }

    fn render() -> String {
        let conf = ExportConfiguration::default();
        let types = [
            ts::export::<nexus_protocol::RequestId>(&conf),
            ts::export::<nexus_protocol::OperationId>(&conf),
            ts::export::<nexus_protocol::ErrorCode>(&conf),
            ts::export::<nexus_protocol::NexusError>(&conf),
            ts::export::<crate::host::HostStatus>(&conf),
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
