// Throwaway: asks a running daemon's Search service what its in-memory index actually holds.
// Same socket + service the interactive TUI uses, minus the terminal rendering that hides
// the answer behind a database fallback. Not part of the build; delete when done.
use atuin_client::database::current_context;
use atuin_client::settings::Settings;
use atuin_daemon::client::SearchClient;
use atuin_client::settings::FilterMode;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let sock = std::env::args().nth(1).expect("usage: probe_index <socket> <query>");
    let query = std::env::args().nth(2).unwrap_or_default();

    // Settings::new() populates the global config the context helper reads.
    let _settings = Settings::new()?;
    let ctx = current_context().await?;
    let mut client = SearchClient::new(sock).await?;
    let mut stream = client.search(query.clone(), 1, FilterMode::Global, Some(ctx)).await?;

    if let Some(resp) = stream.message().await? {
        println!("INDEX_HITS query={query:?} count={}", resp.ids.len());
        for id in resp.ids.iter().take(10) {
            println!("  - {}", uuid::Uuid::from_slice(id)
                .map(|u| u.as_simple().to_string())
                .unwrap_or_else(|_| format!("{id:?}")));
        }
    } else {
        println!("INDEX_HITS query={query:?} count=0 (no response)");
    }
    Ok(())
}
