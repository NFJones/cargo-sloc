# cargo-loc Specification

- [cargo-loc Specification](#cargo-loc-specification)
  - [1. Status and Scope](#1-status-and-scope)
  - [2. Normative Language](#2-normative-language)
  - [3. Terminology](#3-terminology)
  - [4. System Model](#4-system-model)
  - [5. Invocation and Command-Line Interface](#5-invocation-and-command-line-interface)
  - [6. Project, Package, and Target Selection](#6-project-package-and-target-selection)
    - [6.1 Project discovery](#61-project-discovery)
    - [6.2 Default target selection](#62-default-target-selection)
    - [6.3 Target inclusion](#63-target-inclusion)
    - [6.4 Target exclusion](#64-target-exclusion)
  - [7. Feature and Configuration Resolution](#7-feature-and-configuration-resolution)
    - [7.1 Feature selection](#71-feature-selection)
    - [7.2 Compilation target cfgs](#72-compilation-target-cfgs)
    - [7.3 Context construction](#73-context-construction)
    - [7.4 Custom cfgs](#74-custom-cfgs)
  - [8. Source Discovery](#8-source-discovery)
  - [9. Conditional-Compilation Semantics](#9-conditional-compilation-semantics)
  - [10. Line Accounting](#10-line-accounting)
    - [10.1 Common measures](#101-common-measures)
    - [10.2 Included lines](#102-included-lines)
    - [10.3 Rust lexical classification](#103-rust-lexical-classification)
    - [10.4 Test classification](#104-test-classification)
    - [10.5 Arithmetic and overflow](#105-arithmetic-and-overflow)
  - [11. Reports and Output Formats](#11-reports-and-output-formats)
    - [11.1 Markdown](#111-markdown)
    - [11.2 JSON](#112-json)
    - [11.3 Empty reports](#113-empty-reports)
    - [11.4 Output streams](#114-output-streams)
  - [12. Diagnostics and Exit Status](#12-diagnostics-and-exit-status)
  - [13. Performance](#13-performance)
  - [14. Limitations and Non-Goals](#14-limitations-and-non-goals)
  - [15. Compatibility and Versioning](#15-compatibility-and-versioning)
  - [16. References](#16-references)

## 1. Status and Scope

This document specifies `cargo-loc`, a Cargo external subcommand for reporting
lines of source code associated with the Cargo projects beneath a selected
directory. It defines command behavior, project discovery, Cargo configuration,
source selection, line classification, reporting, diagnostics, and performance
expectations.

This is the first complete draft of the specification. It is expected to evolve
as the implementation exposes additional requirements. Unless a section says
otherwise, its requirements are normative.

`cargo-loc` is distributed as an executable named `cargo-loc`. Cargo invokes
such an executable as `cargo loc` when it is available on `PATH`.

Rust MUST be the initially supported language. The accounting system MUST be
designed so additional languages can be supported without replacing the Cargo
project-discovery or report models. Files in unsupported languages MUST be
ignored.

The reported metric is source-level LOC. It is not compiler-expanded LOC,
generated machine code, an estimate of executable size, or a measure of
complexity or developer effort.

## 2. Normative Language

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT",
"SHOULD", "SHOULD NOT", "RECOMMENDED", "NOT RECOMMENDED", "MAY", and
"OPTIONAL" in this document are to be interpreted as described in RFC 2119 and
RFC 8174 when, and only when, they appear in all capitals.

## 3. Terminology

cargo-loc:
: The Cargo external subcommand and source-accounting system specified by this
  document.

Root:
: The directory from which cargo-loc discovers Cargo projects. It is supplied
  by the optional positional `PATH` argument and defaults to `.`.

Beneath the Root:
: A path is beneath the Root when it is equal to the Root or is a descendant of
  the canonical Root after resolving the path to its canonical form. A textual
  `..` component or a symlink MUST NOT be allowed to make an out-of-Root path
  appear in-Root. If a path cannot be canonicalized, cargo-loc MUST NOT treat
  its containment as established merely because its normalized textual form
  begins with the Root path.

Project:
: A Cargo workspace or a standalone Cargo package discovered beneath the Root.
  Every selected Package belongs to exactly one Project for configuration,
  accounting, and reporting purposes.

Workspace:
: A Cargo workspace as resolved by Cargo metadata.

Package:
: A Cargo package selected for accounting. A Package is the report-level unit.
  It can contain multiple Cargo Targets, each of which may produce a distinct
  Rust crate.

Target:
: A Cargo target belonging to a package, such as a library, binary,
  integration test, benchmark, example, or build script.

Compilation Target:
: The target platform selected by Cargo's `--target` option, expressed as a
  target triple or custom target specification accepted by Cargo. This is
  distinct from a Cargo package Target.

Build Configuration:
: The Cargo feature selection, compilation target, package and target
  selection, test context, and other conditional-compilation inputs for which
  cargo-loc accounts source.

Cfg Option Set:
: The active Rust conditional-compilation options for one Build Configuration.
  An option is either a bare name, such as `unix`, or an exact name-value pair,
  such as `target_os = "linux"`. Distinct values for the same name MAY coexist.

First-Party Source:
: Source reachable from a selected package Target. Registry and Git dependency
  source, and source belonging only to a package outside the Root, are not
  first-party source.

Active Source:
: Source syntax selected by applicable conditional-compilation predicates under
  a build configuration.

Inactive Source:
: Source syntax excluded by an applicable false conditional-compilation
  predicate under a build configuration.

Production Context:
: A selected compilation context that does not enable the Rust test harness and
  is not an integration-test or benchmark target.

Test Context:
: A selected compilation context that enables the Rust test harness, or that
  represents an integration-test or benchmark target.

Test-Only Code:
: Active source code that is reachable in at least one Test Context and no
  Production Context.

Accountant:
: A language-specific component that discovers and classifies source for one
  language for the source paths and compilation contexts supplied by the core
  system, while producing the common counts defined by this specification.

Physical Line:
: A sequence of source bytes terminated by LF or CRLF, or a final non-empty
  sequence after the last terminator. An empty file has zero Physical Lines. A
  final line terminator does not create an additional empty line.

Line of Code (LOC):
: A source line classified and counted according to the accounting rules in
  Section 10. LOC is a source-level metric and MUST NOT be represented as a
  count of macro-expanded compiler output.

## 4. System Model

cargo-loc MUST perform these logical phases:

1. discover Cargo projects beneath the Root;
2. query Cargo to resolve workspaces, packages, features, targets, and
   configuration;
3. construct the applicable Build Configurations;
4. discover first-party source reachable from selected targets;
5. evaluate source-level conditional compilation;
6. classify included Physical Lines through a language Accountant; and
7. aggregate and render the report.

cargo-loc MUST NOT invoke `cargo build`, compile a selected package, expand a
macro, or execute a build script during normal accounting. It MAY invoke
non-compiling query commands such as `cargo metadata` and `rustc --print cfg`.
It MUST NOT intentionally modify package manifests or source files. Cargo
queries MAY perform the ordinary dependency-index, cache, and lockfile access
required by the installed Cargo version; cargo-loc MUST NOT describe those
queries as side-effect-free.

No single Cargo query is assumed to expose every phase above. In particular,
the context-insensitive feature union in stable `cargo metadata` output is not
sufficient evidence for a context-specific Build Configuration. cargo-loc MAY
combine non-compiling Cargo queries with an implementation of Cargo's
documented resolver semantics. If it cannot determine a feature or target
context with the fidelity required by Sections 6 and 7, it MUST fail with a
diagnostic rather than silently merge contexts or present an approximation as
a complete report.

An Accountant MUST return the common `Files`, `Lines`, `Blanks`, `Comments`,
`Code`, and `Test` measures. Language-specific parsing and configuration logic
MUST be kept behind the Accountant boundary. The architecture MUST permit a
future Accountant to support another language without changing the meaning of
those report fields.

The Rust Accountant MUST use Rust-aware lexical and syntactic analysis. It MUST
NOT identify comments, attributes, strings, or conditional source using regular
expressions alone.

## 5. Invocation and Command-Line Interface

The executable MUST be named `cargo-loc`. It MUST support direct invocation and
Cargo external-subcommand invocation:

```text
cargo-loc [OPTIONS] [PATH]
cargo loc [OPTIONS] [PATH]
```

Under Cargo external-subcommand invocation, Cargo passes the subcommand name as
the executable's first argument, so the first argument is the literal `loc`.
`cargo-loc` MUST consume that marker without changing the remaining command
semantics. Consequently, the bare direct invocation `cargo-loc loc` is
reserved; a Root directory named `loc` MUST be passed in another unambiguous
form such as `cargo-loc ./loc`.

`PATH` MUST be an optional positional directory and MUST default to `.`. A
relative path MUST be resolved against the process working directory. The Root
MUST exist, MUST be a directory, and MUST be canonicalizable. The canonical
absolute Root MUST be used for containment, identity, and deduplication. An
inaccessible, nonexistent, non-directory, or uncanonicalizable Root MUST be a
command error rather than causing cargo-loc to use weaker textual containment.

The command MUST support:

- `-p SPEC` and `--package SPEC` for Cargo-compatible package selection;
- `--workspace` for explicit workspace-wide selection;
- `--exclude SPEC` for excluding packages from a workspace selection;
- `-F FEATURES` and `--features FEATURES`;
- `--all-features`;
- `--no-default-features`;
- repeatable `--target TARGET` for compilation-target selection;
- the package-target selectors in Section 6;
- `--exclude-target SELECTOR` as defined in Section 6;
- `--json` as defined in Section 11;
- `-h` and `--help`; and
- `-V` and `--version`.

`FEATURES` MUST accept Cargo's comma-separated or space-separated feature-list
syntax. Options that Cargo permits more than once MUST be repeatable with the
same accumulation behavior. In particular, repeated `--target` options MUST
select every requested Compilation Target rather than allowing only one or
silently retaining only the last value.

The feature, package, and standard package-target options MUST follow the
corresponding `cargo build` syntax and resolution behavior except where this
specification explicitly defines a different default. Unknown options and
invalid combinations MUST be rejected.

`--help` MUST describe that all eligible Packages beneath the Root, all
features, and all package targets are selected by default. `--version` MUST
print the cargo-loc version and exit successfully.

The command SHOULD provide examples for counting the default Root, a different
Root, a selected feature set, non-default features, and JSON output.

## 6. Project, Package, and Target Selection

### 6.1 Project discovery

cargo-loc MUST recursively discover Cargo manifests beneath the Root. Discovery
MUST ignore at least:

- Cargo `target` directories;
- `.git`, `.hg`, and `.svn` administration directories; and
- paths excluded by `.gitignore` files located at or beneath the Root.

Git-ignore matching MUST begin at the Root and honor nested `.gitignore` files
using Git's pattern and precedence semantics. Repository-local excludes outside
the traversed tree, global excludes, and ignore rules inherited only from an
ancestor of the Root MUST NOT affect discovery. These rules ensure that the
same Root contents produce the same candidate set independently of the user's
global Git configuration.

Ignore rules govern recursive candidate-manifest traversal only. After a
Project is discovered, Cargo metadata is authoritative for its workspace
membership. An in-Root workspace member reported by that metadata MUST NOT be
removed merely because its manifest would have been ignored during independent
recursive discovery. A nonmember path dependency MUST NOT become part of that
Project merely because Cargo metadata reports it in the dependency graph. Its
manifest MUST be independently discovered before it can form another Project.
An ignored standalone or nonmember manifest therefore remains undiscovered.

Symlinked directories SHOULD NOT be followed during recursive manifest
discovery. A manifest explicitly encountered through the Root path MAY itself
refer to symlinked source files.

For each candidate manifest, cargo-loc MUST use Cargo metadata to determine its
workspace root, workspace members, packages, and target source paths. A
candidate manifest is one encountered by the traversal after the exclusions
above have been applied. If Cargo cannot load such a manifest or resolve its
Project metadata, cargo-loc MUST fail rather than silently omit a potentially
countable Project.

Cargo queries for a Project MUST use that Project's workspace root, or
standalone package root, as their working directory so Project-local Cargo
configuration is applied. Manifests resolving to the same workspace MUST be
collapsed into one Project. A discovered package that is not a member of a
discovered workspace MUST form its own standalone Project. If Cargo reports the
same Package in dependency graphs for other Projects, that MUST NOT change its
owning Project or cause its source to be counted more than once.

Only workspace members and standalone packages whose manifest paths are
beneath the Root MUST be selected by default. An in-tree nonmember path package
is eligible only when its manifest is independently discovered, in which case
it belongs to its standalone Project rather than to a Project that depends on
it. Registry, Git, and out-of-Root path dependencies MUST NOT be counted.

A package identified more than once during discovery MUST be selected once,
using Cargo's package identity. Project discovery and report order MUST be
deterministic for an unchanged file tree.

When no package-selection option is present, every eligible Package described
above MUST be selected. Package-selection options apply across the complete set
of discovered Projects: `--workspace` MUST select every eligible workspace
member in every Project, including the sole member of a standalone Project.

If `-p` or `--package` is present, cargo-loc MUST apply Cargo package-spec
matching across the discovered Projects and select only matching packages. If
`--workspace` is present, all workspace members whose manifest paths are
beneath the Root MUST be selected; it MUST NOT expand accounting beyond the
Root. `--exclude` MUST be valid only with a workspace-wide selection and MUST
use Cargo package-spec matching. A requested package or exclusion that matches
no eligible Package in any discovered Project MUST be an error. Failure to
match an unrelated Project MUST NOT by itself make the invocation fail.

### 6.2 Default target selection

All package targets MUST be selected by default. This includes:

- library targets;
- binary targets;
- example targets;
- integration-test targets;
- benchmark targets; and
- custom build-script targets.

This default intentionally corresponds to an accounting-oriented union of a
package's targets and is broader than the default target set of `cargo build`.
For those Targets, the default MUST include every applicable Production and
Test Context described in Section 7.3, including enabled test and benchmark
harness contexts.

The custom build-script Target is an explicit cargo-loc accounting root. This
is an intentional extension of Cargo's documented `--all-targets` selector,
whose selector equivalence does not list custom build scripts even though Cargo
may compile them as supporting artifacts.

A Target whose manifest `required-features` are not enabled in the effective
Package feature set is ineligible. Broad selectors, including the default,
`--bins`, `--examples`, `--tests`, `--benches`, and `--all-targets`, MUST skip
an ineligible Target as Cargo does. A named selector for an ineligible Target
MUST fail with a diagnostic that identifies its missing required features.

### 6.3 Target inclusion

cargo-loc MUST support Cargo's standard target-selection options:

- `--lib`;
- `--bin NAME` and `--bins`;
- `--example NAME` and `--examples`;
- `--test NAME` and `--tests`;
- `--bench NAME` and `--benches`; and
- `--all-targets`.

When any target-inclusion option other than `--all-targets` is supplied, the
selected target and compilation-context set MUST be the union requested by
those options rather than the default all-target set. These options MUST use
Cargo's selection semantics. In particular, plural `--tests` and `--benches`
selectors MUST account for test- or benchmark-enabled targets as Cargo does;
they MUST NOT be approximated as filters for only the `test` and `bench` target
kinds. A named selector MUST fail if it matches no target among the selected
Packages, following Cargo's applicable package-selection behavior. It MUST NOT
fail merely because another selected Package has no target with that name.

For cargo-loc, `--all-targets` MUST explicitly select the same complete target
set as the cargo-loc default, including the custom build-script Target. Target
eligibility MUST be evaluated after Cargo feature resolution and before the
selected contexts are constructed.

Package-target selectors define accounting roots, not Cargo's complete
dependency compilation graph. cargo-loc MUST NOT add a Package's library,
binary, or another Target solely because Cargo would compile it as a supporting
artifact of a selected Target. Such a Target contributes source only when it is
independently selected. Cargo-compatible target semantics in this section
govern selector matching, eligibility, and compilation contexts; they do not
cause linked crate dependencies to become accounting roots.

### 6.4 Target exclusion

`--exclude-target SELECTOR` MUST be repeatable and MUST remove matching targets
after inclusion is resolved. Its grammar MUST be:

```text
SELECTOR := KIND | KIND ":" NAME
KIND     := "lib" | "bin" | "example" | "test" | "bench" | "build-script"
```

A kind-only selector MUST exclude every target of that kind. A named selector
MUST exclude the target of that kind and name. `build-script` MUST NOT accept a
name. For exclusion purposes, `lib` includes every library-like Cargo target,
including procedural-macro targets. A kind-only `test` selector MUST remove all
Test Contexts that Cargo's `--tests` selector would select, including test
harness contexts for library and binary Targets. A kind-only `bench` selector
MUST analogously remove all contexts selected by Cargo's `--benches` selector.
A named `test:NAME` or `bench:NAME` selector MUST remove only the named
integration-test or benchmark Target. This distinction allows
`--exclude-target test --exclude-target bench` to request a production-only
count. An invalid selector MUST be a command-line error. An exclusion that
matches no target or context SHOULD produce a warning rather than fail the
command.

If inclusion and exclusion leave a package with no selected targets, that
package MUST contribute no report row. If every package has no selected target,
the command MUST produce the empty successful report described in Section 11.

## 7. Feature and Configuration Resolution

### 7.1 Feature selection

When none of `--features`, `--all-features`, or `--no-default-features` is
present, cargo-loc MUST enable all features of every selected package. This
default intentionally differs from `cargo build` in order to report source for
the all-features configuration by default. It MUST NOT be described as a union
or maximum of the source active under every possible feature configuration;
negative feature predicates can make source inactive when all features are
enabled.

An explicit `--all-features` MUST have the same effect as the cargo-loc default.

When `--features FEATURES` is supplied without `--all-features`, cargo-loc MUST
use Cargo's normal feature resolution: default features are enabled unless
`--no-default-features` is also supplied, the listed features are enabled, and
their transitive feature closure is resolved by Cargo.

`--no-default-features` without `--features` MUST request no features directly
and MUST disable each selected package's default feature. The effective feature
set MUST still be the set produced by Cargo after feature unification and any
requirements imposed by other selected workspace packages.

Combinations of the three feature-selection options MUST otherwise follow
Cargo's accepted syntax and precedence. In particular, `--all-features` MAY be
combined with `--no-default-features` or `--features`; the effective set still
contains every feature exposed by the selected Packages.

cargo-loc MUST NOT define an `--exclude-features` option. Users MUST express an
excluded feature configuration through Cargo's `--no-default-features` and
`--features` options. This avoids configurations that violate Cargo's additive
feature model or transitive feature closure.

Feature names qualified for workspace packages MUST use the syntax and
semantics accepted by the installed Cargo version. cargo-loc MUST rely on Cargo
resolution rather than implementing a conflicting independent feature graph.
Feature resolution MUST occur independently for each Project containing at
least one selected Package. A Package's `cfg(feature = "...")` predicates MUST
be evaluated against the effective feature set for that Package in the
applicable Cargo compilation context, not against a union of feature names from
other Packages or Projects.

cargo-loc MUST honor the Project's Cargo feature resolver. If Cargo resolves
distinct feature sets for different compilation contexts of the same Package,
cargo-loc MUST preserve and evaluate those contexts separately. It MUST NOT
replace them with their feature-name union: doing so would produce incorrect
results for predicates such as `cfg(not(feature = "..."))`. The line-accounting
union and production-context-wins rule apply only after each such context has
been evaluated independently.

Stable `cargo metadata` may report only a union of features for a resolved
Package and cannot, by itself, prove that two resolver contexts use the same
feature set. An implementation MUST NOT infer context equivalence from that
union. It MUST obtain or reproduce the applicable context-specific resolution,
or fail as required by Section 4 when that cannot be done faithfully.

Because one Root may contain independent Projects, a requested feature MUST be
forwarded only to the Projects in which it applies to a selected Package under
Cargo's workspace feature-selection rules. A feature request that matches no
selected Package in any discovered Project MUST be an error. Its absence from
an unrelated Project MUST NOT by itself make the invocation fail.

### 7.2 Compilation target cfgs

For each Project containing at least one selected Package, cargo-loc MUST
determine the effective Compilation Target set using Cargo's command-line and
configuration precedence. The query MUST use the Project root as its working
directory so Project-local Cargo configuration and toolchain selection apply.
Explicit `--target` values MUST select those targets. Without an explicit
value, Cargo's configured `build.target` value or values MUST apply. If neither
source selects a target, the effective set MUST contain the host target used by
the Rust toolchain selected for that Project. Different Projects beneath one
Root MAY therefore have different host and effective Compilation Targets.

Target-built libraries, binaries, examples, integration tests, and benchmarks
MUST be evaluated separately for every effective Compilation Target. Their cfg
sets MUST NOT be merged: source is active in a context only under the cfg set
for that context's Compilation Target. Host-built artifacts that Cargo runs
during a cross-build, including custom build scripts and procedural macros,
MUST instead use that Project's host target predicates and MUST NOT be
duplicated merely because multiple target-built Compilation Targets are
selected.

cargo-loc SHOULD obtain built-in target cfg values from the selected toolchain
using `rustc --print cfg`, passing the applicable compilation target and crate
type when needed. It MUST NOT maintain a hard-coded target-predicate table as
its sole source of truth. The probe and resulting context MUST account for
properties such as crate type when they alter built-in cfg values; for example,
a procedural-macro crate has the `proc_macro` cfg. Different selected Targets
and different effective Compilation Targets MAY therefore have different
built-in cfg sets. Probes for a Project MUST use the Rust toolchain and relevant
environment selected for that Project.

The baseline Cfg Option Set is the set observable through these non-compiling
probes, augmented by Cargo features and the context-specific options in this
specification. For every enabled Package feature named `NAME`, the applicable
Cfg Option Set MUST contain the exact option `feature = "NAME"`. Bare-name and
name-value options MUST remain distinct, and multiple name-value options with
the same name MUST NOT overwrite one another.

An atomic bare-name predicate MUST be true exactly when that bare name is in
the applicable Cfg Option Set. An atomic name-value predicate MUST be true
exactly when that pair is in the set. `all`, `any`, and `not` MUST compose those
truth values according to the Rust Reference. The recognized-option universe
in Section 7.4 controls diagnostics only; recognition MUST NOT make an inactive
option true.

cargo-loc does not model Cargo profile settings or arbitrary compiler flags in
the baseline command. If detectable settings may alter predicates such as
`debug_assertions`, `panic`, or `target_feature`, cargo-loc SHOULD warn that the
result is not build-observed.

### 7.3 Context construction

For every selected target, cargo-loc MUST construct each context needed to
classify source as production or test-only:

- a normal library, procedural-macro, binary, example, or build-script
  compilation is a Production Context;
- a test-harness compilation of any test-enabled Target is a Test Context;
- an integration-test target is a Test Context; and
- a benchmark target is a Test Context.

Target-selection options MUST determine which of those contexts are observed
according to Cargo's semantics. A Target MAY have both Production and Test
Contexts. A manifest setting that disables a target's test or benchmark
harness MUST be honored. Production and Test are report-provenance categories;
they MUST NOT themselves add or remove a Rust cfg predicate. The `test` cfg
predicate MUST be set exactly in the contexts where the corresponding Cargo
operation would set it for rustc. It MUST NOT be inferred solely from this
specification's Test Context label. Package features and other cfg inputs
modeled by the baseline command MUST otherwise match the corresponding Cargo
observations for the invocation. The exclusions for custom cfgs, profiles, and
arbitrary compiler flags in Sections 7.2, 7.4, and 14 still apply.

Source active in at least one Production Context MUST be eligible for `Code`
even when it is also active in a Test Context. Source active only in Test
Contexts MUST be eligible for `Test`, as detailed in Section 10.

### 7.4 Custom cfgs

The baseline command MUST NOT execute build scripts to discover custom cfg
values. cargo-loc MUST distinguish the active cfg set from the universe of cfg
names and values recognized by the selected Rust toolchain. A recognized
built-in predicate that is not in the active set, such as
`target_os = "windows"` on a Linux target, MUST evaluate to false without an
unknown-cfg warning.

Every feature declared by the Package manifest MUST be a recognized value of
the `feature` cfg key. A declared but disabled feature MUST likewise evaluate
to false without an unknown-cfg warning. Any other recognized but inactive cfg
predicate MUST behave the same way.

A cfg name or value outside that recognized universe MUST be treated as unset,
matching Rust's behavior for a cfg option that was not provided in the modeled
configuration. cargo-loc SHOULD warn that a build script, compiler flag, or
another unmodeled input may change the count. The warning SHOULD name the
predicate and affected Package. cargo-loc MUST NOT describe such a result as
build-observed or compiler-exact.

The implementation MAY derive the recognized universe from toolchain queries,
toolchain diagnostics, and version-appropriate Rust documentation. It MUST NOT
use the active output of `rustc --print cfg` alone as that universe.

Environment variables and Cargo configuration that alter the selected Cargo
toolchain or target MUST be honored to the extent that Cargo metadata and the
invoked `rustc --print cfg` honor them. Arbitrary `--cfg` values embedded in
compiler flags are outside the baseline guarantee.

## 8. Source Discovery

The Rust Accountant MUST begin from the source path of every selected Cargo
target reported by Cargo metadata. It MUST discover external modules through
Rust module declarations, including `#[path = "..."]` and a `path` attribute
produced by an active `cfg_attr`.

Source discovery MUST follow Rust module inclusion within each selected Target,
but MUST NOT traverse an `extern crate` item, an external-prelude reference, or
another linked-crate edge. A library or other Target from the same Package is
not implicitly reachable through such an edge; Section 6.3 determines whether
that Target is an independent accounting root.

An external module MUST be traversed only in a Build Configuration in which its
module declaration is active. A file reachable in any selected context MUST be
included in that package's source set.

The Rust Accountant MUST NOT count every `.rs` file beneath a package merely
because it exists. Unreferenced fixtures, generated remnants, editor backups,
and source belonging only to unselected targets MUST NOT be counted.

Macro invocations MUST NOT be expanded for discovery. In particular,
`include!`, procedural macros, and declarative macros MUST be counted as source
invocations but MUST NOT cause their generated or included Rust to be added to
the source graph. A source file independently reachable through an ordinary
selected target or module declaration remains countable.

Each physical source file MUST be counted at most once per Package, even when:

- it is reachable from multiple selected targets;
- it is compiled in both Production and Test Contexts;
- more than one module path resolves to it; or
- multiple selected contexts expose the same file.

File identity MUST use the canonical absolute path where canonicalization
succeeds. If an otherwise readable first-party source path cannot be
canonicalized, cargo-loc MUST use its normalized absolute path as a fallback,
MUST warn, and MUST NOT silently omit the file. The fallback provides stable
best-effort deduplication but MUST NOT be represented as proof that two paths
refer to different physical files.

If the same physical file is first-party source for two distinct selected
Packages, each Package MUST count it once. The total row MUST be the arithmetic
sum of Package rows. This reflects the file's participation in both packages
and keeps report totals composable.

Files in unsupported languages MUST be ignored and MUST NOT generate a report
row. The implementation MAY provide a verbose diagnostic for ignored files,
but default operation SHOULD remain quiet.

Unreadable selected source, invalid source paths, and Rust syntax that prevents
required cfg or module analysis MUST be diagnosed as errors. cargo-loc MUST NOT
silently substitute an unfiltered text count when Rust-aware analysis fails.

## 9. Conditional-Compilation Semantics

cargo-loc MUST evaluate Rust conditional-compilation attributes according to
the Rust Reference, including:

- option-name predicates such as `unix` and `test`;
- key-value predicates such as `feature = "serde"`;
- `all(...)`, `any(...)`, and `not(...)` composition;
- nested predicate expressions;
- `#[cfg(...)]`; and
- `#[cfg_attr(...)]`, including nested `cfg_attr` results.

The Rust Accountant MUST recognize conditional attributes at every source
position where the selected Rust toolchain accepts them. Depending on the
language edition and toolchain, these positions include crates, modules, items,
fields, enum variants, statements, match arms, generic parameters, macro
invocations, and supported expression positions. It MUST NOT accept or reject
an attribute position merely because that position appears in this illustrative
list, and it MUST NOT limit cfg filtering to line-oriented item or module
patterns.

The Rust Accountant MUST interpret syntax using the Package's declared Rust
edition. Its lexical and syntactic behavior MUST be compatible with source
accepted by the supported Rust toolchain; it MUST NOT silently reinterpret
newer or edition-specific syntax as unfiltered text.

When `cfg_attr(PREDICATE, ATTRIBUTES...)` has a true predicate, its produced
attributes MUST participate in subsequent attribute processing. When its
predicate is false, it MUST produce no attributes. Generated `cfg`, `cfg_attr`,
`path`, `test`, and related attributes MUST affect selection and classification
as they would at that source location without macro expansion. A `test` or
`bench` attribute that is present in a context MUST make its annotated function
harness-only in that context. If `cfg_attr` does not produce that attribute in
another context, the function MAY remain active there and the
production-context-wins rule applies.

A conditional attribute written in the source is itself source code. A true
`cfg` attribute and a `cfg_attr` attached to an active construct MUST be counted
as active non-comment syntax on every Physical Line they touch, even when the
`cfg_attr` predicate is false and therefore produces no attribute. When a
false `cfg` makes its construct inactive, the governing conditional attribute
is inactive with that construct as specified below.

If any applicable `cfg` attribute on syntax is false, that syntax and the
conditional attributes governing it MUST be inactive in that Build
Configuration. The inactive region MUST include every outer attribute attached
to the governed construct, separators or terminators owned by the construct,
trivia between those attributes and the construct, and comments and whitespace
lexically enclosed by the construct. This includes attached attributes that
precede or follow the false `cfg` attribute. A crate-level false inner `cfg`
MUST govern the crate source. Trivia outside the governed construct, such as a
standalone comment before the first attached outer attribute or after the
construct, MUST remain independently active unless another false conditional
attribute governs it.

Inactive syntax and its governed trivia MUST contribute to none of `Lines`,
`Blanks`, `Comments`, `Code`, or `Test` unless some part of the same Physical
Line is independently active in another selected syntax region or context.

Conditional attributes may select only part of a Physical Line. The Accountant
MUST preserve independently active material on such a line and MUST remove only
the source spans governed by false attributes. It MUST preserve line-boundary
information while constructing the active source projection.

`cfg!(...)` is an expression that evaluates to a compile-time boolean; it does
not remove its enclosing syntax. cargo-loc MUST count the invocation and every
otherwise active source branch around it. It MUST NOT infer that an `if
cfg!(...)` control-flow branch is absent.

Declarative macro definitions and invocations, procedural macro attributes and
invocations, and other macro source MUST be counted as written when active.
Their expansions MUST NOT be counted. cfg attributes directly attached to a
macro definition or invocation MUST still be evaluated.

Tokens inside a macro definition body or invocation input MUST remain counted
as part of that active macro source. Attribute-like tokens inside such a token
tree MUST NOT be evaluated as conditional attributes unless they are themselves
in a compiler-recognized attribute position outside the unexpanded token tree.
This rule prevents cargo-loc from predicting how a macro will interpret its
input while preserving cfg filtering on the macro item or invocation itself.

The Accountant MUST parse cfg predicates structurally. It MUST NOT use substring
matching, regular-expression deletion, or feature-name searches as a substitute
for Rust cfg semantics.

## 10. Line Accounting

### 10.1 Common measures

Every Accountant MUST produce these unsigned integer measures:

`Files`:
: The number of unique physical source files reachable in at least one selected
  context for the Package and language. A reachable empty file or a reachable
  file whose lines are all inactive still increments `Files`.

`Lines`:
: The number of Physical Lines included under Section 10.2 in at least one
  selected Build Configuration. This includes eligible blank lines, which do
  not themselves contain Active Source.

`Blanks`:
: The number of included Physical Lines containing no active language token and
  no active comment token.

`Comments`:
: The number of included Physical Lines intersected by at least one active
  comment token.

`Code`:
: The number of included Physical Lines intersected by at least one active
  non-whitespace, non-comment language token in a Production Context.

`Test`:
: The number of included Physical Lines intersected by at least one active
  non-whitespace, non-comment language token in a Test Context and no such token
  in a Production Context.

`Code` and `Test` MUST be mutually exclusive. A line that qualifies for both,
whether because of two Build Configurations or because distinct active spans on
that line have different provenance, MUST be classified as `Code`, implementing
the production-context-wins rule.

`Comments` is an independent measure and MAY overlap `Code` or `Test`. Therefore
the report MUST NOT imply that `Blanks + Comments + Code + Test = Lines`.
`Blanks` MUST NOT overlap `Comments`, `Code`, or `Test`.

Every included line MUST contribute to at least one of `Blanks`, `Comments`,
`Code`, or `Test`. `Lines` is therefore the cardinality of the union of those
line sets, not the arithmetic sum of the four counters.

### 10.2 Included lines

An active source projection MUST preserve the original Physical Line boundaries
while suppressing inactive syntax and governed trivia. A Physical Line MUST
increment `Lines` when it contains an active language token or comment token in
at least one selected context. A whitespace-only Physical Line in a reachable
file MUST also increment `Lines` and `Blanks` unless it is enclosed by an
inactive conditional region in every selected context.

A Physical Line containing only inactive tokens and incidental whitespace MUST
contribute to no measure. A line containing both inactive and active source
MUST be classified from its active portions only. Residual indentation or
spacing around an otherwise wholly inactive construct MUST NOT cause the line
to be included.

Blank lines within the lexical extent of an active multiline token are not
blank for accounting purposes. For example, an apparently empty line inside a
block comment increments `Comments`, while an apparently empty line inside a
multiline string literal increments `Code` or `Test` according to context.

### 10.3 Rust lexical classification

The Rust Accountant MUST tokenize source according to Rust lexical rules. In
particular, it MUST distinguish comments from comment-like text inside string,
raw-string, byte-string, byte, and character literals.

Rust input preprocessing that occurs before tokenization MUST have explicit
accounting behavior. An optional UTF-8 byte-order mark at the start of a file
MUST be ignored when classifying its Physical Line. A permitted Unix shebang on
the first line MUST be treated as active non-comment source and therefore as
`Code` or `Test` according to context; `#![...]` inner attributes are not
shebangs and MUST be processed as Rust syntax.

The following MUST be comment tokens:

- line comments beginning with `//`;
- block comments delimited by `/*` and `*/`, including nested block comments;
- outer and inner line doc comments; and
- outer and inner block doc comments.

Every Physical Line touched by an active comment token MUST increment
`Comments`, including the opening, interior, and closing lines of a multiline
block comment. Documentation comments MUST be treated as comments, not code,
except that independently active code on the same line is still code.

Every Physical Line touched by an active non-comment Rust token MUST increment
`Code` or `Test` according to context. A multiline literal is one token, so each
Physical Line it touches MUST be treated as containing code even when a line's
literal content is visually empty or resembles a comment.

A mixed line such as:

```rust
let answer = 42; // the answer
```

MUST increment both `Code` and `Comments`. A comment-only production line MUST
increment `Comments` but not `Code`. A comment-only test line MUST increment
`Comments` but not `Test`.

A whitespace-only included line MUST increment `Blanks`. Every character that
the selected Rust lexer accepts as source whitespace MUST be treated as
whitespace for this classification. CR in a CRLF terminator MUST NOT make a
line non-blank.

### 10.4 Test classification

The `Test` measure is a provenance classification for code, not a count of test
functions. It MUST include code that is active only in Test Contexts, including:

- syntax selected only by `cfg(test)`;
- functions included only because of an active built-in `#[test]` or
  `#[bench]` attribute;
- integration-test target source; and
- benchmark target source.

The built-in `#[test]` and `#[bench]` attributes identify harness entry points
and cause their annotated functions to be included only in applicable harness
contexts. Such functions MUST therefore be `Test` unless the same source line
is independently active in a Production Context, for example because a
`cfg_attr` produces the harness attribute only in the Test Context.

An example target is a Production Context and MUST contribute to `Code`, not
`Test`, unless some of its source is independently test-only.

If a shared library or module line is compiled in both normal and test harness
contexts, it MUST be counted once as `Code`. If a line is reachable through
several Test Contexts and no Production Context, it MUST be counted once as
`Test`.

Conditional attributes and target selection MAY cause different tokens on one
Physical Line to have different provenance. If any active code token on the
line has Production Context provenance, the entire line MUST increment `Code`
and MUST NOT increment `Test`.

### 10.5 Arithmetic and overflow

All counters MUST be capable of representing at least the full range of an
unsigned 64-bit integer. An implementation MUST detect rather than wrap an
overflow. Report totals MUST be computed from the per-Package rows and MUST use
the same overflow behavior.

## 11. Reports and Output Formats

### 11.1 Markdown

Markdown MUST be the default output format. Successful default output MUST be a
valid GitHub-Flavored Markdown pipe table with these columns in this order:

```text
| Package | Language | Files | Lines | Blanks | Comments | Code | Test |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
```

There MUST be one row for each selected Package and supported language pair
with at least one discovered file. The `Package` cell identifies the
Package-level aggregation row; it does not identify an individual Rust crate or
Cargo Target. It MUST normally contain the Cargo package name. If names collide
within the invocation, cargo-loc MUST add a stable Root-relative path or
equivalent package qualifier so the displayed labels are unique. Text cells
MUST escape pipe characters, backslashes, and other content that would
invalidate the table.

Rows MUST be ordered deterministically, first by package label and then by
language. Language names MUST use stable display spelling; the Rust Accountant
MUST emit `Rust`.

The final row MUST be a `Total` row whose numeric values are arithmetic sums of
the preceding rows. Its `Package` cell MUST be `Total` and its `Language` cell
MUST be `All`. Because files are deduplicated within a Package but may
participate in two Packages, the `Total` row MUST follow the Package-row
semantics in Section 8 rather than global path deduplication.

The Markdown report MUST contain no ANSI escape sequences when stdout is not a
terminal. An implementation MAY offer terminal styling in the future only if
the bytes remain usable as Markdown or styling is explicitly requested.

### 11.2 JSON

`--json` MUST replace Markdown output with one UTF-8 JSON object on stdout. The
object MUST have this logical shape:

```json
{
  "schema_version": 1,
  "root": "/absolute/root",
  "configuration": {
    "package_selectors": [],
    "workspace": false,
    "package_exclude_selectors": [],
    "host_targets": ["x86_64-unknown-linux-gnu"],
    "targets": ["x86_64-unknown-linux-gnu"],
    "project_targets": [
      {
        "project_root": "/absolute/root",
        "host_target": "x86_64-unknown-linux-gnu",
        "targets": ["x86_64-unknown-linux-gnu"]
      }
    ],
    "all_features": true,
    "no_default_features": false,
    "features": [],
    "target_includes": ["all-targets"],
    "target_excludes": []
  },
  "packages": [
    {
      "name": "example",
      "package_id": "...",
      "project_root": "/absolute/root",
      "manifest_path": "/absolute/root/example/Cargo.toml",
      "language": "Rust",
      "files": 1,
      "lines": 10,
      "blanks": 2,
      "comments": 2,
      "code": 5,
      "test": 2
    }
  ],
  "total": {
    "files": 1,
    "lines": 10,
    "blanks": 2,
    "comments": 2,
    "code": 5,
    "test": 2
  },
  "warnings": []
}
```

The concrete object MAY add fields in a backwards-compatible specification
revision, but it MUST preserve the meanings and types shown above for schema
version 1. `root` MUST be the canonical absolute Root identity used for
discovery. The configuration object MUST describe the normalized selection
request. `package_selectors` and `package_exclude_selectors` MUST be
deterministically ordered arrays of the requested package specifications.
`workspace` MUST indicate whether `--workspace` was explicit. `host_targets`
MUST be the deterministically ordered, duplicate-free union of the host triples
used by Projects containing at least one selected Package. `targets` MUST be
the correspondingly ordered, duplicate-free union of their effective
Compilation Targets. `project_targets` MUST contain one object for every such
Project, ordered by `project_root`; a discovered Project with no selected
Package MUST NOT appear. Each object MUST contain the Project's absolute
workspace or standalone-package root, its `host_target`, and its
deterministically ordered, duplicate-free effective `targets`. When no explicit
or configured target is selected for a Project, that Project's `targets` MUST
contain its `host_target`. This mapping MUST preserve Project-specific
differences caused by Cargo configuration and toolchain selection. The
configuration object MAY additionally distinguish targets requested on the
command line from targets obtained through Cargo configuration.

`all_features` and `no_default_features` MUST be booleans describing the
normalized feature mode; the implicit cargo-loc default MUST set
`all_features` to `true`. `features` MUST be a deterministically ordered,
duplicate-free array of requested feature names, flattening repeated options
and comma- or space-separated lists while preserving Cargo package
qualification. `target_includes` MUST be a deterministically ordered,
duplicate-free array of canonical target selectors. Canonical entries MUST use
`lib`, `bins`, `bin:NAME`, `examples`, `example:NAME`, `tests`, `test:NAME`,
`benches`, `bench:NAME`, or `all-targets`, retaining any Cargo-supported name
pattern verbatim after the colon. The implicit default MUST be represented by
`all-targets`.
`target_excludes` MUST contain the canonical forms of the applied
`--exclude-target` selectors. The configuration object MAY additionally expose
effective feature sets, but any such extension MUST identify the Project,
Package, and compilation context to which each set applies. It MUST NOT collapse
context-specific feature sets into one union when that would lose the semantics
defined in Section 7.1.

JSON numeric counts MUST be non-negative integers. Paths MUST be valid JSON
strings and MUST be absolute so they do not depend on the consumer's working
directory. A path MUST NOT be converted with a lossy replacement of non-UTF-8
bytes; if a required path cannot be represented losslessly in JSON, report
serialization MUST fail. `packages` MUST use the same order and aggregation
semantics as the Markdown rows. Every package record MUST contain all fields
shown in the example. `package_id` MUST be Cargo's opaque Package ID serialized
as a string, and consumers MUST NOT infer its internal format. `project_root`
MUST identify the absolute Project root used for that Package. `manifest_path`
MUST be the Package's absolute manifest path. Together, `package_id`,
`project_root`, and `language` MUST unambiguously identify an aggregation row
within one report.

The `warnings` array MUST contain every nonfatal warning produced while
constructing a JSON report. Each warning MUST be an object containing stable
string `code` and `message` fields; it MAY contain additional fields identifying
the affected Project, Package, Target, file, or option. Warning objects MUST be
ordered deterministically.

### 11.3 Empty reports

If no Cargo project, selected package, selected target, or supported source file
contributes a row, cargo-loc MUST exit successfully and report zero totals.

Markdown output MUST contain the header, separator, and a zero-valued `Total`
row. JSON output MUST contain an empty `packages` array and a `total` object
whose six numeric fields are zero.

### 11.4 Output streams

The selected report MUST be written to stdout. Diagnostics MUST be written to
stderr and MUST NOT corrupt the Markdown or JSON document on stdout. Successful
JSON warnings MUST appear in the JSON `warnings` field and MAY also be shown on
stderr when stderr is a terminal.

If accounting or serialization fails, cargo-loc MUST NOT write a partial report
that could be mistaken for a successful complete result. It SHOULD buffer the
report until successful completion or otherwise ensure that stdout is empty on
failure. Diagnostics describing the failure belong on stderr.

## 12. Diagnostics and Exit Status

cargo-loc MUST return exit status zero when accounting and report rendering
succeed, including when the result is empty.

It MUST return a nonzero exit status for at least:

- invalid command-line syntax or option combinations;
- an invalid or inaccessible Root;
- Cargo metadata failure for a candidate manifest not excluded from discovery;
- failure to obtain required target cfg information;
- an explicitly requested package or target that cannot be resolved;
- unreadable selected source;
- Rust parse or analysis failure that prevents correct filtering; and
- count overflow or output serialization failure.

Command-line usage errors SHOULD use an exit status distinct from operational
or analysis errors. The exact nonzero values are not otherwise stable in schema
version 1.

Diagnostics MUST identify the affected Project, Package, target, file, or
option when that information is available. Multiple independent warnings MAY be
aggregated. Errors MUST NOT be downgraded to warnings when doing so would cause
the report to be presented as complete.

Conditions that SHOULD warn without failing include:

- an `--exclude-target` selector that matches nothing;
- a cfg predicate that may depend on an unmodeled custom cfg input;
- inability to canonicalize a readable source path; and
- a source file shared by distinct selected Packages.

Unsupported-language files MUST be ignored without warning by default.

## 13. Performance

Elapsed time is the primary performance objective. For a fixed correct result,
an implementation SHOULD prefer the design that produces the complete report
sooner.

The implementation MUST nevertheless bound concurrency and memory use so an
ordinary invocation does not intentionally exhaust host resources or make the
system unusable. It SHOULD derive a default worker limit from available
parallelism and SHOULD permit the operating system to provide backpressure.

The implementation SHOULD:

- process independent Projects, Packages, or files in parallel;
- avoid reading, tokenizing, or parsing the same physical file more often than
  its distinct language, edition, and analysis inputs require;
- reuse a parsed file across target, feature, production, and test contexts
  when those parsing inputs are equivalent;
- avoid compiling source and executing build scripts in the default path, and
  cache or minimize the non-compiling Cargo and rustc probes required by
  Sections 4 and 7;
- stream or release file data that is no longer needed;
- minimize allocation and copying while preserving correct UTF-8 and span
  handling; and
- aggregate counts without retaining unnecessary per-line records.

Deterministic output is REQUIRED even when discovery and accounting execute in
parallel.

This specification intentionally defines no benchmark corpus, latency target,
throughput floor, or regression threshold. Performance criteria MAY be added
after implementation measurements exist, but optimizations MUST NOT knowingly
weaken cfg filtering or lexical classification.

## 14. Limitations and Non-Goals

The baseline result is configuration-aware source LOC, not an exact
representation of every token passed to rustc. In particular:

- declarative and procedural macro expansions are not counted;
- conditional-looking tokens inside unexpanded macro bodies and inputs are not
  evaluated as attributes;
- source reached only through `include!` is not discovered through expansion;
- Rust code embedded in documentation examples is not parsed as a separate
  doctest Target; its containing documentation remains comment source;
- build-script-generated source is not counted unless independently present and
  reachable as first-party source;
- build scripts are not executed to discover custom cfg values;
- Cargo profile settings and arbitrary compiler flags that alter cfg values are
  not modeled by the baseline command;
- `cfg!` control-flow branches are not removed; and
- compiler optimization and dead-code elimination do not affect counts.

The initial implementation MAY support only the Rust Accountant. The
architecture MUST remain capable of adding other language Accountants, but this
specification does not require a generic fallback or prescribe an internal Rust
trait or public plugin ABI.

Dependency source outside the Root is not project code and MUST NOT be counted.
The specification does not currently define an option to count registry, Git,
or out-of-root path dependencies.

A future optional compile-observed mode MAY run Cargo or observe rustc
invocations to obtain build-script cfg values and generated-source paths. Such a
mode MUST be opt-in, MUST disclose that it may execute package code, and MUST
remain distinct from counting macro-expanded compiler output unless expansion
is itself explicitly specified.

## 15. Compatibility and Versioning

The project MUST use semantic versioning for released command-line behavior.
Before version 1.0, incompatible corrections MAY occur between minor versions
but SHOULD be documented in release notes.

The Markdown column names, order, and meanings defined in Section 11 are part of
the human-readable interface. Cosmetic spacing and alignment are not stable.

The JSON `schema_version` is independent of the package version. A change that
removes a required field, changes a field's type or meaning, or changes
aggregation semantics MUST increment `schema_version`. Adding an optional field
does not require an increment when existing consumers can ignore it.

Results MAY differ across Cargo or Rust toolchain versions when those tools
change manifest resolution, target discovery, accepted syntax, cfg values, or
language semantics. cargo-loc SHOULD report its own version and SHOULD include
relevant toolchain identity in verbose or machine-readable metadata in a future
compatible extension.

Each cargo-loc release MUST document its minimum supported Cargo and Rust
toolchain versions. If a selected Project requires an unavailable query,
configuration behavior, or language capability, cargo-loc MUST fail with a
diagnostic identifying the unsupported toolchain or capability rather than
silently applying semantics from another version.

## 16. References

- RFC 2119: <https://www.rfc-editor.org/rfc/rfc2119>
- RFC 8174: <https://www.rfc-editor.org/rfc/rfc8174>
- JSON (RFC 8259): <https://www.rfc-editor.org/rfc/rfc8259>
- GitHub Flavored Markdown: <https://github.github.com/gfm/>
- Cargo external tools: <https://doc.rust-lang.org/cargo/reference/external-tools.html>
- `cargo build`: <https://doc.rust-lang.org/cargo/commands/cargo-build.html>
- `cargo metadata`: <https://doc.rust-lang.org/cargo/commands/cargo-metadata.html>
- `cargo test`: <https://doc.rust-lang.org/cargo/commands/cargo-test.html>
- `cargo bench`: <https://doc.rust-lang.org/cargo/commands/cargo-bench.html>
- Cargo configuration: <https://doc.rust-lang.org/cargo/reference/config.html>
- Cargo features: <https://doc.rust-lang.org/cargo/reference/features.html>
- Cargo dependency resolution: <https://doc.rust-lang.org/cargo/reference/resolver.html>
- Cargo metadata format: <https://doc.rust-lang.org/cargo/commands/cargo-metadata.html#json-format-version-1>
- Cargo workspaces: <https://doc.rust-lang.org/cargo/reference/workspaces.html>
- Cargo targets: <https://doc.rust-lang.org/cargo/reference/cargo-targets.html>
- Cargo build scripts: <https://doc.rust-lang.org/cargo/reference/build-scripts.html>
- Rust attributes: <https://doc.rust-lang.org/reference/attributes.html>
- Rust conditional compilation: <https://doc.rust-lang.org/reference/conditional-compilation.html>
- Rust comments: <https://doc.rust-lang.org/reference/comments.html>
- Rust input format: <https://doc.rust-lang.org/reference/input-format.html>
- Rust lexical structure: <https://doc.rust-lang.org/reference/lexical-structure.html>
- Rust modules and source files: <https://doc.rust-lang.org/reference/items/modules.html>
- Rust testing attributes: <https://doc.rust-lang.org/reference/attributes/testing.html>
- rustc command-line arguments: <https://doc.rust-lang.org/rustc/command-line-arguments.html>
