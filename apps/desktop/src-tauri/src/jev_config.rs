const SYSTEM_ONE_PROTOCOL: &str = "https://";
const SYSTEM_ONE_HOSTNAME: &str = "api.typesafe.ai";
const SYSTEM_ONE_API_PATH: &str = "/v1/systemone";

pub(crate) fn system_one_endpoint() -> String {
    format!("{SYSTEM_ONE_PROTOCOL}{SYSTEM_ONE_HOSTNAME}{SYSTEM_ONE_API_PATH}")
}
