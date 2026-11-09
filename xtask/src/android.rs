//! `android-build`: cross-compile `agent-core` and generate Kotlin bindings.
//!
//! Outputs (both git-ignored, under the app's Gradle build dir):
//!
//! ```text
//! android/app/build/generated/rust/jniLibs/<abi>/libpocket_agent_core.so
//! android/app/build/generated/rust/kotlin/io/github/pocketagent/core/pocket_agent_core.kt
//! ```
//!
//! Gradle runs this automatically before every build (see
//! `android/app/build.gradle.kts`), so a plain `./gradlew assembleDebug` works.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use camino::Utf8PathBuf;

use crate::util::root;

/// ABIs we know how to build. Releases ship arm64-v8a only (see SETUP.md).
const KNOWN_ABIS: &[(&str, &str)] = &[
    ("arm64-v8a", "aarch64-linux-android"),
    ("armeabi-v7a", "armv7-linux-androideabi"),
    ("x86_64", "x86_64-linux-android"),
    ("x86", "i686-linux-android"),
];
/// Minimum Android API level (matches `minSdk`).
const MIN_API: &str = "29";

/// Build the native library for `abis` and generate bindings.
pub fn build(release: bool, abis: &[String]) -> Result<()> {
    let ndk = find_ndk()?;
    let out = root().join("android/app/build/generated/rust");
    let jni_dir = out.join("jniLibs");
    let kotlin_dir = out.join("kotlin");

    let mut cmd = Command::new(env!("CARGO"));
    cmd.arg("ndk")
        .env("ANDROID_NDK_HOME", &ndk)
        .current_dir(root());
    for abi in abis {
        let (_, triple) = KNOWN_ABIS.iter().find(|(a, _)| a == abi).with_context(|| {
            format!("unknown ABI `{abi}`; known: arm64-v8a, armeabi-v7a, x86_64, x86")
        })?;
        ensure_target(triple)?;
        cmd.args(["-t", abi]);
    }
    cmd.args(["-P", MIN_API, "-o"])
        .arg(&jni_dir)
        .args(["build", "-p", "pocket-agent-core"]);
    if release {
        cmd.arg("--release");
    }
    println!(
        "[android-build] cargo ndk ({}) → {}",
        if release { "release" } else { "debug" },
        jni_dir.display()
    );
    let status = cmd
        .status()
        .context("could not run `cargo ndk`; install it with `cargo install cargo-ndk`")?;
    if !status.success() {
        bail!("cargo ndk failed ({status})");
    }

    // Bindings are generated from the compiled library (UniFFI library mode).
    let first = abis.first().map_or("arm64-v8a", String::as_str);
    let lib = jni_dir.join(first).join("libpocket_agent_core.so");
    let _ = std::fs::remove_dir_all(&kotlin_dir);
    uniffi::generate(uniffi::GenerateOptions {
        languages: vec![uniffi::TargetLanguage::Kotlin],
        source: utf8(&lib)?,
        out_dir: utf8(&kotlin_dir)?,
        config_override: None,
        format: false,
        crate_filter: Some("pocket_agent_core".into()),
        metadata_no_deps: true,
    })
    .context("UniFFI binding generation failed")?;
    println!("[android-build] Kotlin bindings → {}", kotlin_dir.display());
    Ok(())
}

fn utf8(p: &Path) -> Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(p.to_path_buf())
        .map_err(|p| anyhow::anyhow!("non-UTF-8 path: {}", p.display()))
}

/// Locate the NDK: `ANDROID_NDK_HOME`, else the newest `$ANDROID_HOME/ndk/*`.
pub fn find_ndk() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("ANDROID_NDK_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
    {
        return Ok(p);
    }
    let sdk = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .find_map(|v| {
            std::env::var_os(v)
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
        })
        .context("Android SDK not found: set ANDROID_HOME (see CONTRIBUTING.md)")?;
    let mut ndks: Vec<PathBuf> = std::fs::read_dir(sdk.join("ndk"))
        .context("no NDK installed: run `sdkmanager \"ndk;<version>\"`")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    ndks.sort();
    ndks.pop()
        .context("no NDK installed under $ANDROID_HOME/ndk")
}

fn ensure_target(triple: &str) -> Result<()> {
    let out = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    if let Ok(out) = out
        && !String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim() == triple)
    {
        println!("[android-build] installing Rust target {triple}");
        let ok = Command::new("rustup")
            .args(["target", "add", triple])
            .status()?
            .success();
        if !ok {
            bail!("`rustup target add {triple}` failed");
        }
    }
    Ok(())
}
