//! Startup benchmark: measures the time from app init (config loading + agent
//! setup) to first "ready" state, without making any actual API calls.
//!
//! This provides a baseline for tracking startup regression. The goal is
//! startup under 200ms (typically well under on current hardware since it's
//! just file I/O for config loading).

use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

/// Benchmark config loading + tool registry construction (no provider call).
fn bench_config_load(c: &mut Criterion) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();

    // Write a minimal config so load_config succeeds without a real ~/.config.
    let config_dir = root.join(".clido");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
default_profile = "default"

[profiles.default]
provider = "anthropic"
model = "claude-3-5-haiku-20241022"
"#,
    )
    .unwrap();

    c.bench_function("config_load", |b| {
        b.iter(|| {
            let loaded = clido_core::load_config(&root).expect("load_config");
            let _ = std::hint::black_box(loaded);
        });
    });
}

/// Benchmark tool registry construction (the bulk of agent setup sans I/O).
fn bench_registry_build(c: &mut Criterion) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();

    c.bench_function("registry_build", |b| {
        b.iter(|| {
            let r = clido_tools::default_registry_with_blocked(root.clone(), vec![]);
            let _ = std::hint::black_box(r);
        });
    });
}

/// Benchmark the combined startup path: config load + registry build.
/// This approximates the time from binary launch to first API call readiness.
fn bench_startup_combined(c: &mut Criterion) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();

    let config_dir = root.join(".clido");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
default_profile = "default"

[profiles.default]
provider = "anthropic"
model = "claude-3-5-haiku-20241022"
"#,
    )
    .unwrap();

    c.bench_function("startup_combined", |b| {
        b.iter(|| {
            let loaded = clido_core::load_config(&root).expect("load_config");
            let (pricing, _) = clido_core::load_pricing();
            let r = clido_tools::default_registry_with_blocked(root.clone(), vec![]);
            let _ = std::hint::black_box((loaded, pricing, r));
        });
    });
}

criterion_group! {
    name = benches;
    // Reduced sample size and measurement time for CI friendliness.
    config = Criterion::default()
        .sample_size(50)
        .measurement_time(Duration::from_secs(5));
    targets = bench_config_load, bench_registry_build, bench_startup_combined
}
criterion_main!(benches);
