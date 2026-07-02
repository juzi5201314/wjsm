use anyhow::{Context, Result};
use clap::parser::ValueSource;
use clap::{CommandFactory, FromArgMatches};
use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::cli_args::{Cli, ColorChoice, Commands, Target};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct CliConfig {
    quiet: Option<bool>,
    verbose: Option<u8>,
    time: Option<bool>,
    stats: Option<bool>,
    color: Option<ColorChoice>,
    no_color: Option<bool>,
    target: Option<Target>,
    root: Option<PathBuf>,
    script: Option<bool>,
}

pub(crate) fn parse_cli<I, T>(args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut command = Cli::command();
    let matches = command.try_get_matches_from_mut(args)?;
    let mut cli = Cli::from_arg_matches(&matches)?;

    if let Some(path) = config_path(cli.config.as_deref()) {
        let config = load_config(&path).map_err(|error| {
            clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                format!("failed to load config '{}': {error:#}", path.display()),
            )
        })?;
        apply_global_config(&mut cli, &matches, &config);
        if let Some((_, sub_matches)) = matches.subcommand() {
            apply_command_config(&mut cli.command, sub_matches, &config);
        }
        cli.config = Some(path);
    }

    Ok(cli)
}

fn config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }

    let cwd = std::env::current_dir().ok()?;
    ["wjsm.toml", "wjsm.json"]
        .into_iter()
        .map(|name| cwd.join(name))
        .find(|path| path.is_file())
}

fn load_config(path: &Path) -> Result<CliConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config '{}';", path.display()))?;
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => load_json_config(&text),
        _ => load_toml_config(&text),
    }
}

fn load_json_config(text: &str) -> Result<CliConfig> {
    let value: serde_json::Value = serde_json::from_str(text).context("invalid JSON config")?;
    let cli_value = value.get("cli").cloned().unwrap_or(value);
    serde_json::from_value(cli_value).context("invalid CLI config")
}

fn load_toml_config(text: &str) -> Result<CliConfig> {
    let value: toml::Value = toml::from_str(text).context("invalid TOML config")?;
    let cli_value = value.get("cli").cloned().unwrap_or(value);
    cli_value.try_into().context("invalid CLI config")
}

fn command_line_global(matches: &clap::ArgMatches, id: &str) -> bool {
    matches.value_source(id) == Some(ValueSource::CommandLine)
}

fn command_line_subcommand(matches: &clap::ArgMatches, id: &str) -> bool {
    matches.value_source(id) == Some(ValueSource::CommandLine)
}

fn apply_global_config(cli: &mut Cli, matches: &clap::ArgMatches, config: &CliConfig) {
    if let Some(quiet) = config.quiet
        && !command_line_global(matches, "quiet")
    {
        cli.quiet = quiet;
    }
    if let Some(verbose) = config.verbose
        && !command_line_global(matches, "verbose")
    {
        cli.verbose = verbose;
    }
    if let Some(time) = config.time
        && !command_line_global(matches, "time")
    {
        cli.time = time;
    }
    if let Some(stats) = config.stats
        && !command_line_global(matches, "stats")
    {
        cli.stats = stats;
    }
    if let Some(target) = config.target
        && !command_line_global(matches, "target")
    {
        cli.target = target;
    }
    if let Some(color) = config.color
        && !command_line_global(matches, "color")
        && !command_line_global(matches, "no_color")
    {
        cli.color = Some(color);
        cli.no_color = false;
    }
    if let Some(no_color) = config.no_color
        && !command_line_global(matches, "color")
        && !command_line_global(matches, "no_color")
    {
        cli.no_color = no_color;
        if no_color {
            cli.color = Some(ColorChoice::Never);
        }
    }
}

fn apply_command_config(command: &mut Commands, matches: &clap::ArgMatches, config: &CliConfig) {
    match command {
        Commands::Build { root, script, .. }
        | Commands::Run { root, script, .. }
        | Commands::Test { root, script, .. }
        | Commands::Check { root, script, .. }
        | Commands::Lint { root, script, .. }
        | Commands::DumpIr { root, script, .. }
        | Commands::DumpAst { root, script, .. }
        | Commands::DumpWat { root, script, .. } => {
            apply_root(root, matches, config);
            apply_script(script, matches, config);
        }
        Commands::Repl { script, .. } => apply_script(script, matches, config),
        Commands::Eval { .. }
        | Commands::Fmt { .. }
        | Commands::Validate { .. }
        | Commands::Size { .. }
        | Commands::Disasm { .. }
        | Commands::Cache { .. }
        | Commands::Completions { .. }
        | Commands::Init { .. }
        | Commands::Version { .. } => {}
    }
}

fn apply_root(root: &mut Option<PathBuf>, matches: &clap::ArgMatches, config: &CliConfig) {
    if let Some(config_root) = &config.root
        && !command_line_subcommand(matches, "root")
    {
        *root = Some(config_root.clone());
    }
}

fn apply_script(script: &mut bool, matches: &clap::ArgMatches, config: &CliConfig) {
    if let Some(config_script) = config.script
        && !command_line_subcommand(matches, "script")
    {
        *script = config_script;
    }
}
