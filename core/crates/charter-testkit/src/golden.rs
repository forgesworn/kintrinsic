//! Golden-vector loader scaffold. Vectors live under `charter-testkit/vectors/`
//! and are generated (never hand-written hex) in Phases 1, 2, and 6. The
//! `CARGO_MANIFEST_DIR` is baked at this crate's compile time, so consumers in
//! other crates still resolve the same `vectors/` directory.

use std::path::PathBuf;

/// The absolute path to the testkit `vectors/` directory.
pub fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vectors")
}

/// Read a vector file as a string.
pub fn load_string(name: &str) -> std::io::Result<String> {
    std::fs::read_to_string(vectors_dir().join(name))
}

/// Read and deserialize a JSON vector file. Panics with a clear message if the
/// vector is missing or malformed — generators must keep these in sync.
pub fn load_json<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let text = load_string(name)
        .unwrap_or_else(|e| panic!("missing golden vector {name}: {e} (run the generator)"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("malformed golden vector {name}: {e}"))
}

/// Whether a vector file exists (lets a suite skip cleanly until generated).
pub fn vector_exists(name: &str) -> bool {
    vectors_dir().join(name).exists()
}
