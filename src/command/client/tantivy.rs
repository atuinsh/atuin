use atuin_client::database::Database;
use clap::Subcommand;
use eyre::Result;

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    RefreshIndex,
    GarbageCollect,
}

impl Cmd {
    pub async fn run(self, db: &mut dyn Database) -> Result<()> {
        match self {
            Self::RefreshIndex => atuin_client::tantivy::refresh(db).await,
            Self::GarbageCollect => atuin_client::tantivy::garbage_collect().await,
        }
    }
}
