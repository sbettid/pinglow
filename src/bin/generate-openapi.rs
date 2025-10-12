use pinglow::api::ApiDoc;
use utoipa::OpenApi;

fn main() {
    let mut apidoc = ApiDoc::openapi();
    apidoc.info.version = env!("CARGO_PKG_VERSION").to_string();

    let json = apidoc.to_json().unwrap();
    std::fs::write("docs/static/openapi.json", json).unwrap();
}
