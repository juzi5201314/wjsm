use super::*;

mod control_analysis;
use control_analysis::{chain_jumps_to, resolve_jump_chain};
mod control_locals;
mod control_structured;
mod control_switch;
mod control_branch;
