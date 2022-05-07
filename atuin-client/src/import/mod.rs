use std::io;

use async_trait::async_trait;
use eyre::Result;

use crate::history::History;

pub mod bash;
// pub mod fish;
// pub mod resh;
// pub mod zsh;

fn count_lines(input: &[u8]) -> usize {
    memchr::memchr_iter(b'\n', input).count()
}

#[async_trait]
pub trait Importer: Sized {
    const NAME: &'static str;
    async fn new() -> io::Result<Self>;
    async fn entries(&mut self) -> Result<usize>;
    async fn load(self, loader: &mut impl Loader) -> Result<()>;
}

#[async_trait]
pub trait Loader: Sync + Send {
    async fn push(&mut self, hist: History) -> eyre::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    pub struct TestLoader {
        pub buf: Vec<History>,
    }

    #[async_trait]
    impl Loader for TestLoader {
        async fn push(&mut self, hist: History) -> Result<()> {
            self.buf.push(hist);
            Ok(())
        }
    }
}
