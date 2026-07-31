use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    match wjsm_bench::runner::run(wjsm_bench::cli::Cli::parse()) {
        Ok(code) => ExitCode::from(u8::try_from(code).expect("benchmark exit code fits u8")),
        Err(error) => {
            eprintln!("benchmark error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
