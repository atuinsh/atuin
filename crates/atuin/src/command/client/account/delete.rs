use atuin_client::{api_client, settings::Settings};
use eyre::{Result, bail};

pub async fn run(settings: &Settings) -> Result<()> {
    if let Some(endpoint) = settings.active_hub_endpoint() {
        match settings.hub_session_token().await {
            Ok(_) => {
                println!("You are authenticated with Atuin Hub.");
                println!("Delete your account on Atuin Hub: {endpoint}/settings/account");
                return Ok(());
            }
            Err(_) => {
                println!("You are not currently logged in to Atuin Hub.");
                println!(
                    "Run 'atuin login' to log in to Atuin Hub, or visit {endpoint}/settings/account to delete your account."
                );
                return Ok(());
            }
        }
    }

    if !settings.logged_in().await? {
        bail!("You are not logged in");
    }

    let client = api_client::Client::new(
        &settings.sync_address,
        settings.sync_auth_token().await?,
        settings.network_connect_timeout,
        settings.network_timeout,
    )?;

    client.delete().await?;

    // Clean up session from meta store
    Settings::meta_store().await?.delete_session().await?;

    println!("Your account is deleted");

    Ok(())
}
