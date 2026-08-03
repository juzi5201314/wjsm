use super::*;

mod control_analysis;
pub(crate) use control_analysis::eliminate_empty_jump_blocks;
use control_analysis::{chain_jumps_to, count_predecessors, resolve_jump_chain};
mod control_branch;
mod control_locals;
mod control_structured;
mod control_switch;
