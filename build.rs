use std::env;

fn main() {
	let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
	println!("cargo:ui_src={manifest_dir}/src/ui.rs");
}
