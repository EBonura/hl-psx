# Retail feature census

Run after every game landing, using the landed source checkout as PORT_REPO:

```sh
CARGO_TARGET_DIR=/absolute/private/census-target cargo run --release --manifest-path tools/census/Cargo.toml -- /absolute/retail/valve /absolute/port/repo /absolute/report-directory
```

The Rust tool reads all 103 canonical retail BSP entity lumps plus original UI
resources and HUD sprite declarations. It never reads original game source.
The exact retail file hashes, source revision, every entity/key and original
resource location are preserved in census.json. The ranked report includes
every missing/unverified or implemented-but-unexercised feature.

evidence.json is an explicit human-reviewed execution-path inventory. A source
symbol check only detects stale evidence; it never discovers implementation.
Unlisted features default to missing/unverified, which is a review task rather
than an assertion of absent code. Existing partial implementations do not prove
full parity. Coverage requires reviewed feature-specific assertion and report
hash, not a generic screenshot or successful boot. No existing journey has been
promoted to feature-specific coverage in this baseline.

Loose retail maps are required. An installation missing any canonical map fails
closed. UI data scope includes multiplayer resources but does not invent their
runtime behavior. Unreferenced model filenames do not imply supported actors.

```sh
CARGO_TARGET_DIR=/absolute/private/census-target cargo test --manifest-path tools/census/Cargo.toml
```

Implemented records describe the exact reviewed subset in their reason, not full
parity. Missing and unverified are separate counts. Named behavior gaps remain
in the queue even when a renderer or cook representation exists. Additional
execution-path files and primary source files are bound by SHA256; a changed
file invalidates that row to unverified, preserving a useful report after a
landing. Original entities absent from the103-map census cannot receive evidence.

Legacy retail bytes outside UTF8 map reversibly to Latin1 code points; the raw
file SHA256 remains authoritative. No original resource or BSP bytes are committed.

Coverage accepts only an externally audited JSON receipt with schema
`audited-feature-coverage-v1`, exact `source_revision`, `journey` and `disc`
objects containing absolute `path` and `sha256`, and `assertions` containing an
exact unique `name`, `status: PASS`, and matching feature-specific `observation`.
The registry binds that receipt's SHA256. Failed, duplicate, stale or generic
unstructured report mentions cannot exercise a feature. This baseline registers
no production coverage receipts. Synthetic passing/failing unit fixtures test
the validator and never enter production counts.
