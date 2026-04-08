use anyhow::{Result, bail};
use wjsm_ir::Program;

pub fn compile(_program: &Program) -> Result<Vec<u8>> {
    bail!("JIT backend is not implemented yet")
}
