use std::path::PathBuf;
use ui_metadata_validator::validate_manifests;

fn main() {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    match validate_manifests(&paths) {
        Ok(summary) => println!(
            "validated {} manifest(s): {} resource(s), {} page(s), {} fragment(s), {} extension(s), {} component(s)",
            summary.manifests,
            summary.resources,
            summary.pages,
            summary.fragments,
            summary.extensions,
            summary.components
        ),
        Err(error) => {
            eprintln!("UI metadata validation failed: {error}");
            std::process::exit(1);
        }
    }
}
