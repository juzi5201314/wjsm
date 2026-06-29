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
        input: Option<String>,

        /// The output .wasm file, or - for stdout
        #[arg(short, long, default_value = "out.wasm")]
        output: String,

        /// Stop at a specific pipeline stage
        #[arg(long, value_name = "STAGE")]
        stage: Option<Stage>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,

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
        input: Option<String>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,

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

    /// Parse and check a JS/TS file for errors (no output)
    Check {
        /// The input file to check, or - for stdin. Optional when -e is used.
        input: Option<String>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,

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

    /// Dump IR for a JS/TS file
    DumpIr {
        /// The input file, or - for stdin. Optional when -e is used.
        input: Option<String>,

        /// Output format (text or dot for Graphviz)
        #[arg(long, default_value = "text")]
        format: DumpFormat,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,

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
        input: Option<String>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,

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
        input: Option<String>,

        /// The root directory for module resolution
        #[arg(long)]
        root: Option<String>,

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

        /// Disassemble only the function with this name
        #[arg(long, value_name = "NAME")]
        func: Option<String>,

        /// Print function signatures only, no instruction bodies
        #[arg(long)]
        skeleton: bool,
    },

    /// Create a new wjsm project
    Init {
        /// The project directory to create
        path: String,

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
