use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Verbose output (-v shows progress, -vv shows details)
    #[arg(short = 'v', long, action = clap::ArgAction::Count, global = true)]
    pub(crate) verbose: u8,

    /// Show timing information for each pipeline stage
    #[arg(long, global = true)]
    pub(crate) time: bool,

    /// Show statistics after build (constants, functions, blocks, instructions, WASM size)
    #[arg(long, global = true)]
    pub(crate) stats: bool,

    /// Color output control (auto/always/never). Also respects NO_COLOR env var.
    #[arg(long, value_name = "WHEN", global = true)]
    pub(crate) color: Option<ColorChoice>,

    /// Target backend (wasm or jit)
    #[arg(long, default_value = "wasm", global = true)]
    pub(crate) target: Target,

    /// GC algorithm (runtime; mark-sweep is the only implementation now, generational/incremental reserved)
    #[arg(long, default_value = "mark-sweep", global = true)]
    pub(crate) gc_algorithm: GcAlgorithmChoice,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ColorChoice {
    /// Auto-detect based on terminal
    Auto,
    /// Always use colors
    Always,
    /// Never use colors
    Never,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum Target {
    Wasm,
    Jit,
}

/// GC 算法选择（运行期切换，spec §6 trait 框架）。当前仅 MarkSweep；预留 generational/incremental。
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum GcAlgorithmChoice {
    /// Non-moving mark-sweep + segregated free list（默认实现，spec §8/§9）
    #[value(alias = "mark_sweep")]
    MarkSweep,
    // 未来：Generational, Incremental（impl 新 struct，不改框架）
}

#[derive(Clone, Copy, Debug, ValueEnum)]
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
        /// The input file to compile, or - for stdin
        input: String,

        /// The output .wasm file, or - for stdout
        #[arg(short, long, default_value = "out.wasm")]
        output: String,

        /// Stop at a specific pipeline stage
        #[arg(long, value_name = "STAGE")]
        stage: Option<Stage>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,
    },

    /// Run a JS/TS file directly
    Run {
        /// The input file to run, or - for stdin
        input: String,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,

        /// Watch for changes and re-run
        #[arg(short, long)]
        watch: bool,

        /// Parse as script instead of module (allows await as identifier)
        #[arg(long)]
        script: bool,
    },

    /// Parse and check a JS/TS file for errors (no output)
    Check {
        /// The input file to check, or - for stdin
        input: String,
    },

    /// Evaluate a JS expression and print the result
    Eval {
        /// The JS expression to evaluate
        code: String,
    },

    /// Dump IR for a JS/TS file
    DumpIr {
        /// The input file, or - for stdin
        input: String,

        /// Output format (text or dot for Graphviz)
        #[arg(long, default_value = "text")]
        format: DumpFormat,
    },

    /// Dump SWC AST as JSON for a JS/TS file
    DumpAst {
        /// The input file, or - for stdin
        input: String,
    },

    /// Dump WAT (WebAssembly Text) for a compiled JS/TS file
    DumpWat {
        /// The input file, or - for stdin
        input: String,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,
    },

    /// Format a JS/TS file using SWC codegen
    Fmt {
        /// The input file to format
        input: String,

        /// Write formatted output back to the file
        #[arg(short, long)]
        write: bool,
    },

    /// Validate a .wasm file
    Validate {
        /// The .wasm file to validate
        input: String,
    },

    /// Show size breakdown of WASM sections
    Size {
        /// The .wasm file to analyze
        input: String,
    },

    /// Disassemble a .wasm file with detailed output
    Disasm {
        /// The .wasm file to disassemble
        input: String,
    },

    /// Create a new wjsm project
    Init {
        /// The project directory to create
        path: String,
    },

    /// Show extended version information
    Version {
        /// Show extended version info
        #[arg(long)]
        extended: bool,
    },
}
