#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::try_init().ok();

    let pairing_token = nexus_protocol::pairing::load_or_create_token()?;
    let name = nexus_protocol::pairing::pipe_name();

    let listener = nexus_host::bind_first_instance(&name)?;
    tracing::info!(pipe = %name, "nexus-host listening");

    nexus_host::serve_forever(listener, name, pairing_token).await
}
