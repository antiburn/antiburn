fn main() {
    // `analytics::config` reads these with `option_env!`, which is
    // resolved at compile time. Cargo does not track an environment variable
    // as an input unless it is told to, so without these two lines changing
    // the endpoint would leave a stale binary behind and the change would
    // look like it had no effect — the worst kind of build bug, because it
    // reads as a code problem.
    println!("cargo:rerun-if-env-changed=ANTIBURN_ANALYTICS_URL");
    println!("cargo:rerun-if-env-changed=ANTIBURN_ANALYTICS_OPERATOR");
    println!("cargo:rerun-if-env-changed=ANTIBURN_ANALYTICS_ENABLED");
    tauri_build::build();
}
