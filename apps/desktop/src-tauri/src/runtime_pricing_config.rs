//! Fixed public source for the runtime model catalog.

/// The public models.dev catalog endpoint.
pub struct CatalogEndpoint {
    scheme: &'static str,
    authority: &'static str,
    path: &'static str,
}

impl CatalogEndpoint {
    pub fn url(&self) -> String {
        format!("{}://{}{}", self.scheme, self.authority, self.path)
    }
}

pub const MODELS_DEV: CatalogEndpoint = CatalogEndpoint {
    scheme: "https",
    authority: "models.dev",
    path: "/api.json",
};
