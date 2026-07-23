use anyhow::Result;

use crate::cli::{Cli, Command};
use crate::report::write_json;
use crate::resource::HostInfo;
use crate::run::run_bench;

pub fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Run(args) => {
            let report = run_bench(&args)?;
            write_json(&args.common.output, &report)?;
            eprintln!(
                "wjsm-gc-bench: {} / {} / {} → {}",
                report.config.gc,
                report.config.scenario,
                report.config.heap_bytes,
                args.common.output.display()
            );
            Ok(0)
        }
        Command::Info(args) => {
            let info = HostInfo::detect();
            write_json(&args.output, &info)?;
            eprintln!("wjsm-gc-bench: host info → {}", args.output.display());
            Ok(0)
        }
    }
}
