//! Compiles `swift/*.swift` into a static library and links it with the system Swift runtime.

use std::path::PathBuf;
use std::process::Command;

const SWIFT_SOURCES: [&str; 2] = ["swift/DoryAI.swift", "swift/DorySpeech.swift"];
/// `SystemLanguageModel.contextSize` first ships in this SDK.
const MIN_SDK: (u32, u32) = (26, 4);

fn main() {
    for source in SWIFT_SOURCES {
        println!("cargo:rerun-if-changed={source}");
    }
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    if !macos || std::env::var_os("CARGO_FEATURE_APPLE_INTELLIGENCE").is_none() {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        other => fail(&format!(
            "dory-macos cannot build Swift for architecture {other:?}."
        )),
    };

    let sdk = xcrun(&["--sdk", "macosx", "--show-sdk-path"]);
    let version = xcrun(&["--sdk", "macosx", "--show-sdk-version"]);
    if parse_version(&version) < MIN_SDK {
        fail(&format!(
            "dory-macos needs the macOS {}.{} SDK or later from Xcode, and found {version}.",
            MIN_SDK.0, MIN_SDK.1
        ));
    }

    let status = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "swiftc",
            "-parse-as-library",
            "-emit-library",
            "-static",
            "-O",
        ])
        .args(["-target", &format!("{arch}-apple-macosx26.0")])
        .args(["-module-name", "DoryAI"])
        .args(SWIFT_SOURCES)
        .arg("-o")
        .arg(out.join("libdoryai.a"))
        .status()
        .unwrap_or_else(|_| fail(MISSING_XCODE));
    if !status.success() {
        fail("dory-macos could not compile its Swift sources; the swiftc output above says why.");
    }

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=doryai");
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    for framework in ["FoundationModels", "Speech", "AVFoundation", "CoreMedia"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

const MISSING_XCODE: &str = "dory-macos needs Xcode's `xcrun swiftc`; install Xcode and run `xcode-select -s /Applications/Xcode.app`.";

fn xcrun(args: &[&str]) -> String {
    let output = Command::new("xcrun")
        .args(args)
        .output()
        .unwrap_or_else(|_| fail(MISSING_XCODE));
    if !output.status.success() {
        fail(MISSING_XCODE);
    }
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn parse_version(version: &str) -> (u32, u32) {
    let mut parts = version.split('.').map(|p| p.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

fn fail(sentence: &str) -> ! {
    println!("cargo::error={sentence}");
    std::process::exit(1);
}
