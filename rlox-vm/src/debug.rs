//! Human-readable disassembler for `Chunk`. Output format mirrors the book's
//! ch. 14 `debug.c` so golden-test traces can be compared against clox.
//!
//! Unlike the book we return `String`s rather than printing directly — this
//! lets callers (and tests) decide where the output goes.

use std::fmt::Write as _;

use crate::chunk::{Chunk, OpCode};

/// Disassemble an entire chunk. The header (`== <name> ==`) is followed by
/// one line per instruction.
pub fn disassemble_chunk(chunk: &Chunk, name: &str) -> String {
    let mut out = String::new();
    writeln!(out, "== {name} ==").unwrap();

    let mut offset = 0;
    while offset < chunk.code.len() {
        let (line, next) = disassemble_instruction(chunk, offset);
        out.push_str(&line);
        out.push('\n');
        offset = next;
    }
    out
}

/// Disassemble a single instruction at `offset`, returning the formatted
/// line (no trailing newline) and the byte offset of the next instruction.
pub fn disassemble_instruction(chunk: &Chunk, offset: usize) -> (String, usize) {
    let mut line = String::new();
    write!(line, "{offset:04} ").unwrap();

    // Line-number column: `   |` when this byte shares a line with the
    // previous byte, otherwise the 4-wide right-justified line number.
    if offset > 0 && chunk.lines[offset] == chunk.lines[offset - 1] {
        line.push_str("   |");
    } else {
        write!(line, "{:4}", chunk.lines[offset]).unwrap();
    }
    line.push(' ');

    let byte = chunk.code[offset];
    let Some(op) = OpCode::from_byte(byte) else {
        write!(line, "Unknown opcode {byte}").unwrap();
        return (line, offset + 1);
    };

    match op {
        // Simple 1-byte opcodes.
        OpCode::Nil
        | OpCode::True
        | OpCode::False
        | OpCode::Pop
        | OpCode::Equal
        | OpCode::Greater
        | OpCode::Less
        | OpCode::Add
        | OpCode::Subtract
        | OpCode::Multiply
        | OpCode::Divide
        | OpCode::Not
        | OpCode::Negate
        | OpCode::Print
        | OpCode::CloseUpvalue
        | OpCode::Return
        | OpCode::Inherit => simple(&mut line, op_name(op), offset),

        // Constant-index-style opcodes: 1-byte operand that names a constant
        // (or, for locals/upvalues, a slot index). Formatted identically.
        OpCode::Constant
        | OpCode::GetGlobal
        | OpCode::DefineGlobal
        | OpCode::SetGlobal
        | OpCode::GetProperty
        | OpCode::SetProperty
        | OpCode::GetSuper
        | OpCode::Class
        | OpCode::Method => constant_instruction(&mut line, op_name(op), chunk, offset),

        OpCode::GetLocal | OpCode::SetLocal | OpCode::GetUpvalue | OpCode::SetUpvalue => {
            byte_instruction(&mut line, op_name(op), chunk, offset)
        }

        OpCode::Call => byte_instruction(&mut line, op_name(op), chunk, offset),

        OpCode::Jump | OpCode::JumpIfFalse => {
            jump_instruction(&mut line, op_name(op), 1, chunk, offset)
        }
        OpCode::Loop => jump_instruction(&mut line, op_name(op), -1, chunk, offset),

        OpCode::Invoke | OpCode::SuperInvoke => {
            invoke_instruction(&mut line, op_name(op), chunk, offset)
        }

        OpCode::Closure => closure_instruction(&mut line, chunk, offset),
    }
}

// --- per-shape helpers -----------------------------------------------------

fn simple(line: &mut String, name: &str, offset: usize) -> (String, usize) {
    line.push_str(name);
    (std::mem::take(line), offset + 1)
}

fn constant_instruction(
    line: &mut String,
    name: &str,
    chunk: &Chunk,
    offset: usize,
) -> (String, usize) {
    let idx = chunk.code[offset + 1] as usize;
    // Book format: `%-16s %4d '<value>'`
    write!(line, "{name:<16} {idx:>4} '").unwrap();
    if let Some(v) = chunk.constants.get(idx) {
        write!(line, "{v}").unwrap();
    } else {
        write!(line, "?").unwrap();
    }
    line.push('\'');
    (std::mem::take(line), offset + 2)
}

fn byte_instruction(
    line: &mut String,
    name: &str,
    chunk: &Chunk,
    offset: usize,
) -> (String, usize) {
    let slot = chunk.code[offset + 1] as usize;
    write!(line, "{name:<16} {slot:>4}").unwrap();
    (std::mem::take(line), offset + 2)
}

fn jump_instruction(
    line: &mut String,
    name: &str,
    sign: i32,
    chunk: &Chunk,
    offset: usize,
) -> (String, usize) {
    let hi = chunk.code[offset + 1] as u16;
    let lo = chunk.code[offset + 2] as u16;
    let jump = (hi << 8) | lo;
    // `post` = ip immediately after the 3-byte instruction.
    let post = offset as i64 + 3;
    let target = post + sign as i64 * jump as i64;
    write!(line, "{name:<16} {offset:>4} -> {target}").unwrap();
    (std::mem::take(line), offset + 3)
}

fn invoke_instruction(
    line: &mut String,
    name: &str,
    chunk: &Chunk,
    offset: usize,
) -> (String, usize) {
    let const_idx = chunk.code[offset + 1] as usize;
    let arg_count = chunk.code[offset + 2] as usize;
    write!(line, "{name:<16} ({arg_count} args) {const_idx:>4} '").unwrap();
    if let Some(v) = chunk.constants.get(const_idx) {
        write!(line, "{v}").unwrap();
    } else {
        write!(line, "?").unwrap();
    }
    line.push('\'');
    (std::mem::take(line), offset + 3)
}

fn closure_instruction(line: &mut String, chunk: &Chunk, offset: usize) -> (String, usize) {
    // `OP_CLOSURE  <const-idx>  '<fn>'`, then one indented line per upvalue.
    //
    // We cannot read the embedded upvalue count from the function value until
    // M6 lands `ObjFunction`; for M4 we simply emit the header line and
    // advance past the constant operand. The compiler in M5 will revisit
    // this routine (the book extends it to walk upvalue pairs).
    let const_idx = chunk.code[offset + 1] as usize;
    write!(line, "{:<16} {const_idx:>4} '", "OP_CLOSURE").unwrap();
    if let Some(v) = chunk.constants.get(const_idx) {
        write!(line, "{v}").unwrap();
    } else {
        write!(line, "?").unwrap();
    }
    line.push('\'');
    (std::mem::take(line), offset + 2)
}

// --- opcode → printable name ----------------------------------------------

fn op_name(op: OpCode) -> &'static str {
    match op {
        OpCode::Constant => "OP_CONSTANT",
        OpCode::Nil => "OP_NIL",
        OpCode::True => "OP_TRUE",
        OpCode::False => "OP_FALSE",
        OpCode::Pop => "OP_POP",
        OpCode::GetLocal => "OP_GET_LOCAL",
        OpCode::SetLocal => "OP_SET_LOCAL",
        OpCode::GetGlobal => "OP_GET_GLOBAL",
        OpCode::DefineGlobal => "OP_DEFINE_GLOBAL",
        OpCode::SetGlobal => "OP_SET_GLOBAL",
        OpCode::GetUpvalue => "OP_GET_UPVALUE",
        OpCode::SetUpvalue => "OP_SET_UPVALUE",
        OpCode::GetProperty => "OP_GET_PROPERTY",
        OpCode::SetProperty => "OP_SET_PROPERTY",
        OpCode::GetSuper => "OP_GET_SUPER",
        OpCode::Equal => "OP_EQUAL",
        OpCode::Greater => "OP_GREATER",
        OpCode::Less => "OP_LESS",
        OpCode::Add => "OP_ADD",
        OpCode::Subtract => "OP_SUBTRACT",
        OpCode::Multiply => "OP_MULTIPLY",
        OpCode::Divide => "OP_DIVIDE",
        OpCode::Not => "OP_NOT",
        OpCode::Negate => "OP_NEGATE",
        OpCode::Print => "OP_PRINT",
        OpCode::Jump => "OP_JUMP",
        OpCode::JumpIfFalse => "OP_JUMP_IF_FALSE",
        OpCode::Loop => "OP_LOOP",
        OpCode::Call => "OP_CALL",
        OpCode::Invoke => "OP_INVOKE",
        OpCode::SuperInvoke => "OP_SUPER_INVOKE",
        OpCode::Closure => "OP_CLOSURE",
        OpCode::CloseUpvalue => "OP_CLOSE_UPVALUE",
        OpCode::Return => "OP_RETURN",
        OpCode::Class => "OP_CLASS",
        OpCode::Inherit => "OP_INHERIT",
        OpCode::Method => "OP_METHOD",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;

    #[test]
    fn debug_disassemble_constant_instruction() {
        let mut chunk = Chunk::new();
        let idx = chunk.add_constant(Value::Number(42.0));
        chunk.write_op(OpCode::Constant, 1);
        chunk.write_byte(idx as u8, 1);

        let (line, next) = disassemble_instruction(&chunk, 0);
        assert!(line.contains("OP_CONSTANT"), "line={line:?}");
        assert!(line.starts_with("0000"), "line={line:?}");
        assert!(line.contains("'42'"), "line={line:?}");
        assert_eq!(next, 2);
    }

    #[test]
    fn debug_disassemble_return_simple() {
        let mut chunk = Chunk::new();
        chunk.write_op(OpCode::Return, 1);
        let (line, next) = disassemble_instruction(&chunk, 0);
        assert!(line.contains("OP_RETURN"), "line={line:?}");
        assert_eq!(next, 1);
    }

    #[test]
    fn debug_disassemble_same_line_shows_pipe() {
        let mut chunk = Chunk::new();
        chunk.write_op(OpCode::Nil, 3);
        chunk.write_op(OpCode::Return, 3);

        let (first, next) = disassemble_instruction(&chunk, 0);
        assert!(first.contains("   3"), "first={first:?}");

        let (second, _) = disassemble_instruction(&chunk, next);
        // The line column for the second instruction is `   |`.
        assert!(second.contains("   |"), "second={second:?}");
        assert!(!second.contains("   3"), "second={second:?}");
    }

    #[test]
    fn debug_disassemble_different_line_shows_number() {
        let mut chunk = Chunk::new();
        chunk.write_op(OpCode::Nil, 1);
        chunk.write_op(OpCode::Return, 2);

        let (_, next) = disassemble_instruction(&chunk, 0);
        let (second, _) = disassemble_instruction(&chunk, next);
        assert!(second.contains("   2"), "second={second:?}");
        assert!(!second.contains("   |"), "second={second:?}");
    }

    #[test]
    fn debug_disassemble_jump_operand() {
        // OP_JUMP (3 bytes) at offset 0, OP_RETURN at offset 3 + 2 bytes back
        // for the encoded jump. Use offset 2 so post-instruction ip (3) + 2
        // = target offset 5.
        let mut chunk = Chunk::new();
        chunk.write_op(OpCode::Jump, 1);
        chunk.write_byte(0x00, 1);
        chunk.write_byte(0x02, 1);
        chunk.write_op(OpCode::Return, 1);
        chunk.write_op(OpCode::Return, 1);

        let (line, next) = disassemble_instruction(&chunk, 0);
        assert!(line.contains("OP_JUMP"), "line={line:?}");
        // Book format: `%-16s %4d -> %d` → `OP_JUMP             0 -> 5`.
        assert!(line.contains("   0 -> 5"), "line={line:?}");
        assert_eq!(next, 3);
    }

    #[test]
    fn debug_disassemble_chunk_header() {
        let mut chunk = Chunk::new();
        chunk.write_op(OpCode::Return, 1);
        let dump = disassemble_chunk(&chunk, "test");
        assert!(dump.starts_with("== test =="), "dump={dump:?}");
        assert!(dump.contains("OP_RETURN"), "dump={dump:?}");
    }

    #[test]
    fn debug_disassemble_unknown_opcode() {
        let mut chunk = Chunk::new();
        chunk.write_byte(250, 1);
        let (line, next) = disassemble_instruction(&chunk, 0);
        assert!(line.contains("Unknown opcode 250"), "line={line:?}");
        assert_eq!(next, 1);
    }

    #[test]
    fn debug_disassemble_loop_goes_backward() {
        // Put OP_LOOP at offset 3 so post-ip = 6; jump encoded as 6 brings
        // target to 0.
        let mut chunk = Chunk::new();
        chunk.write_op(OpCode::Nil, 1);
        chunk.write_op(OpCode::Pop, 1);
        chunk.write_op(OpCode::Print, 1);
        chunk.write_op(OpCode::Loop, 1);
        chunk.write_byte(0x00, 1);
        chunk.write_byte(0x06, 1);

        let (line, next) = disassemble_instruction(&chunk, 3);
        assert!(line.contains("OP_LOOP"), "line={line:?}");
        assert!(line.contains("   3 -> 0"), "line={line:?}");
        assert_eq!(next, 6);
    }
}
