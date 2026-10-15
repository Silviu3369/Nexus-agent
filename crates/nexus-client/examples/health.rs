//! Dev probe, not shipped: `cargo run -p nexus-client --example health`.
//! Connects to the running host as an independent client and prints its
//! identity and uptime — used to show the host outlives the dashboard.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (client, handshake) = nexus_client::NexusClient::connect("nexus-probe").await?;
    let health = client.get_health().await??;
    println!(
        "host={} protocol={} status={:?} uptime_seconds={}",
        handshake.host_identity,
        handshake.server_protocol_version,
        health.status,
        health.uptime_seconds
    );
    Ok(())
}
