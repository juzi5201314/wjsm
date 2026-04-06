use anyhow::Result;

use crate::fixture_runner::FixtureRunner;

#[test]
fn happy() -> Result<()> {
    FixtureRunner::new()?.run_suite("happy")
}

#[test]
fn errors() -> Result<()> {
    FixtureRunner::new()?.run_suite("errors")
}
