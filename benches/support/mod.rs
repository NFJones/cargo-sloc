//! Deterministic synthetic workspaces shared by performance benchmarks.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// A generated Cargo workspace retained for the lifetime of a benchmark run.
pub struct SyntheticWorkspace {
    _temp: TempDir,
    root: PathBuf,
    edited_source: PathBuf,
    original_source: String,
}

impl SyntheticWorkspace {
    /// Generates a workspace with regular modules, cfg-gated test source, comments, and literals.
    pub fn new(packages: usize, modules: usize, lines_per_module: usize) -> Self {
        let temp = tempfile::tempdir().expect("create benchmark workspace");
        let root = temp.path().to_path_buf();
        let members = (0..packages)
            .map(|package| format!("package-{package}"))
            .collect::<Vec<_>>();
        write(
            root.join("Cargo.toml"),
            &format!(
                "[workspace]\nmembers = [{}]\nresolver = \"3\"\n",
                members
                    .iter()
                    .map(|member| format!("\"{member}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );

        for (package_index, member) in members.iter().enumerate() {
            let package = root.join(member);
            write(
                package.join("Cargo.toml"),
                &format!(
                    "[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\ndefault = [\"full\"]\nfull = []\n"
                ),
            );
            let mut module_declarations = Vec::new();
            for module_index in 0..modules {
                module_declarations.push(format!("mod module_{module_index};"));
                let body = (0..lines_per_module)
                    .map(|line| {
                        format!(
                            "pub fn item_{package_index}_{module_index}_{line}() -> usize {{ {line} }} // benchmark item"
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                write(
                    package.join(format!("src/module_{module_index}.rs")),
                    &format!("{body}\n"),
                );
            }
            module_declarations.push("#[cfg(test)]".to_owned());
            module_declarations.push(
                "mod tests { #[test] fn smoke() { assert_eq!(\"// literal\", \"// literal\"); } }"
                    .to_owned(),
            );
            write(
                package.join("src/lib.rs"),
                &format!("{}\n", module_declarations.join("\n")),
            );
        }

        let edited_source = root.join("package-0/src/module_0.rs");
        let original_source = fs::read_to_string(&edited_source)
            .expect("read benchmark source selected for controlled edits");
        Self {
            _temp: temp,
            root,
            edited_source,
            original_source,
        }
    }

    /// Returns the generated workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Applies or removes one deterministic source edit used by incremental scenarios.
    pub fn set_source_edit(&self, edited: bool) {
        let contents = if edited {
            format!(
                "{}pub fn item_added_after_warm_run() {{}}\n",
                self.original_source
            )
        } else {
            self.original_source.clone()
        };
        fs::write(&self.edited_source, contents).expect("write controlled benchmark source edit");
    }
}

/// A package whose library and many binaries all reach the same module graph.
pub struct SharedTargetWorkspace {
    _temp: TempDir,
    root: PathBuf,
}

impl SharedTargetWorkspace {
    /// Generates one library and `binary_count` reporting targets over shared source.
    pub fn new(binary_count: usize, module_count: usize, lines_per_module: usize) -> Self {
        let temp = tempfile::tempdir().expect("create shared-target benchmark workspace");
        let root = temp.path().to_path_buf();
        let mut manifest = String::from(
            "[package]\nname = \"shared-targets\"\nversion = \"0.1.0\"\nedition = \"2024\"\nautobins = false\n\n[lib]\npath = \"src/lib.rs\"\ntest = false\nbench = false\n",
        );
        for index in 0..binary_count {
            manifest.push_str(&format!(
                "\n[[bin]]\nname = \"shared-bin-{index}\"\npath = \"src/lib.rs\"\ntest = false\nbench = false\n"
            ));
        }
        write(root.join("Cargo.toml"), &manifest);

        let declarations = (0..module_count)
            .map(|index| format!("mod module_{index};"))
            .collect::<Vec<_>>()
            .join("\n");
        write(root.join("src/lib.rs"), &format!("{declarations}\n"));
        for module in 0..module_count {
            let body = (0..lines_per_module)
                .map(|line| format!("pub fn item_{module}_{line}() -> usize {{ {line} }}"))
                .collect::<Vec<_>>()
                .join("\n");
            write(
                root.join(format!("src/module_{module}.rs")),
                &format!("{body}\n"),
            );
        }

        Self { _temp: temp, root }
    }

    /// Returns the generated workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// A package whose shared module graph is evaluated under divergent production and test cfgs.
pub struct DivergentContextWorkspace {
    _temp: TempDir,
    root: PathBuf,
}

impl DivergentContextWorkspace {
    /// Generates one library whose files are reachable in production and test contexts.
    pub fn new(module_count: usize, lines_per_module: usize) -> Self {
        let temp = tempfile::tempdir().expect("create divergent-context benchmark workspace");
        let root = temp.path().to_path_buf();
        write(
            root.join("Cargo.toml"),
            "[package]\nname = \"divergent-contexts\"\nversion = \"0.1.0\"\nedition = \"2024\"\nautobins = false\n\n[lib]\npath = \"src/lib.rs\"\nbench = false\n",
        );

        let declarations = (0..module_count)
            .map(|index| format!("mod module_{index};"))
            .collect::<Vec<_>>()
            .join("\n");
        write(root.join("src/lib.rs"), &format!("{declarations}\n"));
        for module in 0..module_count {
            let body = (0..lines_per_module)
                .map(|line| {
                    if line % 2 == 0 {
                        format!(
                            "#[cfg(not(test))] pub fn production_{module}_{line}() -> usize {{ {line} }}"
                        )
                    } else {
                        format!(
                            "#[cfg(test)] pub fn test_{module}_{line}() -> usize {{ {line} }}"
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            write(
                root.join(format!("src/module_{module}.rs")),
                &format!("{body}\n"),
            );
        }

        Self { _temp: temp, root }
    }

    /// Returns the generated workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("benchmark fixture parent"))
        .expect("create benchmark fixture directory");
    fs::write(path, contents).expect("write benchmark fixture");
}
