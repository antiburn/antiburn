fn main() {
    println!(
        "cargo:rustc-env=ANTIBURN_CLI_RUNTIME_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
}
