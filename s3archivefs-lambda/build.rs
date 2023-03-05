fn main() {
    println!("cargo:rustc-link-search=/usr/local/lib");
    println!("cargo:rustc-link-arg=-L/usr/local/lib");
    println!("cargo:rustc-link-lib=squashfs");
}
