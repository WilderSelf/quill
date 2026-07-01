//! Quill desktop application (GUI shell).
//!
//! The editor UI (an `egui` shell hosting a Skia document canvas) is not built yet. This
//! placeholder keeps the binary target in the workspace and prints basic document info so the
//! app crate is wired to the core model.

use quill_core_model::Document;

fn main() {
    let doc = Document::sample();
    println!("Quill (pre-alpha) — GUI not yet implemented.");
    println!(
        "Loaded sample document: \"{}\" ({} blocks).",
        doc.metadata.title,
        doc.content.len()
    );
}
