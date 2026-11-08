fn main() {
    println!("cargo:rerun-if-env-changed=CON_SPARKLE_PUBLIC_ED_KEY");
    println!("cargo:rerun-if-env-changed=CON_RELEASE_VERSION");
}
