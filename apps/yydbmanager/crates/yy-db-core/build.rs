fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Bundled DuckDB uses Restart Manager to describe file-lock owners.
        // Link it explicitly instead of relying on unrelated driver dependencies.
        println!("cargo:rustc-link-lib=Rstrtmgr");
    }
}
