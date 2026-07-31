#[cfg(feature = "remote")]
pub fn client() -> axoasset::AxoClient {
    axoasset::AxoClient::with_reqwest(reqwest::ClientBuilder::new().build().unwrap())
}
