use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

const TESSDATA_FAST_REVISION: &str = "87416418657359cb625c412a48b6e1d6d41c29bd";
const TESSDATA: &[(&str, u64, &str)] = &[
    (
        "eng",
        4_113_088,
        "7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2",
    ),
    (
        "deu",
        1_525_436,
        "19d219bbb6672c869d20a9636c6816a81eb9a71796cb93ebe0cb1530e2cdb22d",
    ),
    (
        "fra",
        1_130_365,
        "ced037562e8c80c13122dece28dd477d399af80911a28791a66a63ac1e3445ca",
    ),
    (
        "spa",
        2_294_433,
        "6f2e04d02774a18f01bed44b1111f2cd7f3ba7ac9dc4373cd3f898a40ea6b464",
    ),
    (
        "ita",
        2_701_314,
        "b8f89e1e785118dac4d51ae042c029a64edb5c3ee42ef73027a6d412748d8827",
    ),
    (
        "nld",
        6_050_296,
        "ced0e5e046a84c908a6aa7accbef9a232c4a5d9a8276691b81c6ee64d02963f6",
    ),
    (
        "pol",
        4_765_518,
        "c4476cdbc0e33d898d32345122b7be1cbf85ace15f920f06c7714756e1ef79b2",
    ),
];

fn main() {
    println!("cargo:rerun-if-changed=schema/basemind-config-v1.schema.json");
    println!("cargo:rerun-if-changed=src/queries");
    println!("cargo:rerun-if-env-changed=TESSDATA_PREFIX");

    // Keep the default/code-only builds free of OCR data. Full document builds embed the
    // language packs into the binary so first use never depends on a network download.
    if std::env::var_os("CARGO_FEATURE_DOCUMENTS").is_none() {
        return;
    }

    let out_dir = std::env::var_os("OUT_DIR").expect("Cargo always sets OUT_DIR");
    let out_dir = Path::new(&out_dir);
    for &(language, expected_size, expected_sha256) in TESSDATA {
        let path = out_dir.join(format!("{language}.traineddata"));
        let valid = fs::read(&path)
            .map(|bytes| verify(&bytes, expected_size, expected_sha256).is_ok())
            .unwrap_or(false);
        if !valid {
            if let Some(local) = local_tessdata_path(language) {
                fs::copy(&local, &path).unwrap_or_else(|error| {
                    panic!("copy local {language}.traineddata from {}: {error}", local.display())
                });
            } else {
                let url = format!(
                    "https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/{TESSDATA_FAST_REVISION}/{language}.traineddata"
                );
                let response = ureq::get(&url)
                    .call()
                    .unwrap_or_else(|error| panic!("download {language}.traineddata from {url}: {error}"));
                let bytes = response
                    .into_body()
                    .with_config()
                    .limit(expected_size.saturating_add(1))
                    .read_to_vec()
                    .unwrap_or_else(|error| panic!("read {language}.traineddata from {url}: {error}"));
                verify(&bytes, expected_size, expected_sha256)
                    .unwrap_or_else(|error| panic!("invalid {language}.traineddata from {url}: {error}"));
                fs::write(&path, bytes).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
            }
        }

        let variable = format!("BASEMIND_BUNDLED_TESSDATA_{}", language.to_ascii_uppercase());
        println!("cargo:rustc-env={variable}={}", path.display());
    }
}

/// Find a locally installed, hash-verified language file before using the network fallback.
fn local_tessdata_path(language: &str) -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Some(prefix) = std::env::var_os("TESSDATA_PREFIX") {
        let prefix = std::path::PathBuf::from(prefix);
        candidates.push(prefix.join(format!("{language}.traineddata")));
        candidates.push(prefix.join("tessdata").join(format!("{language}.traineddata")));
    }
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    candidates.extend([
        manifest_dir.join("tessdata").join(format!("{language}.traineddata")),
        std::path::PathBuf::from("/usr/share/tesseract-ocr/5/tessdata").join(format!("{language}.traineddata")),
        std::path::PathBuf::from("/usr/share/tessdata").join(format!("{language}.traineddata")),
        std::path::PathBuf::from("/opt/homebrew/share/tessdata").join(format!("{language}.traineddata")),
        std::path::PathBuf::from("/usr/local/share/tessdata").join(format!("{language}.traineddata")),
    ]);
    candidates.into_iter().find(|candidate| {
        fs::read(candidate)
            .map(|bytes| {
                TESSDATA
                    .iter()
                    .find(|entry| entry.0 == language)
                    .is_some_and(|(_, size, sha)| verify(&bytes, *size, sha).is_ok())
            })
            .unwrap_or(false)
    })
}

fn verify(bytes: &[u8], expected_size: u64, expected_sha256: &str) -> Result<(), String> {
    if bytes.len() as u64 != expected_size {
        return Err(format!("expected {expected_size} bytes, got {}", bytes.len()));
    }
    let digest = Sha256::digest(bytes);
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if actual != expected_sha256 {
        return Err(format!("expected SHA-256 {expected_sha256}, got {actual}"));
    }
    Ok(())
}
