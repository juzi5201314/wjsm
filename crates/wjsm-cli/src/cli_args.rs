use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use std::path::PathBuf;

pub(crate) fn parse_heap_size(raw: &str) -> Result<usize, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("heap size must not be empty".to_string());
    }
    let split_at = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (digits, suffix) = trimmed.split_at(split_at);
    if digits.is_empty() {
        return Err(format!("invalid heap size `{raw}`"));
    }
    let value = digits
        .parse::<usize>()
        .map_err(|_| format!("invalid heap size `{raw}`"))?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        _ => return Err(format!("unsupported heap size suffix `{suffix}`")),
    };
    let bytes = value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("heap size `{raw}` is too large"))?;
    if bytes == 0 {
        return Err("heap size must be greater than zero".to_string());
    }
    Ok(bytes)
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Load defaults from a wjsm.toml/wjsm.json configuration file
    #[arg(long, value_name = "PATH", global = true)]
    pub(crate) config: Option<PathBuf>,

    /// Suppress non-essential diagnostic output
    #[arg(short = 'q', long, global = true)]
    pub(crate) quiet: bool,

    /// Verbose output (-v shows progress, -vv shows details)
    #[arg(short = 'v', long, action = clap::ArgAction::Count, global = true)]
    pub(crate) verbose: u8,

    /// Show timing information for each pipeline stage
    #[arg(long, global = true)]
    pub(crate) time: bool,

    /// Show statistics after build (constants, functions, blocks, instructions, WASM size)
    #[arg(long, global = true)]
    pub(crate) stats: bool,

    /// Verify lowered IR invariants before continuing past lower stage
    #[arg(long, global = true)]
    pub(crate) verify_ir: bool,

    /// Color output control (auto/always/never). Also respects NO_COLOR env var.
    #[arg(long, value_name = "WHEN", global = true)]
    pub(crate) color: Option<ColorChoice>,

    /// Disable colored output
    #[arg(long, global = true, conflicts_with = "color")]
    pub(crate) no_color: bool,

    /// Target backend (wasm or jit)
    #[arg(long, default_value = "wasm", global = true)]
    pub(crate) target: Target,

    /// Limit JavaScript heap allocations (bytes, or K/M/G suffixes)
    #[arg(long, value_name = "SIZE", global = true, value_parser = parse_heap_size)]
    pub(crate) max_heap_size: Option<usize>,
}

impl Cli {
    pub(crate) fn verbose_enabled(&self, level: u8) -> bool {
        !self.quiet && self.verbose >= level
    }

    pub(crate) fn effective_verbose(&self) -> u8 {
        if self.quiet { 0 } else { self.verbose }
    }

    pub(crate) fn should_verify_ir(&self) -> bool {
        self.verify_ir
    }
}

#[derive(Clone, Copy, Debug, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ColorChoice {
    /// Auto-detect based on terminal
    Auto,
    /// Always use colors
    Always,
    /// Never use colors
    Never,
}

#[derive(Clone, Copy, Debug, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Target {
    Wasm,
    Jit,
}

#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub(crate) enum DumpFormat {
    Text,
    Dot,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Stage {
    /// Parse and print AST JSON
    Parse,
    /// Lower to IR and print
    Lower,
    /// Compile to WASM and write output
    Compile,
    /// Compile and execute (default for run)
    Execute,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Build a JS/TS file to WebAssembly
    Build {
        /// The input file to compile, or - for stdin. Optional when -e is used.
        input: Option<PathBuf>,

        /// The output .wasm file, or - for stdout
        #[arg(short, long, default_value = "out.wasm")]
        output: PathBuf,

        /// Stop at a specific pipeline stage
        #[arg(long, value_name = "STAGE")]
        stage: Option<Stage>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<PathBuf>,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,

        /// Evaluate inline code string instead of a file
        #[arg(short, long = "eval")]
        eval: Option<String>,
    },

    /// Run a JS/TS file directly
    Run {
        /// The input file to run, or - for stdin. Optional when -e is used.
        input: Option<PathBuf>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<PathBuf>,

        /// Watch for changes and re-run
        #[arg(short, long)]
        watch: bool,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,

        /// Evaluate inline code string instead of a file
        #[arg(short, long = "eval")]
        eval: Option<String>,
    },

    /// Run JS/TS test files or an inline test snippet
    Test {
        /// File or directory to test. Directories run *.test.js/*.test.ts and *_test.js/*_test.ts.
        input: Option<PathBuf>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<PathBuf>,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,

        /// Evaluate inline test code instead of discovering files
        #[arg(short, long = "eval")]
        eval: Option<String>,
    },

    /// Parse and check a JS/TS file for errors (no output)
    Check {
        /// The input file to check, or - for stdin. Optional when -e is used.
        input: Option<PathBuf>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<PathBuf>,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,

        /// Evaluate inline code string instead of a file
        #[arg(short, long = "eval")]
        eval: Option<String>,
    },

    /// Lint a JS/TS file or inline source
    Lint {
        /// The input file to lint, or - for stdin. Optional when -e is used.
        input: Option<PathBuf>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<PathBuf>,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,

        /// Evaluate inline code string instead of a file
        #[arg(short, long = "eval")]
        eval: Option<String>,
    },

    /// Evaluate a JS expression and print the result
    Eval {
        /// The JS expression to evaluate
        code: String,
    },

    /// Start an interactive expression REPL
    Repl {
        /// Evaluate one expression through the REPL pipeline and exit
        #[arg(short, long = "eval")]
        eval: Option<String>,

        /// Parse REPL input as script instead of module
        #[arg(long)]
        script: bool,
    },

    /// Dump IR for a JS/TS file
    DumpIr {
        /// The input file, or - for stdin. Optional when -e is used.
        input: Option<PathBuf>,

        /// Output format (text or dot for Graphviz)
        #[arg(long, default_value = "text")]
        format: DumpFormat,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<PathBuf>,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,

        /// Evaluate inline code string instead of a file
        #[arg(short, long = "eval")]
        eval: Option<String>,

        /// Dump only the function with this name
        #[arg(long, value_name = "NAME")]
        func: Option<String>,
    },

    /// Dump SWC AST as JSON for a JS/TS file
    DumpAst {
        /// The input file, or - for stdin. Optional when -e is used.
        input: Option<PathBuf>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<PathBuf>,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,

        /// Evaluate inline code string instead of a file
        #[arg(short, long = "eval")]
        eval: Option<String>,
    },

    /// Dump WAT (WebAssembly Text) for a compiled JS/TS file
    DumpWat {
        /// The input file, or - for stdin. Optional when -e is used.
        input: Option<PathBuf>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<PathBuf>,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,

        /// Evaluate inline code string instead of a file
        #[arg(short, long = "eval")]
        eval: Option<String>,

        /// Dump only the function with this name
        #[arg(long, value_name = "NAME")]
        func: Option<String>,

        /// Print function signatures only, no instruction bodies
        #[arg(long)]
        skeleton: bool,
    },

    /// Format a JS/TS file using SWC codegen
    Fmt {
        /// The input file to format
        input: PathBuf,

        /// Write formatted output back to the file
        #[arg(short, long)]
        write: bool,
    },

    /// Validate a .wasm file
    Validate {
        /// The .wasm file to validate
        input: PathBuf,
    },

    /// Show size breakdown of WASM sections
    Size {
        /// The .wasm file to analyze
        input: PathBuf,
    },

    /// Disassemble a .wasm file with detailed output
    Disasm {
        /// The .wasm file to disassemble
        input: PathBuf,

        /// Disassemble only the function with this name
        #[arg(long, value_name = "NAME")]
        func: Option<String>,

        /// Print function signatures only, no instruction bodies
        #[arg(long)]
        skeleton: bool,
    },

    /// Inspect or clear the compiled WASM cache
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },

    /// Create a new wjsm project
    Init {
        /// The project directory to create
        path: PathBuf,

        /// Overwrite existing project files
        #[arg(long)]
        force: bool,
    },

    /// Show extended version information
    Version {
        /// Show extended version info
        #[arg(long)]
        extended: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum CacheCommand {
    /// Show compiled WASM cache location and size
    Stats,
    /// Remove compiled WASM cache entries
    Clear,
}
