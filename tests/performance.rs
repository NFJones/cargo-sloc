//! Determinism coverage for a representative bounded synthetic workspace.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use cargo_loc::cli::ParseOutcome;
use cargo_loc::report::Report;
use tempfile::TempDir;

#[test]
fn repeated_synthetic_workspace_reports_are_byte_identical() {
    let root = synthetic_workspace();
    let arguments = || [OsString::from("--json"), root.path().as_os_str().to_owned()];

    let first = cargo_loc::run(arguments());
    let second = cargo_loc::run(arguments());
    let third = cargo_loc::run(arguments());
    assert_eq!(first.exit_code, 0, "first run failed");
    assert_eq!(second.exit_code, 0, "second run failed");
    assert_eq!(third.exit_code, 0, "third run failed");
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stdout, third.stdout);
    assert_eq!(first.stderr, second.stderr);
    assert_eq!(first.stderr, third.stderr);

    let report: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("parse deterministic report");
    assert_eq!(report["rows"].as_array().map(Vec::len), Some(7));
    assert_eq!(report["total"]["files"], 19);
    assert_eq!(report["total"]["lines"], 282);
}

#[test]
fn measured_runs_preserve_output_and_expose_pipeline_work() {
    let root = synthetic_workspace();
    let arguments = || [OsString::from("--json"), root.path().as_os_str().to_owned()];

    let expected = cargo_loc::run(arguments());
    let measured = cargo_loc::run_with_metrics(arguments());

    assert_eq!(measured.output, expected);
    assert_eq!(measured.output.exit_code, 0);
    assert_eq!(measured.metrics.workload.projects, 1);
    assert_eq!(measured.metrics.workload.packages, 3);
    assert_eq!(measured.metrics.workload.reachable_source_files, 15);
    assert!(measured.metrics.workload.build_contexts >= 3);
    assert!(measured.metrics.workload.source_contexts >= 15);
    assert_eq!(measured.metrics.workload.file_analysis_lowerings, 15);
    assert_eq!(
        measured.metrics.workload.file_context_evaluations,
        measured.metrics.workload.source_contexts
    );
    assert_eq!(measured.metrics.queries.cargo_metadata, 1);
    assert_eq!(measured.metrics.queries.cargo_package_id, 0);
    assert_eq!(measured.metrics.queries.rustc_host, 1);
    assert_eq!(measured.metrics.queries.rustc_cfg, 1);
    assert_eq!(
        measured.metrics.subprocesses,
        measured.metrics.queries.total()
    );
    assert_eq!(measured.metrics.subprocesses, 3);
    assert_eq!(measured.metrics.caches.parse_misses, 15);
    assert_eq!(measured.metrics.caches.parse_hits, 0);
    assert!(measured.metrics.caches.cfg_misses >= 1);
    assert!(measured.metrics.workload.accounting_workers > 1);
    assert!(measured.metrics.workload.accounting_workers <= 8);
    assert!(measured.metrics.phases.total >= measured.metrics.phases.discovery);
    assert!(measured.metrics.phases.total >= measured.metrics.phases.configuration);
    assert!(measured.metrics.phases.total >= measured.metrics.phases.source_discovery);
    assert!(measured.metrics.phases.total >= measured.metrics.phases.accounting);
    assert!(measured.metrics.phases.total >= measured.metrics.phases.rendering);
}

#[test]
fn cold_warm_and_one_source_edit_reports_are_deterministic() {
    let root = synthetic_workspace();
    let arguments = || [OsString::from("--json"), root.path().as_os_str().to_owned()];

    let cold = cargo_loc::run_with_metrics(arguments());
    let warm = cargo_loc::run_with_metrics(arguments());
    assert_eq!(cold.output.exit_code, 0);
    assert_eq!(warm.output, cold.output);

    let edited_path = root.path().join("a/src/one.rs");
    let original = fs::read(&edited_path).expect("read source before edit");
    let mut edited = original.clone();
    edited.extend_from_slice(b"pub fn added_after_warm_run() {}\n");
    fs::write(&edited_path, &edited).expect("write one-source edit");

    let first_edit = cargo_loc::run_with_metrics(arguments());
    let second_edit = cargo_loc::run_with_metrics(arguments());
    assert_eq!(first_edit.output.exit_code, 0);
    assert_eq!(second_edit.output, first_edit.output);
    assert_eq!(first_edit.metrics.workload, warm.metrics.workload);
    let edited_report: serde_json::Value =
        serde_json::from_slice(&first_edit.output.stdout).expect("parse edited report");
    assert_eq!(edited_report["total"]["lines"], 283);

    fs::write(&edited_path, original).expect("restore edited source");
    let restored = cargo_loc::run_with_metrics(arguments());
    assert_eq!(restored.output, cold.output);
}

#[test]
fn representative_selections_are_byte_identical_across_repeated_runs() {
    let root = synthetic_workspace();
    let selections = [
        vec![OsString::from("--json")],
        vec![OsString::from("--json"), OsString::from("--lib")],
        vec![
            OsString::from("--json"),
            OsString::from("--no-default-features"),
        ],
        vec![
            OsString::from("--json"),
            OsString::from("--features"),
            OsString::from("full"),
        ],
    ];

    for mut selection in selections {
        selection.push(root.path().as_os_str().to_owned());
        let first = cargo_loc::run_with_metrics(selection.clone());
        let second = cargo_loc::run_with_metrics(selection);
        assert_eq!(first.output.exit_code, 0);
        assert_eq!(second.output, first.output);
        assert_eq!(second.metrics.workload, first.metrics.workload);
    }
}

#[test]
fn cfg_equivalent_report_contexts_share_one_source_analysis() {
    let root = shared_target_workspace(8);
    let measured =
        cargo_loc::run_with_metrics([OsString::from("--json"), root.path().as_os_str().to_owned()]);

    assert_eq!(measured.output.exit_code, 0);
    let report: serde_json::Value =
        serde_json::from_slice(&measured.output.stdout).expect("parse shared-target report");
    let report_contexts = report["configuration"]["feature_contexts"]
        .as_array()
        .expect("feature contexts");
    assert!(report_contexts.len() >= 9, "all target labels must remain");
    assert_eq!(report["total"]["files"], 2);
    assert_eq!(report["total"]["lines"], 59);
    assert!(measured.metrics.workload.build_contexts >= 9);
    assert_eq!(measured.metrics.workload.semantic_contexts, 1);
    assert_eq!(measured.metrics.workload.source_contexts, 1);
    assert_eq!(measured.metrics.workload.file_analysis_lowerings, 1);
    assert_eq!(measured.metrics.workload.file_context_evaluations, 1);
}

#[test]
fn production_and_test_cfgs_are_distinct_and_deterministic() {
    let root = tempfile::tempdir().expect("create distinct-context workspace");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"distinct-contexts\"\nversion = \"0.1.0\"\nedition = \"2024\"\nautobins = false\n\n[lib]\npath = \"shared.rs\"\nbench = false\n",
    );
    write(
        root.path().join("shared.rs"),
        "pub fn production() {}\n#[cfg(test)]\nfn test_only() {}\n",
    );

    let first =
        cargo_loc::run_with_metrics([OsString::from("--json"), root.path().as_os_str().to_owned()]);
    let second =
        cargo_loc::run_with_metrics([OsString::from("--json"), root.path().as_os_str().to_owned()]);

    assert_eq!(first.output.exit_code, 0);
    assert_eq!(second.output, first.output);
    assert_eq!(first.metrics.workload.build_contexts, 2);
    assert_eq!(first.metrics.workload.semantic_contexts, 2);
    assert_eq!(first.metrics.workload.source_contexts, 2);
    assert_eq!(first.metrics.workload.file_analysis_lowerings, 1);
    assert_eq!(first.metrics.workload.file_context_evaluations, 2);
    assert_eq!(first.metrics.workload.accounting_workers, 1);
    assert_eq!(second.metrics.workload, first.metrics.workload);
}

#[test]
fn accounting_worker_counts_render_byte_identical_reports() {
    let root = synthetic_workspace();

    let serial = render_with_workers(root.path(), 1);
    let parallel = render_with_workers(root.path(), 4);

    assert_eq!(parallel, serial);
}

fn render_with_workers(root: &std::path::Path, workers: usize) -> Vec<u8> {
    let selection = match cargo_loc::cli::parse(
        [OsString::from("--json"), root.as_os_str().to_owned()],
        root,
    )
    .expect("parse worker-count fixture")
    {
        ParseOutcome::Selection(selection) => selection,
        ParseOutcome::EarlyExit { .. } => panic!("unexpected early CLI exit"),
    };
    let inventory = cargo_loc::discovery::discover(&selection).expect("discover worker fixture");
    let configured = cargo_loc::configuration::resolve(&selection, &inventory)
        .expect("configure worker fixture");
    let sources = cargo_loc::rust_source::discover(&configured).expect("discover worker source");
    let accounting = cargo_loc::rust_accounting::account_with_workers(&sources, workers)
        .expect("account worker fixture");
    let mut report = Report::empty(selection);
    report.warnings = inventory.warnings;
    report.warnings.extend(configured.warnings.iter().cloned());
    report.warnings.extend(sources.warnings);
    report.warnings.sort();
    report
        .apply_configuration(&configured)
        .expect("apply worker configuration");
    report
        .apply_accounting(&accounting)
        .expect("apply worker accounting");
    report.render().expect("render worker report")
}

fn synthetic_workspace() -> TempDir {
    let root = tempfile::tempdir().expect("create synthetic workspace");
    write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"a\", \"b\", \"c\"]\nresolver = \"3\"\n",
    );
    for package in ["a", "b", "c"] {
        write(
            root.path().join(package).join("Cargo.toml"),
            &format!(
                "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\ndefault = [\"full\"]\nfull = []\n"
            ),
        );
        write(
            root.path().join(package).join("src/lib.rs"),
            "mod one;\nmod two;\nmod three;\nmod four;\n#[cfg(test)] mod tests { #[test] fn smoke() {} }\n",
        );
        for module in ["one", "two", "three", "four"] {
            let body = (0..20)
                .map(|line| format!("pub fn item_{module}_{line}() -> usize {{ {line} }}"))
                .collect::<Vec<_>>()
                .join("\n");
            write(
                root.path().join(package).join(format!("src/{module}.rs")),
                &format!("{body}\n"),
            );
        }
    }
    root
}

fn shared_target_workspace(binary_count: usize) -> TempDir {
    let root = tempfile::tempdir().expect("create shared-target workspace");
    let mut manifest = String::from(
        "[package]\nname = \"shared-targets\"\nversion = \"0.1.0\"\nedition = \"2024\"\nautobins = false\n\n[lib]\npath = \"shared.rs\"\ntest = false\nbench = false\n",
    );
    for index in 0..binary_count {
        manifest.push_str(&format!(
            "\n[[bin]]\nname = \"shared-bin-{index}\"\npath = \"shared.rs\"\ntest = false\nbench = false\n"
        ));
    }
    write(root.path().join("Cargo.toml"), &manifest);
    write(root.path().join("shared.rs"), "pub fn shared() {}\n");
    root
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("synthetic fixture parent"))
        .expect("create synthetic fixture directory");
    fs::write(path, contents).expect("write synthetic fixture");
}
