//! Reusable support for Cargo conformance fixtures.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A checked-in fixture copied to an isolated writable directory.
pub struct Fixture {
    _temp: TempDir,
    root: PathBuf,
}

impl Fixture {
    /// Copies `tests/fixtures/<name>` into a temporary directory.
    pub fn copy(name: &str) -> Self {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let temp = tempfile::tempdir().expect("create fixture directory");
        copy_tree(&source, temp.path());

        Self {
            root: temp.path().to_path_buf(),
            _temp: temp,
        }
    }

    /// Returns the fixture root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Runs Cargo in the fixture with deterministic, isolated state.
    pub fn cargo<I, S>(&self, arguments: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let cargo_home = self.root.join(".cargo-home");
        let target_dir = self.root.join("target");
        fs::create_dir_all(&cargo_home).expect("create fixture Cargo home");

        Command::new(env!("CARGO"))
            .args(arguments)
            .current_dir(&self.root)
            .env("CARGO_HOME", cargo_home)
            .env("CARGO_TARGET_DIR", target_dir)
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_TERM_COLOR", "never")
            .env("CARGO_BUILD_JOBS", "1")
            .output()
            .expect("run Cargo fixture")
    }

    /// Observes rustc invocations made by a test-only Cargo check.
    #[cfg(unix)]
    pub fn observed_rustc(&self) -> Vec<RustcInvocation> {
        use std::os::unix::fs::PermissionsExt;

        let wrapper = self.root.join("rustc-wrapper.sh");
        let log = self.root.join("rustc-invocations.bin");
        fs::write(
            &wrapper,
            "#!/bin/sh\n{ printf 'BEGIN\\0'; for arg in \"$@\"; do printf '%s\\0' \"$arg\"; done; printf 'END\\0'; } >> \"$CARGO_SLOC_RUSTC_LOG\"\nexec \"$@\"\n",
        )
        .expect("write rustc wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))
            .expect("make rustc wrapper executable");

        let cargo_home = self.root.join(".cargo-home");
        let target_dir = self.root.join("target-observed");
        fs::create_dir_all(&cargo_home).expect("create fixture Cargo home");
        let output = Command::new(env!("CARGO"))
            .args(["check", "-p", "app", "--all-targets", "--offline"])
            .current_dir(&self.root)
            .env("CARGO_HOME", cargo_home)
            .env("CARGO_TARGET_DIR", target_dir)
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_TERM_COLOR", "never")
            .env("CARGO_BUILD_JOBS", "1")
            .env("RUSTC_WRAPPER", &wrapper)
            .env("CARGO_SLOC_RUSTC_LOG", &log)
            .output()
            .expect("run observed Cargo fixture");
        assert_success(&output, "observed cargo check");

        parse_invocations(&fs::read(log).expect("read rustc invocation log"))
    }
}

/// One rustc command observed through Cargo's `RUSTC_WRAPPER` protocol.
#[derive(Debug)]
pub struct RustcInvocation {
    pub arguments: Vec<String>,
}

impl RustcInvocation {
    /// Returns the value following a rustc option.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.arguments
            .windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| pair[1].as_str())
    }

    /// Returns the feature names passed through `--cfg feature="..."`.
    pub fn features(&self) -> BTreeSet<String> {
        self.arguments
            .windows(2)
            .filter(|pair| pair[0] == "--cfg")
            .filter_map(|pair| {
                pair[1]
                    .strip_prefix("feature=\"")
                    .and_then(|feature| feature.strip_suffix('"'))
                    .map(str::to_owned)
            })
            .collect()
    }
}

/// Asserts that a subprocess completed successfully and includes diagnostics on failure.
pub fn assert_success(output: &Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn copy_tree(source: &Path, destination: &Path) {
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let source_path = entry.path();
        let entry_name = entry.file_name();
        let destination_name = if entry_name == "Cargo.toml.fixture" {
            OsString::from("Cargo.toml")
        } else {
            entry_name
        };
        let destination_path = destination.join(destination_name);
        if entry.file_type().expect("read fixture file type").is_dir() {
            fs::create_dir_all(&destination_path).expect("create fixture subdirectory");
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("copy fixture file");
        }
    }
}

#[cfg(unix)]
fn parse_invocations(bytes: &[u8]) -> Vec<RustcInvocation> {
    let mut invocations = Vec::new();
    let mut current = None;

    for value in bytes
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
    {
        match value {
            b"BEGIN" => current = Some(Vec::new()),
            b"END" => invocations.push(RustcInvocation {
                arguments: current.take().expect("END after BEGIN"),
            }),
            value => current
                .as_mut()
                .expect("argument inside invocation")
                .push(String::from_utf8(value.to_vec()).expect("UTF-8 rustc argument")),
        }
    }

    assert!(current.is_none(), "unterminated rustc invocation");
    invocations
}
