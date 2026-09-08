//! `basemind cpu-features` — detect CPU feature flags for ONNX model compatibility.
//!
//! Standalone utility: prints JSON to stdout and exits. No server, no MCP dispatch.

use std::io::Write;

use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
pub struct CpuFeatures {
    pub arch: String,
    pub avx2: bool,
    pub avx: bool,
    pub sse4_1: bool,
    pub sse4_2: bool,
    pub neon: bool,
}

/// Detect CPU feature flags using stable Rust intrinsics.
pub fn detect() -> CpuFeatures {
    #[cfg(target_arch = "x86_64")]
    {
        CpuFeatures {
            arch: "x86_64".into(),
            avx2: std::is_x86_feature_detected!("avx2"),
            avx: std::is_x86_feature_detected!("avx"),
            sse4_1: std::is_x86_feature_detected!("sse4.1"),
            sse4_2: std::is_x86_feature_detected!("sse4.2"),
            neon: false,
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        CpuFeatures {
            arch: "aarch64".into(),
            avx2: false,
            avx: false,
            sse4_1: false,
            sse4_2: false,
            neon: true,
        }
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        CpuFeatures {
            arch: std::env::consts::ARCH.into(),
            avx2: false,
            avx: false,
            sse4_1: false,
            sse4_2: false,
            neon: false,
        }
    }
}

/// Run the `cpu-features` command: detect and print JSON.
pub fn run(out: &mut impl Write) -> Result<()> {
    let features = detect();
    let json = serde_json::to_string(&features)?;
    out.write_all(json.as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_valid_arch() {
        let f = detect();
        assert!(
            f.arch == "x86_64" || f.arch == "aarch64" || f.arch == std::env::consts::ARCH,
            "unexpected arch: {}",
            f.arch
        );
    }

    #[test]
    fn detect_x86_64_has_consistent_flags() {
        let f = detect();
        if f.arch == "x86_64" {
            // AVX2 implies AVX
            if f.avx2 {
                assert!(f.avx, "avx2=true but avx=false");
            }
            // AVX implies SSE4.2 implies SSE4.1
            if f.avx {
                assert!(f.sse4_2, "avx=true but sse4_2=false");
            }
            if f.sse4_2 {
                assert!(f.sse4_1, "sse4_2=true but sse4_1=false");
            }
            // x86_64 never has NEON
            assert!(!f.neon, "x86_64 should not have neon");
        }
    }

    #[test]
    fn detect_aarch64_has_neon() {
        let f = detect();
        if f.arch == "aarch64" {
            assert!(f.neon, "aarch64 should have neon=true");
            assert!(!f.avx2, "aarch64 should not have avx2");
        }
    }

    #[test]
    fn run_outputs_valid_json() {
        let mut buf = Vec::new();
        run(&mut buf).unwrap();
        let json_str = String::from_utf8(buf).unwrap();
        let v: serde_json::Value = serde_json::from_str(json_str.trim()).unwrap();
        assert!(v.get("arch").is_some(), "missing arch field");
        assert!(v.get("avx2").is_some(), "missing avx2 field");
        assert!(v.get("avx").is_some(), "missing avx field");
        assert!(v.get("sse4_1").is_some(), "missing sse4_1 field");
        assert!(v.get("sse4_2").is_some(), "missing sse4_2 field");
        assert!(v.get("neon").is_some(), "missing neon field");
    }

    #[test]
    fn run_outputs_single_line() {
        let mut buf = Vec::new();
        run(&mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 1, "expected single line of output");
    }
}
