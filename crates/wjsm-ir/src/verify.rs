use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::{
    BasicBlock, BasicBlockId, Constant, ConstantId, Function, HomeObject, Instruction, Module,
    PhiSource, Terminator, ValueId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrVerificationError {
    message: String,
}

impl IrVerificationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for IrVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for IrVerificationError {}

#[derive(Debug, Clone, Copy)]
struct ValueDefinition {
    block: BasicBlockId,
    instruction_index: usize,
}

type Predecessors = HashMap<BasicBlockId, HashSet<BasicBlockId>>;
type Successors = HashMap<BasicBlockId, Vec<BasicBlockId>>;
type Definitions = HashMap<ValueId, ValueDefinition>;
type Dominators = HashMap<BasicBlockId, HashSet<BasicBlockId>>;

pub(crate) fn verify_module(module: &Module) -> Result<(), IrVerificationError> {
    verify_module_constants(module)?;
    for (index, function) in module.functions().iter().enumerate() {
        verify_function(module, index, function)?;
    }
    Ok(())
}

fn verify_module_constants(module: &Module) -> Result<(), IrVerificationError> {
    let function_count = module.functions().len();
    for (index, constant) in module.constants().iter().enumerate() {
        if let Constant::FunctionRef(function_id) = constant
            && function_id.0 as usize >= function_count
        {
            return Err(IrVerificationError::new(format!(
                "IR verification failed: constant c{index} references missing function @{function_id}"
            )));
        }
    }
    Ok(())
}

fn verify_function(
    module: &Module,
    function_index: usize,
    function: &Function,
) -> Result<(), IrVerificationError> {
    let block_count = function.blocks().len();
    if function.block_by_id(function.entry()).is_none() {
        return Err(function_error(
            function,
            format_args!("entry block {} does not exist", function.entry()),
        ));
    }
    verify_function_id_refs(module, function)?;

    let mut predecessors = empty_predecessors(function);
    let mut successors = HashMap::new();

    for (index, block) in function.blocks().iter().enumerate() {
        if block.id().0 as usize != index {
            return Err(function_error(
                function,
                format_args!(
                    "block index {index} has id {}; block ids must match their function order",
                    block.id()
                ),
            ));
        }

        if !block.instructions().is_empty() && matches!(block.terminator(), Terminator::Unreachable)
        {
            return Err(block_error(
                function,
                block,
                format_args!("block has instructions but its terminator is still unreachable"),
            ));
        }

        verify_instruction_constant_refs(module, function, block)?;
        verify_terminator_constant_refs(module, function, block)?;

        let block_successors = terminator_successors(block.terminator());
        for successor in &block_successors {
            if successor.0 as usize >= block_count {
                return Err(block_error(
                    function,
                    block,
                    format_args!("terminator targets missing block {successor}"),
                ));
            }
            predecessors
                .entry(*successor)
                .or_default()
                .insert(block.id());
        }
        successors.insert(block.id(), block_successors);
    }

    let definitions = collect_definitions(function)?;
    let reachable = reachable_blocks(function, &successors);
    let dominators = compute_dominators(function, &predecessors, &reachable);
    verify_phi_sources(
        function,
        &predecessors,
        &definitions,
        &reachable,
        &dominators,
    )?;
    verify_non_phi_uses(function, &definitions, &predecessors, &successors)?;

    let _ = function_index;
    Ok(())
}

fn empty_predecessors(function: &Function) -> Predecessors {
    function
        .blocks()
        .iter()
        .map(|block| (block.id(), HashSet::new()))
        .collect()
}

fn verify_instruction_constant_refs(
    module: &Module,
    function: &Function,
    block: &BasicBlock,
) -> Result<(), IrVerificationError> {
    for instruction in block.instructions() {
        if let Instruction::Const { constant, .. } = instruction {
            verify_constant_id(module, function, block, *constant)?;
        }
    }
    Ok(())
}

fn verify_terminator_constant_refs(
    module: &Module,
    function: &Function,
    block: &BasicBlock,
) -> Result<(), IrVerificationError> {
    if let Terminator::Switch { cases, .. } = block.terminator() {
        for case in cases {
            verify_constant_id(module, function, block, case.constant)?;
        }
    }
    Ok(())
}

fn verify_constant_id(
    module: &Module,
    function: &Function,
    block: &BasicBlock,
    constant: ConstantId,
) -> Result<(), IrVerificationError> {
    if constant.0 as usize >= module.constants().len() {
        return Err(block_error(
            function,
            block,
            format_args!("references missing constant {constant}"),
        ));
    }
    Ok(())
}

fn verify_function_id_refs(
    module: &Module,
    function: &Function,
) -> Result<(), IrVerificationError> {
    let Some(home_object) = function.home_object else {
        return Ok(());
    };
    let function_id = match home_object {
        HomeObject::Prototype(function_id) | HomeObject::Constructor(function_id) => function_id,
    };
    if module.functions().get(function_id.0 as usize).is_none() {
        return Err(function_error(
            function,
            format_args!("invalid home_object function id @{function_id}"),
        ));
    }
    Ok(())
}

fn collect_definitions(function: &Function) -> Result<Definitions, IrVerificationError> {
    let mut definitions = HashMap::new();
    for block in function.blocks() {
        for (instruction_index, instruction) in block.instructions().iter().enumerate() {
            let Some(dest) = instruction_dest(instruction) else {
                continue;
            };
            if let Some(previous) = definitions.insert(
                dest,
                ValueDefinition {
                    block: block.id(),
                    instruction_index,
                },
            ) {
                return Err(block_error(
                    function,
                    block,
                    format_args!(
                        "value {dest} is defined more than once; first definition was in {}",
                        previous.block
                    ),
                ));
            }
        }
    }
    Ok(definitions)
}

fn verify_phi_sources(
    function: &Function,
    predecessors: &Predecessors,
    definitions: &Definitions,
    reachable: &HashSet<BasicBlockId>,
    dominators: &Dominators,
) -> Result<(), IrVerificationError> {
    for block in function.blocks() {
        let actual_predecessors = predecessors.get(&block.id()).expect("block key must exist");
        for instruction in block.instructions() {
            let Instruction::Phi { sources, .. } = instruction else {
                continue;
            };
            if sources.is_empty() {
                return Err(block_error(
                    function,
                    block,
                    format_args!("phi instruction has no sources"),
                ));
            }
            if block.id() == function.entry() {
                return Err(block_error(
                    function,
                    block,
                    format_args!("entry block must not contain phi instruction"),
                ));
            }
            let mut seen = HashSet::new();
            for source in sources {
                verify_phi_source_predecessor(
                    function,
                    block,
                    source,
                    actual_predecessors,
                    &mut seen,
                )?;
                verify_value_use(
                    function,
                    definitions,
                    source.value,
                    ValueUseSite::PhiSource {
                        phi_block: block.id(),
                        predecessor: source.predecessor,
                    },
                    Some((reachable, dominators)),
                )?;
            }
            for predecessor in actual_predecessors {
                if !seen.contains(predecessor) {
                    return Err(block_error(
                        function,
                        block,
                        format_args!(
                            "phi instruction is missing source for predecessor {predecessor}"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn verify_phi_source_predecessor(
    function: &Function,
    block: &BasicBlock,
    source: &PhiSource,
    actual_predecessors: &HashSet<BasicBlockId>,
    seen: &mut HashSet<BasicBlockId>,
) -> Result<(), IrVerificationError> {
    if !seen.insert(source.predecessor) {
        return Err(block_error(
            function,
            block,
            format_args!(
                "phi instruction has duplicate source for predecessor {}",
                source.predecessor
            ),
        ));
    }
    if !actual_predecessors.contains(&source.predecessor) {
        return Err(block_error(
            function,
            block,
            format_args!(
                "phi source predecessor {} is not an actual predecessor of {}",
                source.predecessor,
                block.id()
            ),
        ));
    }
    Ok(())
}

fn verify_non_phi_uses(
    function: &Function,
    definitions: &Definitions,
    predecessors: &Predecessors,
    successors: &Successors,
) -> Result<(), IrVerificationError> {
    let reachable = reachable_blocks(function, successors);
    let dominators = compute_dominators(function, predecessors, &reachable);

    for block in function.blocks() {
        for (instruction_index, instruction) in block.instructions().iter().enumerate() {
            verify_instruction_uses(
                function,
                definitions,
                instruction,
                ValueUseSite::Instruction {
                    block: block.id(),
                    instruction_index,
                },
                Some((&reachable, &dominators)),
            )?;
        }
        verify_terminator_uses(
            function,
            definitions,
            block.terminator(),
            block.id(),
            Some((&reachable, &dominators)),
        )?;
    }
    Ok(())
}

fn verify_instruction_uses(
    function: &Function,
    definitions: &Definitions,
    instruction: &Instruction,
    site: ValueUseSite,
    dominance: Option<(&HashSet<BasicBlockId>, &Dominators)>,
) -> Result<(), IrVerificationError> {
    match instruction {
        Instruction::Const { .. } | Instruction::LoadVar { .. } | Instruction::Phi { .. } => {}
        Instruction::Binary { lhs, rhs, .. } | Instruction::Compare { lhs, rhs, .. } => {
            verify_value_use(function, definitions, *lhs, site, dominance)?;
            verify_value_use(function, definitions, *rhs, site, dominance)?;
        }
        Instruction::Unary { value, .. }
        | Instruction::StoreVar { value, .. }
        | Instruction::ObjectSpread { source: value, .. }
        | Instruction::PromiseResolve { value, .. }
        | Instruction::PromiseReject { reason: value, .. }
        | Instruction::Suspend { promise: value, .. }
        | Instruction::GeneratorSuspend { result: value, .. }
        | Instruction::IsException { value, .. }
        | Instruction::EncodeException { value, .. }
        | Instruction::ExceptionToObject { value, .. } => {
            verify_value_use(function, definitions, *value, site, dominance)?;
        }
        Instruction::CallBuiltin { args, .. } => {
            verify_value_slice(function, definitions, args, site, dominance)?;
        }
        Instruction::StringConcatVa { parts, .. } => {
            verify_value_slice(function, definitions, parts, site, dominance)?;
        }
        Instruction::Call {
            callee,
            this_val,
            args,
            ..
        }
        | Instruction::ConstructCall {
            callee,
            this_val,
            args,
            ..
        } => {
            verify_value_use(function, definitions, *callee, site, dominance)?;
            verify_value_use(function, definitions, *this_val, site, dominance)?;
            verify_value_slice(function, definitions, args, site, dominance)?;
        }
        Instruction::SuperCall {
            callee,
            this_val,
            args,
            forward_args,
            ..
        } => {
            if *forward_args && !args.is_empty() {
                return Err(function_error(
                    function,
                    format_args!("super_call cannot combine forward_args with explicit args"),
                ));
            }
            verify_value_use(function, definitions, *callee, site, dominance)?;
            verify_value_use(function, definitions, *this_val, site, dominance)?;
            verify_value_slice(function, definitions, args, site, dominance)?;
        }
        Instruction::GetProp { object, key, .. }
        | Instruction::DeleteProp { object, key, .. }
        | Instruction::GetElem {
            object, index: key, ..
        }
        | Instruction::OptionalGetProp { object, key, .. }
        | Instruction::OptionalGetElem { object, key, .. } => {
            verify_value_use(function, definitions, *object, site, dominance)?;
            verify_value_use(function, definitions, *key, site, dominance)?;
        }
        Instruction::SetProp { object, key, value }
        | Instruction::SetElem {
            object,
            index: key,
            value,
        } => {
            verify_value_use(function, definitions, *object, site, dominance)?;
            verify_value_use(function, definitions, *key, site, dominance)?;
            verify_value_use(function, definitions, *value, site, dominance)?;
        }
        Instruction::SetProto { object, value } => {
            verify_value_use(function, definitions, *object, site, dominance)?;
            verify_value_use(function, definitions, *value, site, dominance)?;
        }
        Instruction::OptionalCall {
            callee,
            this_val,
            args,
            ..
        } => {
            verify_value_use(function, definitions, *callee, site, dominance)?;
            verify_value_use(function, definitions, *this_val, site, dominance)?;
            verify_value_slice(function, definitions, args, site, dominance)?;
        }
        Instruction::NewObject { .. }
        | Instruction::NewArray { .. }
        | Instruction::GetSuperBase { .. }
        | Instruction::GetSuperConstructor { .. }
        | Instruction::NewPromise { .. }
        | Instruction::CollectRestArgs { .. }
        | Instruction::DebugCheck { .. } => {}
    }
    Ok(())
}

fn verify_value_slice(
    function: &Function,
    definitions: &Definitions,
    values: &[ValueId],
    site: ValueUseSite,
    dominance: Option<(&HashSet<BasicBlockId>, &Dominators)>,
) -> Result<(), IrVerificationError> {
    for value in values {
        verify_value_use(function, definitions, *value, site, dominance)?;
    }
    Ok(())
}

fn verify_terminator_uses(
    function: &Function,
    definitions: &Definitions,
    terminator: &Terminator,
    block: BasicBlockId,
    dominance: Option<(&HashSet<BasicBlockId>, &Dominators)>,
) -> Result<(), IrVerificationError> {
    let site = ValueUseSite::Terminator { block };
    match terminator {
        Terminator::Return { value: Some(value) } | Terminator::Throw { value } => {
            verify_value_use(function, definitions, *value, site, dominance)?;
        }
        Terminator::Branch { condition, .. }
        | Terminator::Switch {
            value: condition, ..
        } => {
            verify_value_use(function, definitions, *condition, site, dominance)?;
        }
        Terminator::Return { value: None } | Terminator::Jump { .. } | Terminator::Unreachable => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum ValueUseSite {
    Instruction {
        block: BasicBlockId,
        instruction_index: usize,
    },
    Terminator {
        block: BasicBlockId,
    },
    PhiSource {
        phi_block: BasicBlockId,
        predecessor: BasicBlockId,
    },
}

impl ValueUseSite {
    fn block(self) -> BasicBlockId {
        match self {
            Self::Instruction { block, .. } | Self::Terminator { block } => block,
            Self::PhiSource { predecessor, .. } => predecessor,
        }
    }

    fn instruction_index(self) -> usize {
        match self {
            Self::Instruction {
                instruction_index, ..
            } => instruction_index,
            Self::Terminator { .. } | Self::PhiSource { .. } => usize::MAX,
        }
    }

    fn describe(self) -> String {
        match self {
            Self::Instruction {
                block,
                instruction_index,
            } => format!("instruction #{instruction_index} in {block}"),
            Self::Terminator { block } => format!("terminator in {block}"),
            Self::PhiSource {
                phi_block,
                predecessor,
                ..
            } => format!("phi source for predecessor {predecessor} in {phi_block}"),
        }
    }
}

fn verify_value_use(
    function: &Function,
    definitions: &Definitions,
    value: ValueId,
    site: ValueUseSite,
    dominance: Option<(&HashSet<BasicBlockId>, &Dominators)>,
) -> Result<(), IrVerificationError> {
    let Some(definition) = definitions.get(&value).copied() else {
        return Err(function_error(
            function,
            format_args!("undefined value {value} used by {}", site.describe()),
        ));
    };

    if definition.block == site.block() && definition.instruction_index >= site.instruction_index()
    {
        return Err(function_error(
            function,
            format_args!(
                "value {value} is used before definition by {}; definition is instruction #{} in {}",
                site.describe(),
                definition.instruction_index,
                definition.block
            ),
        ));
    }

    if let Some((reachable, dominators)) = dominance {
        let use_block = site.block();
        if reachable.contains(&use_block)
            && (!reachable.contains(&definition.block)
                || !dominators
                    .get(&use_block)
                    .is_some_and(|block_dominators| block_dominators.contains(&definition.block)))
        {
            return Err(function_error(
                function,
                format_args!(
                    "definition of value {value} in {} does not dominate use by {}",
                    definition.block,
                    site.describe()
                ),
            ));
        }
    }

    Ok(())
}

fn reachable_blocks(function: &Function, successors: &Successors) -> HashSet<BasicBlockId> {
    let mut reachable = HashSet::new();
    let mut stack = vec![function.entry()];
    while let Some(block) = stack.pop() {
        if !reachable.insert(block) {
            continue;
        }
        if let Some(block_successors) = successors.get(&block) {
            for successor in block_successors {
                stack.push(*successor);
            }
        }
    }
    reachable
}

fn compute_dominators(
    function: &Function,
    predecessors: &Predecessors,
    reachable: &HashSet<BasicBlockId>,
) -> Dominators {
    let mut dominators = HashMap::new();
    for block in function.blocks() {
        let initial = if block.id() == function.entry() {
            HashSet::from([block.id()])
        } else if reachable.contains(&block.id()) {
            reachable.clone()
        } else {
            HashSet::from([block.id()])
        };
        dominators.insert(block.id(), initial);
    }

    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks() {
            let block_id = block.id();
            if block_id == function.entry() || !reachable.contains(&block_id) {
                continue;
            }

            let reachable_predecessors: Vec<_> = predecessors
                .get(&block_id)
                .into_iter()
                .flatten()
                .copied()
                .filter(|predecessor| reachable.contains(predecessor))
                .collect();

            let mut next = if let Some((first, rest)) = reachable_predecessors.split_first() {
                let mut intersection = dominators.get(first).cloned().unwrap_or_default();
                for predecessor in rest {
                    if let Some(predecessor_dominators) = dominators.get(predecessor) {
                        intersection.retain(|item| predecessor_dominators.contains(item));
                    }
                }
                intersection
            } else {
                HashSet::new()
            };
            next.insert(block_id);

            if dominators.get(&block_id) != Some(&next) {
                dominators.insert(block_id, next);
                changed = true;
            }
        }
    }

    dominators
}

fn instruction_dest(instruction: &Instruction) -> Option<ValueId> {
    match instruction {
        Instruction::Const { dest, .. }
        | Instruction::Binary { dest, .. }
        | Instruction::Unary { dest, .. }
        | Instruction::Compare { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::StringConcatVa { dest, .. }
        | Instruction::LoadVar { dest, .. }
        | Instruction::NewObject { dest, .. }
        | Instruction::GetProp { dest, .. }
        | Instruction::DeleteProp { dest, .. }
        | Instruction::NewArray { dest, .. }
        | Instruction::GetElem { dest, .. }
        | Instruction::OptionalGetProp { dest, .. }
        | Instruction::OptionalGetElem { dest, .. }
        | Instruction::OptionalCall { dest, .. }
        | Instruction::ObjectSpread { dest, .. }
        | Instruction::GetSuperBase { dest }
        | Instruction::GetSuperConstructor { dest }
        | Instruction::NewPromise { dest }
        | Instruction::CollectRestArgs { dest, .. }
        | Instruction::IsException { dest, .. }
        | Instruction::EncodeException { dest, .. }
        | Instruction::ExceptionToObject { dest, .. } => Some(*dest),
        Instruction::CallBuiltin { dest, .. }
        | Instruction::Call { dest, .. }
        | Instruction::SuperCall { dest, .. }
        | Instruction::ConstructCall { dest, .. } => *dest,
        Instruction::StoreVar { .. }
        | Instruction::SetProp { .. }
        | Instruction::SetProto { .. }
        | Instruction::SetElem { .. }
        | Instruction::PromiseResolve { .. }
        | Instruction::PromiseReject { .. }
        | Instruction::Suspend { .. }
        | Instruction::GeneratorSuspend { .. }
        | Instruction::DebugCheck { .. } => None,
    }
}

fn terminator_successors(terminator: &Terminator) -> Vec<BasicBlockId> {
    let mut successors = Vec::new();
    match terminator {
        Terminator::Return { .. } | Terminator::Throw { .. } | Terminator::Unreachable => {}
        Terminator::Jump { target } => push_unique(&mut successors, *target),
        Terminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            push_unique(&mut successors, *true_block);
            push_unique(&mut successors, *false_block);
        }
        Terminator::Switch {
            cases,
            default_block,
            exit_block,
            ..
        } => {
            for case in cases {
                push_unique(&mut successors, case.target);
            }
            push_unique(&mut successors, *default_block);
            push_unique(&mut successors, *exit_block);
        }
    }
    successors
}

fn push_unique(values: &mut Vec<BasicBlockId>, value: BasicBlockId) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn function_error(function: &Function, detail: fmt::Arguments<'_>) -> IrVerificationError {
    IrVerificationError::new(format!(
        "IR verification failed in function @{}: {detail}",
        function.name()
    ))
}

fn block_error(
    function: &Function,
    block: &BasicBlock,
    detail: fmt::Arguments<'_>,
) -> IrVerificationError {
    function_error(function, format_args!("block {}: {detail}", block.id()))
}
