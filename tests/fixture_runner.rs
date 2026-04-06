use anyhow::{Context, Result, bail};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const UPDATE_SNAPSHOTS_ENV: &str = "WJSM_UPDATE_FIXTURES";

pub struct FixtureRunner {
    binary_path: PathBuf,
    fixtures_root: PathBuf,
    update_snapshots: bool,
}

struct FixtureCase {
    input_path: PathBuf,
    expected_path: PathBuf,
    relative_path: PathBuf,
}

struct FixtureOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

impl FixtureRunner {
    pub fn new() -> Result<Self> {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixtures_root = manifest_dir.join("fixtures");
        let binary_path = resolve_binary_path(&manifest_dir)?;
        let update_snapshots = snapshot_updates_enabled();

        Ok(Self {
            binary_path,
            fixtures_root,
            update_snapshots,
        })
    }

    pub fn run_suite(&self, suite: &str) -> Result<()> {
        let fixtures = self.discover(suite)?;
        let mut failures = Vec::new();

        for fixture in fixtures {
            if let Err(error) = self.run_fixture(&fixture) {
                failures.push(format!("{}\n{error:#}", fixture.relative_path.display()));
            }
        }

        if failures.is_empty() {
            return Ok(());
        }

        bail!(
            "fixture suite '{suite}' failed with {} case(s):\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    fn discover(&self, suite: &str) -> Result<Vec<FixtureCase>> {
        let suite_dir = self.fixtures_root.join(suite);
        if !suite_dir.is_dir() {
            bail!(
                "Fixture suite directory does not exist: {}",
                suite_dir.display()
            );
        }

        let mut fixtures = Vec::new();
        self.collect_cases(&suite_dir, &mut fixtures)?;
        fixtures.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

        if fixtures.is_empty() {
            bail!("Fixture suite '{suite}' does not contain any .js/.ts files");
        }

        Ok(fixtures)
    }

    fn collect_cases(&self, dir: &Path, fixtures: &mut Vec<FixtureCase>) -> Result<()> {
        let mut entries = fs::read_dir(dir)
            .with_context(|| format!("Failed to read fixture directory: {}", dir.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.path());

        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                self.collect_cases(&path, fixtures)?;
                continue;
            }

            if !is_fixture_file(&path) {
                continue;
            }

            let relative_path = path
                .strip_prefix(&self.fixtures_root)
                .with_context(|| format!("Failed to build relative path for {}", path.display()))?
                .to_path_buf();

            fixtures.push(FixtureCase {
                expected_path: path.with_extension("expected"),
                input_path: path,
                relative_path,
            });
        }

        Ok(())
    }

    fn run_fixture(&self, fixture: &FixtureCase) -> Result<()> {
        let output = Command::new(&self.binary_path)
            .arg("run")
            .arg(&fixture.input_path)
            .output()
            .with_context(|| {
                format!(
                    "Failed to execute fixture binary for {}",
                    fixture.relative_path.display()
                )
            })?;

        let actual = FixtureOutput::from_command_output(output).snapshot();
        if !fixture.expected_path.exists() {
            fs::write(&fixture.expected_path, &actual).with_context(|| {
                format!(
                    "Failed to create snapshot file {}",
                    fixture.expected_path.display()
                )
            })?;
            return Ok(());
        }

        let expected = fs::read_to_string(&fixture.expected_path).with_context(|| {
            format!(
                "Failed to read snapshot file {}",
                fixture.expected_path.display()
            )
        })?;

        if expected == actual {
            return Ok(());
        }

        if self.update_snapshots {
            fs::write(&fixture.expected_path, &actual).with_context(|| {
                format!(
                    "Failed to update snapshot file {}",
                    fixture.expected_path.display()
                )
            })?;
            return Ok(());
        }

        bail!(
            "snapshot mismatch: {}\n--- expected ---\n{}\n--- actual ---\n{}",
            fixture.expected_path.display(),
            expected,
            actual
        );
    }
}

impl FixtureOutput {
    fn from_command_output(output: std::process::Output) -> Self {
        Self {
            stdout: normalize_output(&output.stdout),
            stderr: normalize_output(&output.stderr),
            exit_code: output.status.code().unwrap_or(-1),
        }
    }

    fn snapshot(&self) -> String {
        let mut snapshot = String::new();
        snapshot.push_str(&format!("exit_code: {}\n", self.exit_code));
        snapshot.push_str("--- stdout ---\n");
        snapshot.push_str(&self.stdout);
        if !self.stdout.is_empty() && !self.stdout.ends_with('\n') {
            snapshot.push('\n');
        }

        snapshot.push_str("--- stderr ---\n");
        snapshot.push_str(&self.stderr);
        if !self.stderr.is_empty() && !self.stderr.ends_with('\n') {
            snapshot.push('\n');
        }

        snapshot
    }
}

fn resolve_binary_path(manifest_dir: &Path) -> Result<PathBuf> {
    if let Ok(binary_path) = env::var("CARGO_BIN_EXE_wjsm") {
        return Ok(PathBuf::from(binary_path));
    }

    let fallback = manifest_dir
        .join("target")
        .join("debug")
        .join(binary_name());
    if fallback.exists() {
        return Ok(fallback);
    }

    bail!("Unable to locate wjsm binary. Build tests with cargo/nextest first.")
}

fn snapshot_updates_enabled() -> bool {
    matches!(
        env::var(UPDATE_SNAPSHOTS_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

fn normalize_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

fn is_fixture_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(OsStr::to_str),
        Some("js") | Some("ts")
    )
}

fn binary_name() -> &'static str {
    if cfg!(windows) { "wjsm.exe" } else { "wjsm" }
}
