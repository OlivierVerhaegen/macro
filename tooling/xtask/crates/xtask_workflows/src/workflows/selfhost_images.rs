//! Olive Trees fork: build the self-hosted images and push them to GHCR.
//!
//! Runs only outside `macro-inc/macro`, on GitHub-hosted runners (the fork has
//! no Namespace profiles). The build logic lives in `infra/selfhost/images/`;
//! this file only wires it to triggers and runners. Images are tagged with the
//! commit SHA and the branch; the server pins one SHA. Generated into
//! `selfhost-images.yml`.

use gh_workflow::{
    Concurrency, Event, Expression, Job, Level, Permissions, Push, Run, Step, Strategy, Use,
    Workflow, WorkflowDispatch,
};

use crate::workflows::steps;

const RUNNER: &str = "ubuntu-24.04";
const NOT_UPSTREAM: &str = "github.repository != 'macro-inc/macro'";

/// Build the workflow.
pub fn selfhost_images() -> Workflow {
    Workflow::new("Self-hosted images")
        .permissions(Permissions {
            contents: Some(Level::Read),
            packages: Some(Level::Write),
            ..Default::default()
        })
        .on(Event::default()
            .push(Push::default().add_branch("main").add_branch("ota/**"))
            .workflow_dispatch(WorkflowDispatch::default()))
        .concurrency(
            Concurrency::new(Expression::new("${{ github.workflow }}-${{ github.ref }}"))
                .cancel_in_progress(true),
        )
        .add_job("rust", rust_job())
        .add_job("web", web_job())
        .add_job("node", node_job())
}

fn base_job(name: &str, timeout_minutes: u32) -> Job {
    Job::default()
        .name(name)
        .cond(Expression::new(NOT_UPSTREAM))
        .runs_on(RUNNER)
        .timeout_minutes(timeout_minutes)
}

/// `publish.sh` with the token and registry it needs.
fn publish(name: &str, args: &str) -> Step<Run> {
    Step::new("Build and push image")
        .run(format!("infra/selfhost/images/publish.sh {name} {args}"))
        .add_env(("REGISTRY", "ghcr.io/${{ github.repository_owner }}"))
        .add_env(("GITHUB_TOKEN", "${{ secrets.GITHUB_TOKEN }}"))
}

/// All Rust services in one image. A release build of the workspace needs
/// more disk than the runner has free, hence the cleanup.
fn rust_job() -> Job {
    base_job("Rust services", 300)
        .add_step(free_disk_space())
        .add_step(steps::checkout(false, false))
        .add_step(cargo_cache("restore"))
        .add_step(
            Step::new("Build binaries")
                .run("infra/selfhost/images/ci-build-rust.sh target/selfhost-image"),
        )
        // Saved even when a later step fails or the run is cancelled, so a long
        // build is never redone from scratch.
        .add_step(cargo_cache("save").if_condition(Expression::new("always()")))
        .add_step(publish(
            "macro-rust",
            "infra/selfhost/images/rust.Dockerfile target/selfhost-image --check-libs",
        ))
}

fn free_disk_space() -> Step<Run> {
    Step::new("Free disk space").run(
        "sudo rm -rf /usr/share/dotnet /usr/local/lib/android /opt/ghc /opt/hostedtoolcache/CodeQL\ndf -h /",
    )
}

/// Dependencies only: workspace crates rebuild on every commit anyway.
/// `action` is `restore` or `save` (the two halves of `actions/cache`).
fn cargo_cache(action: &str) -> Step<Use> {
    let key =
        "selfhost-rust-${{ hashFiles('Cargo.lock', 'rust-toolchain.toml') }}-${{ github.sha }}";
    let step = Step::new(if action == "save" { "Save cargo cache" } else { "Restore cargo cache" })
        .uses(
            "actions",
            format!("cache/{action}"),
            "5a3ec84eff668545956fd18022155c47e93e2684",
        ) // v4.2.3
        .add_with((
            "path",
            "~/.cargo/registry/index\n~/.cargo/registry/cache\n~/.cargo/git/db\ntarget/release/deps\ntarget/release/build\ntarget/release/.fingerprint",
        ))
        .add_with(("key", key));
    if action == "save" {
        step
    } else {
        step.add_with((
            "restore-keys",
            "selfhost-rust-${{ hashFiles('Cargo.lock', 'rust-toolchain.toml') }}-\nselfhost-rust-",
        ))
    }
}

/// The local-backend bundle `cargo x stack up` serves: every service call goes
/// to the page's own origin, where the public Caddy routes it.
fn web_job() -> Job {
    base_job("Web app", 120)
        .add_env(("CI", "true"))
        .add_step(steps::checkout(false, false))
        .add_step(steps::setup_nix())
        .add_step(steps::setup_reqs_web("Setup", false))
        .add_step(
            Step::new("Build")
                .run("just build-dev")
                .working_directory(xtask_paths::repo_dir!("apps/web"))
                .add_env(("VITE_LOCAL_SERVERS", "ALL"))
                .add_env(("VITE_LOCAL_BACKEND_ORIGIN", "same-origin")),
        )
        .add_step(publish(
            "macro-web",
            "infra/selfhost/images/web.Dockerfile apps/web/dist",
        ))
        .add_step(steps::teardown_nix())
}

/// The JavaScript services, from the Dockerfiles the local stack uses.
fn node_job() -> Job {
    base_job("${{ matrix.name }}", 90)
        .strategy(Strategy {
            fail_fast: Some(false),
            matrix: Some(serde_json::json!({
                "include": [
                    { "name": "macro-sync", "dockerfile": "docker/sync-service.Dockerfile" },
                    { "name": "macro-lexical", "dockerfile": "docker/lexical-service.Dockerfile" },
                    { "name": "macro-websocket", "dockerfile": "docker/websocket-service.Dockerfile" },
                ],
            })),
            max_parallel: None,
        })
        .add_step(steps::checkout(false, false))
        .add_step(publish("${{ matrix.name }}", "${{ matrix.dockerfile }} ."))
}
