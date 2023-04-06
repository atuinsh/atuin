
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn index() -> Json<IndexResponse> {
    Json(IndexResponse {
        version: VERSION.to_string(),
    })
}
