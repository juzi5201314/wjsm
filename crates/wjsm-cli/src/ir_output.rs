use anyhow::{Result, bail};
use colored::Colorize;
use std::fmt::Write;
use std::sync::OnceLock;
use wjsm_ir::Program;

use super::PipelineResult;

pub(crate) fn print_ir(program: &Program) {
    let text = program.dump_text();

    // Check if colors are enabled
    if colored::control::SHOULD_COLORIZE.should_colorize() {
        for line in text.lines() {
            let colored_line = colorize_ir_line(line);
            println!("{}", colored_line);
        }
    } else {
        println!("{}", text);
    }
}

/// 输出单个函数的 IR 文本（含常量块，使 cN 引用可解析）。
pub(crate) fn print_ir_func(program: &Program, name: &str) -> Result<()> {
    let func = match program.functions().iter().find(|f| f.name() == name) {
        Some(f) => f,
        None => bail!("function '{name}' not found"),
    };
    let mut out = String::from("module {\n");
    if program.constants().is_empty() {
        out.push_str("  constants: []\n");
    } else {
        out.push_str("  constants:\n");
        for (index, constant) in program.constants().iter().enumerate() {
            let _ = writeln!(out, "    c{index} = {constant}");
        }
    }
    out.push('\n');
    out.push_str(&func.dump_text());
    out.push_str("}\n");

    if colored::control::SHOULD_COLORIZE.should_colorize() {
        for line in out.lines() {
            let colored_line = colorize_ir_line(line);
            println!("{}", colored_line);
        }
    } else {
        println!("{}", out);
    }

    Ok(())
}

/// DOT 标签字符串转义：反斜杠、双引号、换行。
fn escape_dot_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
    out
}

/// 在双引号字符串字面量之外对片段着色（避免 string("call") 内误匹配关键字）。
fn colorize_outside_string_literals<F>(line: &str, mut colorize: F) -> String
where
    F: FnMut(&str) -> String,
{
    let mut out = String::with_capacity(line.len() * 2);
    let mut in_string = false;
    let mut segment_start = 0usize;
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
            if !in_string {
                let segment = &line[segment_start..i];
                out.push_str(&colorize(segment));
                in_string = true;
                segment_start = i;
            } else {
                out.push_str(&line[segment_start..=i]);
                in_string = false;
                segment_start = i + 1;
            }
        }
        i += 1;
    }
    if segment_start < line.len() {
        let segment = &line[segment_start..];
        if in_string {
            out.push_str(segment);
        } else {
            out.push_str(&colorize(segment));
        }
    }
    out
}


fn colorize_ir_line(line: &str) -> String {
    static VALUE_RE: OnceLock<regex::Regex> = OnceLock::new();
    static SCOPE_RE: OnceLock<regex::Regex> = OnceLock::new();
    static CONST_RE: OnceLock<regex::Regex> = OnceLock::new();

    // Keywords in blue
    let keywords = [
        "module", "fn", "entry=", "bb", "return", "call", "const", "jump", "branch",
    ];

    let mut result = line.to_string();

    // Color types (like "number", "string") in green — 跳过字符串字面量内部
    result = colorize_outside_string_literals(&result, |seg| {
        let mut s = seg.to_string();
        s = s.replace("number(", &"number(".green().to_string());
        s = s.replace("string(", &"string(".green().to_string());
        s
    });

    // Color keywords in blue — 跳过字符串字面量内部
    result = colorize_outside_string_literals(&result, |seg| {
        let mut s = seg.to_string();
        for kw in &keywords {
            s = s.replace(kw, &kw.blue().to_string());
        }
        s
    });

    // Color values (like %0, $0.x) in cyan
    if result.contains('%') {
        let re = VALUE_RE.get_or_init(|| regex::Regex::new(r"%\d+").unwrap());
        result = re
            .replace_all(&result, |caps: &regex::Captures| caps[0].cyan().to_string())
            .to_string();
    }

    // Color scope-qualified names like $0.x in cyan
    if result.contains('$') {
        let re = SCOPE_RE.get_or_init(|| regex::Regex::new(r"\$\d+\.\w+").unwrap());
        result = re
            .replace_all(&result, |caps: &regex::Captures| caps[0].cyan().to_string())
            .to_string();
    }

    // Color constants like c0, c1 in yellow
    if result.contains(" c") || result.starts_with('c') {
        let re = CONST_RE.get_or_init(|| regex::Regex::new(r"\bc\d+").unwrap());
        result = re
            .replace_all(&result, |caps: &regex::Captures| {
                caps[0].yellow().to_string()
            })
            .to_string();
    }

    result
}

pub(crate) fn print_ir_dot(program: &Program) {
    println!("digraph IR {{");
    println!("  rankdir=TB;");
    println!("  node [shape=box];");
    println!();

    // For each function
    for func in program.functions() {
        let func_name = escape_dot_label(func.name());
        println!("  subgraph \"cluster_{}\" {{", func_name);
        println!("    label=\"{}\";", func_name);

        println!("    style=rounded;");
        println!();

        // Create nodes for each basic block using actual block IDs
        for bb in func.blocks() {
            let bb_id = bb.id();
            let inst_lines: String = bb
                .instructions()
                .iter()
                .map(|inst| escape_dot_label(&format!("  {}", inst)))
                .collect::<Vec<_>>()
                .join("\\l");
            let label = format!("{}\\l{}", bb_id, inst_lines);
            println!("    bb{} [label=\"{}\"];", bb_id.0, label);
        }

        // Create edges for control flow using actual block IDs
        for bb in func.blocks() {
            let bb_id = bb.id();
            use wjsm_ir::Terminator;
            match bb.terminator() {
                Terminator::Return { .. } => {
                    // No outgoing edges
                }
                Terminator::Jump { target } => {
                    println!("    bb{} -> bb{};", bb_id.0, target.0);
                }
                Terminator::Branch {
                    condition: _,
                    true_block,
                    false_block,
                } => {
                    println!("    bb{} -> bb{} [label=\"true\"];", bb_id.0, true_block.0);
                    println!(
                        "    bb{} -> bb{} [label=\"false\"];",
                        bb_id.0, false_block.0
                    );
                }
                Terminator::Switch {
                    value: _,
                    cases,
                    default_block,
                    exit_block,
                } => {
                    for case in cases {
                        println!("    bb{} -> bb{};", bb_id.0, case.target.0);
                    }
                    println!(
                        "    bb{} -> bb{} [label=\"default\"];",
                        bb_id.0, default_block.0
                    );
                    println!("    bb{} -> bb{} [label=\"exit\"];", bb_id.0, exit_block.0);
                }
                Terminator::Throw { .. } => {
                    // No outgoing edges
                }
                Terminator::Unreachable => {
                    // No outgoing edges
                }
            }
        }

        println!("  }}");
    }

    println!("}}");
}

pub(crate) fn print_stats(result: &PipelineResult) {
    eprintln!();
    eprintln!("=== Statistics ===");

    if let Some(program) = &result.program {
        let mut total_blocks = 0;
        let mut total_instructions = 0;

        for func in program.functions() {
            total_blocks += func.blocks().len();
            for bb in func.blocks() {
                total_instructions += bb.instructions().len();
            }
        }

        eprintln!("  Constants: {}", program.constants().len());
        eprintln!("  Functions: {}", program.functions().len());
        eprintln!("  Basic Blocks: {}", total_blocks);
        eprintln!("  Instructions: {}", total_instructions);
    }

    if let Some(wasm) = &result.wasm {
        eprintln!("  WASM Size: {} bytes", wasm.len());
    }
}
