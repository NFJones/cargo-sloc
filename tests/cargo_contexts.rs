//! Cargo-observed feature-context feasibility tests.

mod support;

use std::collections::BTreeSet;

use guppy::{
    CargoMetadata,
    graph::{
        cargo::{CargoOptions, CargoResolverVersion},
        feature::StandardFeatures,
    },
    platform::{Platform, TargetFeatures},
};
use serde_json::Value;
use support::{Fixture, assert_success};

#[cfg(unix)]
#[test]
fn resolver_v1_unifies_host_target_dev_and_inactive_target_features() {
    let fixture = Fixture::copy("cargo-context/resolver-v1");
    let observed = shared_feature_sets(&fixture);

    assert_eq!(
        observed,
        BTreeSet::from([BTreeSet::from([
            "build".to_owned(),
            "dev".to_owned(),
            "normal".to_owned(),
            "unix".to_owned(),
            "windows".to_owned(),
        ])])
    );
}

#[cfg(unix)]
#[test]
fn resolver_v2_keeps_host_features_separate_and_filters_inactive_targets() {
    assert_modern_resolver_contexts("cargo-context/resolver-v2");
}

#[cfg(unix)]
#[test]
fn resolver_v3_keeps_host_features_separate_and_filters_inactive_targets() {
    assert_modern_resolver_contexts("cargo-context/resolver-v3");
}

#[cfg(unix)]
#[test]
fn guppy_reproduces_the_observed_resolver_contexts() {
    for (fixture_name, resolver) in [
        ("cargo-context/resolver-v1", CargoResolverVersion::V1),
        ("cargo-context/resolver-v2", CargoResolverVersion::V2),
        ("cargo-context/resolver-v3", CargoResolverVersion::V3),
    ] {
        let fixture = Fixture::copy(fixture_name);
        let observed = shared_feature_sets(&fixture);
        let modeled = guppy_shared_feature_sets(&fixture, resolver);

        assert_eq!(modeled, observed, "resolver comparison for {fixture_name}");
    }
}

#[cfg(unix)]
fn assert_modern_resolver_contexts(name: &str) {
    let fixture = Fixture::copy(name);
    let observed = shared_feature_sets(&fixture);

    assert_eq!(
        observed,
        BTreeSet::from([
            BTreeSet::from(["build".to_owned()]),
            BTreeSet::from(["dev".to_owned(), "normal".to_owned(), "unix".to_owned(),]),
        ])
    );

    let metadata_features = metadata_feature_union(&fixture);
    assert!(metadata_features.is_superset(&BTreeSet::from([
        "build".to_owned(),
        "dev".to_owned(),
        "normal".to_owned(),
        "unix".to_owned(),
        "windows".to_owned(),
    ])));
    assert!(
        !observed.contains(&metadata_features),
        "cargo metadata's package-level union must not be treated as one rustc context"
    );
}

#[cfg(unix)]
fn shared_feature_sets(fixture: &Fixture) -> BTreeSet<BTreeSet<String>> {
    fixture
        .observed_rustc()
        .into_iter()
        .filter(|invocation| invocation.option("--crate-name") == Some("shared"))
        .map(|invocation| invocation.features())
        .collect()
}

fn metadata_feature_union(fixture: &Fixture) -> BTreeSet<String> {
    let output = fixture.cargo(["metadata", "--format-version", "1", "--offline"]);
    assert_success(&output, "cargo metadata");
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("parse cargo metadata");
    let shared_id = metadata["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|package| package["name"] == "shared")
        .and_then(|package| package["id"].as_str())
        .expect("shared package ID");

    metadata["resolve"]["nodes"]
        .as_array()
        .expect("resolve nodes")
        .iter()
        .find(|node| node["id"] == shared_id)
        .and_then(|node| node["features"].as_array())
        .expect("shared feature union")
        .iter()
        .map(|feature| feature.as_str().expect("feature string").to_owned())
        .collect()
}

fn guppy_shared_feature_sets(
    fixture: &Fixture,
    resolver: CargoResolverVersion,
) -> BTreeSet<BTreeSet<String>> {
    let output = fixture.cargo(["metadata", "--format-version", "1", "--offline"]);
    assert_success(&output, "cargo metadata for Guppy");
    let metadata = CargoMetadata::parse_json(
        std::str::from_utf8(&output.stdout).expect("UTF-8 cargo metadata"),
    )
    .expect("parse cargo metadata for Guppy");
    let graph = metadata.build_graph().expect("build Guppy package graph");
    let app = graph
        .resolve_package_name("app")
        .to_feature_set(StandardFeatures::Default);
    let mut options = CargoOptions::new();
    options
        .set_resolver(resolver)
        .set_include_dev(true)
        .set_platform(
            Platform::new("x86_64-unknown-linux-gnu", TargetFeatures::Unknown)
                .expect("known Linux platform"),
        );
    let cargo_set = app
        .into_cargo_set(&options)
        .expect("resolve Guppy Cargo contexts");
    let shared_id = graph
        .packages()
        .find(|package| package.name() == "shared")
        .map(|package| package.id().clone())
        .expect("shared package ID");

    cargo_set
        .all_features()
        .into_iter()
        .filter_map(|(_, features)| {
            features
                .features_for(&shared_id)
                .expect("look up shared Guppy features")
        })
        .map(|features| features.named_features().map(str::to_owned).collect())
        .collect()
}
