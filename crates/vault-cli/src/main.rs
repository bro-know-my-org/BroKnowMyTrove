fn main() {
    clap::Command::new("bkm")
        .version(env!("CARGO_PKG_VERSION"))
        .about(vault_core::PRODUCT_NAME)
        .after_help(vault_core::DEVELOPMENT_STATUS)
        .get_matches();

    println!(
        "{}\n{}",
        vault_core::PRODUCT_NAME,
        vault_core::DEVELOPMENT_STATUS
    );
}
