use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueHint};
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_HORIZONTAL_ONLY};

mod config;
mod exec;
mod read;

use config::should_run_test;
use exec::{SuiteResults, TestResult};
use read::{read_harness, read_suite, read_test};

const TEST262_DIRECTORY: &str = "test262";

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 运行 Test262 测试套件。
    Run {
        /// 指定 Test262 子目录（相对 test262 目录的路径）。
        #[arg(short, long, default_value = "test", value_hint = ValueHint::AnyPath)]
        suite: PathBuf,

        /// 运行所有测试，不按 features 过滤。
        #[arg(long)]
        all: bool,

        /// 输出 JSON 结果到指定文件。
        #[arg(short, long, value_hint = ValueHint::FilePath)]
        json: Option<PathBuf>,

        /// 纯文本输出模式。
        #[arg(long)]
        plain: bool,

        /// 串行执行测试。
        #[arg(long)]
        no_parallel: bool,

        /// 显示详细输出。
        #[arg(short, long, action = clap::ArgAction::Count)]
        verbose: u8,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            suite,
            all,
            json,
            plain,
            no_parallel,
            verbose,
        } => {
            run_test262(suite, all, json, plain, !no_parallel, verbose)
        }
    }
}

fn run_test262(
    suite: PathBuf,
    all: bool,
    json_output: Option<PathBuf>,
    plain: bool,
    parallel: bool,
    verbose: u8,
) -> Result<()> {
    let test262_path = Path::new(TEST262_DIRECTORY);

    if !test262_path.is_dir() {
        bail!(
            "test262 directory not found at {}. Please run `cargo build` first to clone it.",
            test262_path.display()
        );
    }

    if verbose > 0 {
        println!("Loading harness files...");
    }

    let harness = read_harness(test262_path).context("could not read test262 harness")?;

    let suite_path = test262_path.join(&suite);
    if !suite_path.exists() {
        bail!("suite path does not exist: {}", suite_path.display());
    }

    if suite_path.extension().and_then(|e| e.to_str()) == Some("js") {
        // 单个测试文件
        let test = read_test(&suite_path)
            .with_context(|| format!("could not read test: {}", suite_path.display()))?;

        if verbose > 0 {
            println!("Running single test: {}", suite_path.display());
        }

        let result = exec::run_test(&test, &harness);
        print_single_result(&test, &result);
        return Ok(());
    }

    if verbose > 0 {
        println!("Reading test suite: {}", suite_path.display());
    }

    let suite = read_suite(&suite_path)
        .with_context(|| format!("could not read suite: {}", suite_path.display()))?;

    if verbose > 0 {
        println!("Running tests...");
    }

    let results = exec::run_suite(&suite, &harness, parallel, &|test| should_run_test(test, all));

    // 输出结果
    if let Some(json_path) = json_output {
        write_json_results(&results, &json_path)?;
    } else if plain {
        print_plain_results(&results);
    } else {
        print_table_results(&results);
    }

    Ok(())
}

fn print_single_result(test: &read::Test, result: &TestResult) {
    match result {
        TestResult::Passed => {
            println!("{} {}", "PASS".green(), test.path.display());
        }
        TestResult::Failed { expected, actual } => {
            println!("{} {}", "FAIL".red(), test.path.display());
            println!("  expected: {}", expected);
            println!("  actual:   {}", actual);
        }
        TestResult::Ignored => {
            println!("{} {}", "SKIP".yellow(), test.path.display());
        }
        TestResult::Error(msg) => {
            println!("{} {}: {}", "ERROR".red().bold(), test.path.display(), msg);
        }
    }
}

fn print_plain_results(results: &SuiteResults) {
    let stats = &results.stats;
    println!();
    println!("Results:");
    println!("  Total:   {}", stats.total);
    println!("  Passed:  {}", stats.passed.to_string().green());
    println!("  Failed:  {}", stats.failed.to_string().red());
    println!("  Ignored: {}", stats.ignored.to_string().yellow());
    println!("  Errors:  {}", stats.errors.to_string().red().bold());
    println!(
        "  Rate:    {:.2}%",
        stats.conformance_rate()
    );
    println!("  Time:    {:.2}s", results.duration.as_secs_f64());

    if !results.failures.is_empty() {
        println!();
        println!("Failures (first 10):");
        for failure in results.failures.iter().take(10) {
            println!("  - {}", failure.path);
        }
        if results.failures.len() > 10 {
            println!("  ... and {} more", results.failures.len() - 10);
        }
    }
}

fn print_table_results(results: &SuiteResults) {
    let stats = &results.stats;

    println!();
    let mut table = Table::new();
    table.load_preset(UTF8_HORIZONTAL_ONLY);
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Total Tests", &stats.total.to_string()]);
    table.add_row(vec!["Passed", &stats.passed.to_string().green().to_string()]);
    table.add_row(vec!["Failed", &stats.failed.to_string().red().to_string()]);
    table.add_row(vec!["Ignored", &stats.ignored.to_string().yellow().to_string()]);
    table.add_row(vec!["Errors", &stats.errors.to_string().red().bold().to_string()]);
    table.add_row(vec!["Conformance", &format!("{:.2}%", stats.conformance_rate())]);
    table.add_row(vec!["Duration", &format!("{:.2}s", results.duration.as_secs_f64())]);
    println!("{table}");

    // 按 feature 统计
    if !results.by_feature.is_empty() {
        println!();
        let mut table = Table::new();
        table.load_preset(UTF8_HORIZONTAL_ONLY);
        table.set_header(vec!["Feature", "Total", "Passed", "Failed", "Rate"]);

        let mut features: Vec<_> = results.by_feature.iter().collect();
        features.sort_by_key(|(name, _)| *name);

        for (name, stats) in features {
            let rate = format!("{:.2}%", stats.conformance_rate());
            table.add_row(vec![
                name.as_str(),
                &stats.total.to_string(),
                &stats.passed.to_string(),
                &stats.failed.to_string(),
                &rate,
            ]);
        }

        println!("{table}");
    }

    if !results.failures.is_empty() {
        println!();
        println!("{} failures:", results.failures.len());
        for failure in results.failures.iter().take(20) {
            println!("  - {}", failure.path);
        }
        if results.failures.len() > 20 {
            println!("  ... and {} more", results.failures.len() - 20);
        }
    }
}

fn write_json_results(results: &SuiteResults, path: &Path) -> Result<()> {
    let json = serde_json::json!({
        "total": results.stats.total,
        "passed": results.stats.passed,
        "failed": results.stats.failed,
        "ignored": results.stats.ignored,
        "errors": results.stats.errors,
        "conformance_rate": results.stats.conformance_rate(),
        "duration_seconds": results.duration.as_secs_f64(),
        "by_feature": results.by_feature.iter().map(|(name, stats)| {
            (name.clone(), serde_json::json!({
                "total": stats.total,
                "passed": stats.passed,
                "failed": stats.failed,
                "ignored": stats.ignored,
                "errors": stats.errors,
            }))
        }).collect::<serde_json::Map<_, _>>(),
        "failures": results.failures.iter().map(|f| {
            serde_json::json!({
                "path": f.path,
                "expected": f.expected,
                "actual": f.actual,
            })
        }).collect::<Vec<_>>(),
    });

    std::fs::write(path, serde_json::to_string_pretty(&json)?)
        .with_context(|| format!("could not write JSON results to {}", path.display()))?;

    println!("Results written to {}", path.display());
    Ok(())
}
