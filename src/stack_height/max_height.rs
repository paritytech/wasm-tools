use parity_wasm::elements::{self, Type, BlockType};

struct Frame {
	/// Stack becomes polymorphic only after an instruction that
	/// never passes control further was executed.
	is_polymorphic: bool,

	/// Count of values which will be pushed after the exit
	/// from the current block.
	end_arity: u32,

	/// Count of values which should be poped upon a branch to
	/// this frame.
	///
	/// This might be diffirent from `end_arity` since branch
	/// to loop header can't take any values.
	branch_arity: u32,

	/// Stack height before entering in the block.
	start_height: u32,
}

struct Context {
	height: u32,
	control_stack: Vec<Frame>,
}

impl Context {
	fn new() -> Context {
		Context {
			height: 0,
			control_stack: Vec::new(),
		}
	}

	/// Returns current height of the value stack.
	fn height(&self) -> u32 {
		self.height
	}

	/// Returns a reference to a frame by specified depth relative to the top of
	/// control stack.
	fn frame(&self, rel_depth: u32) -> &Frame {
		let control_stack_height: usize = self.control_stack.len();
		let last_idx = control_stack_height
			.checked_sub(1)
			.expect("control stack is empty?");
		let idx = last_idx
			.checked_sub(rel_depth as usize)
			.expect("control stack out-of-bounds");
		&self.control_stack[idx]
	}

	/// Mark successive instructions as unreachable.
	///
	/// This effectively makes stack polymorphic.
	fn mark_unreachable(&mut self) {
		let top_frame = self.control_stack
			.last_mut()
			.expect("stack must be non-empty");
		top_frame.is_polymorphic = true;
	}

	/// Push control frame into the control stack.
	fn push_frame(&mut self, frame: Frame) {
		self.control_stack.push(frame);
	}

	/// Pop control frame from the control stack.
	///
	/// This function will panic if the control stack is empty.
	fn pop_frame(&mut self) -> Frame {
		self.control_stack.pop().expect("stack must be non-empty")
	}

	/// Truncate the height of value stack to the specified height.
	fn trunc(&mut self, new_height: u32) {
		self.height = new_height;
	}

	/// Push specified number of values into the value stack.
	///
	/// This will panic if the height overflow usize value.
	fn push_values(&mut self, value_count: u32) {
		self.height = self.height
			.checked_add(value_count)
			.expect("stack overflow");
	}

	/// Pop specified number of values from the value stack.
	///
	/// This will panic if the stack happen to be negative value after
	/// values popped.
	fn pop_values(&mut self, value_count: u32) {
		{
			let top_frame = self.frame(0);
			if self.height == top_frame.start_height {
				// It is an error to pop more values than was pushed in the current frame
				// (ie pop values pushed in the parent frame), unless the frame become
				// polymorphic.
				if top_frame.is_polymorphic {
					return;
				} else {
					panic!("trying to pop more values than pushed");
				}
			}
		}

		self.height = self.height
			.checked_sub(value_count)
			.expect("stack underflow");
	}
}

/// This function expects the function to be validated.
pub fn max_stack_height(func_idx: u32, module: &elements::Module) -> u32 {
	use parity_wasm::elements::Opcode::*;

	let func_section = module
		.function_section()
		.expect("Due to validation func section should exists");
	let type_section = module
		.type_section()
		.expect("Due to validation types section should exists");
	let code_section = module
		.code_section()
		.expect("Due to validation code section should exists");

	// Get a signature and a body of the specified function.
	let func_sig_idx = func_section.entries()[func_idx as usize].type_ref();
	let Type::Function(ref func_signature) = type_section.types()[func_sig_idx as usize];
	let body = &code_section.bodies()[func_idx as usize];
	let opcodes = body.code();

	let mut ctx = Context::new();
	let mut max_height: u32 = 0;
	let mut pc = 0;

	// Add implicit frame for the function. Breaks to this frame and execution of
	// the last end should deal with this frame.
	let func_arity: u32 = if func_signature.return_type().is_some() {
		1
	} else {
		0
	};
	ctx.push_frame(Frame {
		is_polymorphic: false,
		end_arity: func_arity,
		branch_arity: func_arity,
		start_height: 0,
	});

	loop {
		if pc >= opcodes.elements().len() {
			break;
		}

		// If current value stack is higher than maximal height observed so far,
		// save the new height.
		// However, we don't increase maximal value in unreachable code.
		if ctx.height() > max_height && !ctx.frame(0).is_polymorphic {
			max_height = ctx.height();
		}

		let opcode = &opcodes.elements()[pc];
		match *opcode {
			Nop => {}
			Block(ty) | Loop(ty) | If(ty) => {
				let end_arity = if ty == BlockType::NoResult { 0 } else { 1 };
				let branch_arity = if let Loop(_) = *opcode { 0 } else { end_arity };
				let height = ctx.height();
				ctx.push_frame(Frame {
					is_polymorphic: false,
					end_arity,
					branch_arity,
					start_height: height,
				});
			}
			Else => {
				// The frame at the top should be pushed by `If`. So we leave
				// it as is.
			}
			End => {
				let frame = ctx.pop_frame();
				ctx.trunc(frame.start_height);
				ctx.push_values(frame.end_arity);
			}
			Unreachable => {
				ctx.mark_unreachable();
			}
			Br(target) => {
				// Pop values for the destination block result.
				let target_arity = ctx.frame(target).branch_arity;
				ctx.pop_values(target_arity);

				// This instruction unconditionally transfers control to the specified block,
				// thus all instruction until the end of the current block is deemed unreachable
				ctx.mark_unreachable();
			}
			BrIf(target) => {
				// Pop values for the destination block result.
				let target_arity = ctx.frame(target).branch_arity;
				ctx.pop_values(target_arity);

				// Pop condition value.
				ctx.pop_values(1);
			}
			BrTable(ref targets, default_target) => {
				let arity_of_default = ctx.frame(default_target).branch_arity;

				// Check that all jump targets have an equal arities.
				debug_assert!({
					targets
						.iter()
						.map(|target_rel_depth| ctx.frame(*target_rel_depth).branch_arity)
						.all(|arity| arity == arity_of_default)
				});

				// Because all jump targets have an equal arities, we can just take arity of
				// the default branch.
				ctx.pop_values(arity_of_default);

				// This instruction doesn't let control flow to go further, since the control flow
				// should take either one of branches depending on the value or the default branch.
				ctx.mark_unreachable();
			}
			Return => {
				// Pop return values of the function. Mark successive instructions as unreachable
				// since this instruction doesn't let control flow to go further.
				ctx.pop_values(func_arity);
				ctx.mark_unreachable();
			}
			Call(x) | CallIndirect(x, _) => {
				let Type::Function(ref ty) = type_section.types()[x as usize];

				// Pop values for arguments of the function.
				ctx.pop_values(ty.params().len() as u32);

				// Push result of the function execution to the stack.
				let callee_arity = if ty.return_type().is_some() { 1 } else { 0 };
				ctx.push_values(callee_arity);
			}
			Drop => {
				ctx.pop_values(1);
			}
			Select => {
				// Pop two values and one condition.
				ctx.pop_values(2);
				ctx.pop_values(1);

				// Push the selected value.
				ctx.push_values(1);
			}
			GetLocal(_) => {
				ctx.push_values(1);
			}
			SetLocal(_) => {
				ctx.pop_values(1);
			}
			TeeLocal(_) => {
				// This instruction pops and pushes the value, so
				// effectively it doesn't modify the stack height.
				ctx.pop_values(1);
				ctx.push_values(1);
			}
			GetGlobal(_) => {
				ctx.push_values(1);
			}
			SetGlobal(_) => {
				ctx.pop_values(1);
			}
			I32Load(_, _)
			| I64Load(_, _)
			| F32Load(_, _)
			| F64Load(_, _)
			| I32Load8S(_, _)
			| I32Load8U(_, _)
			| I32Load16S(_, _)
			| I32Load16U(_, _)
			| I64Load8S(_, _)
			| I64Load8U(_, _)
			| I64Load16S(_, _)
			| I64Load16U(_, _)
			| I64Load32S(_, _)
			| I64Load32U(_, _) => {
				// These instructions pop the address and pushes the result,
				// which effictively don't modify the stack height.
				ctx.pop_values(1);
				ctx.push_values(1);
			}

			I32Store(_, _)
			| I64Store(_, _)
			| F32Store(_, _)
			| F64Store(_, _)
			| I32Store8(_, _)
			| I32Store16(_, _)
			| I64Store8(_, _)
			| I64Store16(_, _)
			| I64Store32(_, _) => {
				// These instructions pop the address and the value.
				ctx.pop_values(2);
			}

			CurrentMemory(_) => {
				// Pushes current memory size
				ctx.push_values(1);
			}
			GrowMemory(_) => {
				// Grow memory takes the value of pages to grow and pushes
				ctx.pop_values(1);
				ctx.push_values(1);
			}

			I32Const(_) | I64Const(_) | F32Const(_) | F64Const(_) => {
				// These instructions just push the single literal value onto the stack.
				ctx.push_values(1);
			}

			I32Eqz | I64Eqz => {
				// These instructions pop the value and compare it against zero, and pushes
				// the result of the comparison.
				ctx.pop_values(1);
				ctx.push_values(1);
			}

			I32Eq | I32Ne | I32LtS | I32LtU | I32GtS | I32GtU | I32LeS | I32LeU | I32GeS
			| I32GeU | I64Eq | I64Ne | I64LtS | I64LtU | I64GtS | I64GtU | I64LeS | I64LeU
			| I64GeS | I64GeU | F32Eq | F32Ne | F32Lt | F32Gt | F32Le | F32Ge | F64Eq | F64Ne
			| F64Lt | F64Gt | F64Le | F64Ge => {
				// Comparison operations take two operands and produce one result.
				ctx.pop_values(2);
				ctx.push_values(1);
			}

			I32Clz | I32Ctz | I32Popcnt | I64Clz | I64Ctz | I64Popcnt | F32Abs | F32Neg
			| F32Ceil | F32Floor | F32Trunc | F32Nearest | F32Sqrt | F64Abs | F64Neg | F64Ceil
			| F64Floor | F64Trunc | F64Nearest | F64Sqrt => {
				// Unary operators take one operand and produce one result.
				ctx.pop_values(1);
				ctx.push_values(1);
			}

			I32Add | I32Sub | I32Mul | I32DivS | I32DivU | I32RemS | I32RemU | I32And | I32Or
			| I32Xor | I32Shl | I32ShrS | I32ShrU | I32Rotl | I32Rotr | I64Add | I64Sub
			| I64Mul | I64DivS | I64DivU | I64RemS | I64RemU | I64And | I64Or | I64Xor | I64Shl
			| I64ShrS | I64ShrU | I64Rotl | I64Rotr | F32Add | F32Sub | F32Mul | F32Div
			| F32Min | F32Max | F32Copysign | F64Add | F64Sub | F64Mul | F64Div | F64Min
			| F64Max | F64Copysign => {
				// Binary operators take two operands and produce one result.
				ctx.pop_values(2);
				ctx.push_values(1);
			}

			I32WrapI64 | I32TruncSF32 | I32TruncUF32 | I32TruncSF64 | I32TruncUF64
			| I64ExtendSI32 | I64ExtendUI32 | I64TruncSF32 | I64TruncUF32 | I64TruncSF64
			| I64TruncUF64 | F32ConvertSI32 | F32ConvertUI32 | F32ConvertSI64 | F32ConvertUI64
			| F32DemoteF64 | F64ConvertSI32 | F64ConvertUI32 | F64ConvertSI64 | F64ConvertUI64
			| F64PromoteF32 | I32ReinterpretF32 | I64ReinterpretF64 | F32ReinterpretI32
			| F64ReinterpretI64 => {
				// Conversion operators take one value and produce one result.
				ctx.pop_values(1);
				ctx.push_values(1);
			}
		}
		pc += 1;
	}

	max_height
}

#[cfg(test)]
mod tests {
	extern crate wabt;
	use parity_wasm::elements;
	use super::*;

	fn parse_wat(source: &str) -> elements::Module {
		elements::deserialize_buffer(&wabt::wat2wasm(source).expect("Failed to wat2wasm"))
			.expect("Failed to deserialize the module")
	}

	#[test]
	fn simple_test() {
		let module = parse_wat(
			r#"
(module
	(func
		i32.const 1
			i32.const 2
				i32.const 3
				drop
			drop
		drop
	)
)
"#,
		);

		let height = max_stack_height(0, &module);
		assert_eq!(height, 3);
	}

	#[test]
	fn implicit_and_explicit_return() {
		let module = parse_wat(
			r#"
(module
	(func (result i32)
		i32.const 0
		return
	)
)
"#,
		);

		let height = max_stack_height(0, &module);
		assert_eq!(height, 1);
	}

	#[test]
	fn dont_count_in_unreachable() {
		let module = parse_wat(
			r#"
(module
  (memory 0)
  (func (result i32)
	unreachable
	grow_memory
  )
)
"#,
		);

		let height = max_stack_height(0, &module);
		assert_eq!(height, 0);
	}

	#[test]
	fn yet_another_test() {
		const SOURCE: &'static str = r#"
(module
  (memory 0)
  (func
    ;; Push two values and then pop them.
    ;; This will make max depth to be equal to 2.
    i32.const 0
    i32.const 1
    drop
    drop

    ;; Code after `unreachable` shouldn't have an effect
    ;; on the max depth.
    unreachable
    i32.const 0
    i32.const 1
    i32.const 2
  )
)
"#;
		let module = elements::deserialize_buffer(&wabt::Wat2Wasm::new()
			.validate(false)
			.convert(SOURCE)
			.expect("Failed to wat2wasm")
			.as_ref())
			.expect("Failed to deserialize the module");

		let height = max_stack_height(0, &module);
		assert_eq!(height, 2);
	}
}
