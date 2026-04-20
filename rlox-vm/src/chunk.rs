//! Bytecode chunks — the unit of code the VM executes. A `Chunk` holds the
//! raw instruction stream, a parallel `lines` array for error reporting, and
//! a pool of literal constants referenced by `OP_CONSTANT` et al.

use crate::value::Value;

/// VM opcode set. Ordered to roughly follow the book (ch. 14 → 28); each
/// variant's fixed operand layout is documented inline.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpCode {
    /// Push `constants[operand]`. Operand: 1-byte index.
    Constant,
    /// Push `nil`.
    Nil,
    /// Push `true`.
    True,
    /// Push `false`.
    False,
    /// Pop and discard TOS.
    Pop,
    /// Read local slot. Operand: 1-byte slot index.
    GetLocal,
    /// Write local slot. Operand: 1-byte slot index.
    SetLocal,
    /// Read global by interned-string name. Operand: 1-byte constant index.
    GetGlobal,
    /// Define new global. Operand: 1-byte constant index (name).
    DefineGlobal,
    /// Assign existing global. Operand: 1-byte constant index (name).
    SetGlobal,
    /// Read upvalue. Operand: 1-byte upvalue slot.
    GetUpvalue,
    /// Write upvalue. Operand: 1-byte upvalue slot.
    SetUpvalue,
    /// Read instance field. Operand: 1-byte constant index (name).
    GetProperty,
    /// Write instance field. Operand: 1-byte constant index (name).
    SetProperty,
    /// Read super-class method. Operand: 1-byte constant index (name).
    GetSuper,
    /// `==`.
    Equal,
    /// `>`.
    Greater,
    /// `<`.
    Less,
    /// `+` (numbers and string concat in M6).
    Add,
    /// `-`.
    Subtract,
    /// `*`.
    Multiply,
    /// `/`.
    Divide,
    /// Boolean `!`.
    Not,
    /// Unary `-`.
    Negate,
    /// `print` statement.
    Print,
    /// Unconditional forward jump. Operand: 2-byte big-endian offset.
    Jump,
    /// Forward jump if TOS is falsey. Operand: 2-byte big-endian offset.
    JumpIfFalse,
    /// Backward jump (loop header). Operand: 2-byte big-endian offset
    /// subtracted from the post-operand `ip`.
    Loop,
    /// Function call. Operand: 1-byte argument count.
    Call,
    /// Method invocation shortcut. Operands: 1-byte name const + 1-byte argc.
    Invoke,
    /// `super.method(...)` shortcut. Operands: 1-byte name const + 1-byte argc.
    SuperInvoke,
    /// Wrap a function constant into a closure capturing upvalues.
    /// Operand: 1-byte fn const index, followed by per-upvalue pairs of
    /// `(is_local: u8, index: u8)`. The per-upvalue count is taken from the
    /// function constant itself.
    Closure,
    /// Hoist the top-of-stack local into a heap upvalue.
    CloseUpvalue,
    /// Return from the current function.
    Return,
    /// Declare a class. Operand: 1-byte constant index (name).
    Class,
    /// Link the class's superclass (TOS-1) into the child (TOS).
    Inherit,
    /// Attach a method to the enclosing class. Operand: 1-byte constant index.
    Method,
}

impl OpCode {
    /// Convert a raw byte back into an `OpCode`. Returns `None` for values
    /// outside the defined range.
    ///
    /// Explicit match (no `unsafe` transmute) — if the byte stream is corrupt
    /// we want a recoverable `None`, not UB.
    pub fn from_byte(byte: u8) -> Option<OpCode> {
        let op = match byte {
            0 => OpCode::Constant,
            1 => OpCode::Nil,
            2 => OpCode::True,
            3 => OpCode::False,
            4 => OpCode::Pop,
            5 => OpCode::GetLocal,
            6 => OpCode::SetLocal,
            7 => OpCode::GetGlobal,
            8 => OpCode::DefineGlobal,
            9 => OpCode::SetGlobal,
            10 => OpCode::GetUpvalue,
            11 => OpCode::SetUpvalue,
            12 => OpCode::GetProperty,
            13 => OpCode::SetProperty,
            14 => OpCode::GetSuper,
            15 => OpCode::Equal,
            16 => OpCode::Greater,
            17 => OpCode::Less,
            18 => OpCode::Add,
            19 => OpCode::Subtract,
            20 => OpCode::Multiply,
            21 => OpCode::Divide,
            22 => OpCode::Not,
            23 => OpCode::Negate,
            24 => OpCode::Print,
            25 => OpCode::Jump,
            26 => OpCode::JumpIfFalse,
            27 => OpCode::Loop,
            28 => OpCode::Call,
            29 => OpCode::Invoke,
            30 => OpCode::SuperInvoke,
            31 => OpCode::Closure,
            32 => OpCode::CloseUpvalue,
            33 => OpCode::Return,
            34 => OpCode::Class,
            35 => OpCode::Inherit,
            36 => OpCode::Method,
            _ => return None,
        };
        Some(op)
    }

    /// Raw byte for the opcode, matching the `#[repr(u8)]` discriminant.
    pub fn as_byte(self) -> u8 {
        self as u8
    }
}

/// A compiled bytecode chunk: instructions, per-byte line numbers, constants.
#[derive(Debug, Default)]
pub struct Chunk {
    /// Flat instruction stream (opcodes and inline operands mixed).
    pub code: Vec<u8>,
    /// Source line for each byte in `code`. Mirrors book ch. 14; a real
    /// implementation would RLE-compress this, but parallel arrays suffice
    /// for M4 correctness.
    pub lines: Vec<usize>,
    /// Literal pool referenced by `OP_CONSTANT` and friends.
    pub constants: Vec<Value>,
}

impl Chunk {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a raw byte with its originating source line.
    pub fn write_byte(&mut self, byte: u8, line: usize) {
        self.code.push(byte);
        self.lines.push(line);
    }

    /// Convenience: append an opcode.
    pub fn write_op(&mut self, op: OpCode, line: usize) {
        self.write_byte(op.as_byte(), line);
    }

    /// Push `value` onto the constants pool and return its index.
    pub fn add_constant(&mut self, value: Value) -> usize {
        self.constants.push(value);
        self.constants.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_write_byte_records_line() {
        let mut chunk = Chunk::new();
        chunk.write_byte(0xAB, 7);
        chunk.write_byte(0xCD, 8);
        assert_eq!(chunk.code, vec![0xAB, 0xCD]);
        assert_eq!(chunk.lines, vec![7, 8]);
    }

    #[test]
    fn chunk_write_op_round_trip_via_from_byte() {
        let mut chunk = Chunk::new();
        chunk.write_op(OpCode::Return, 1);
        chunk.write_op(OpCode::Add, 2);
        chunk.write_op(OpCode::Class, 3);

        assert_eq!(chunk.code.len(), 3);
        assert_eq!(OpCode::from_byte(chunk.code[0]), Some(OpCode::Return));
        assert_eq!(OpCode::from_byte(chunk.code[1]), Some(OpCode::Add));
        assert_eq!(OpCode::from_byte(chunk.code[2]), Some(OpCode::Class));
    }

    #[test]
    fn chunk_add_constant_returns_increasing_index() {
        let mut chunk = Chunk::new();
        let i0 = chunk.add_constant(Value::Number(1.0));
        let i1 = chunk.add_constant(Value::Number(2.0));
        let i2 = chunk.add_constant(Value::Nil);
        assert_eq!(i0, 0);
        assert_eq!(i1, 1);
        assert_eq!(i2, 2);
        assert_eq!(chunk.constants.len(), 3);
    }

    #[test]
    fn chunk_opcode_from_byte_unknown_returns_none() {
        assert!(OpCode::from_byte(255).is_none());
        assert!(OpCode::from_byte(200).is_none());
        // Sanity: the last known opcode still round-trips.
        assert_eq!(
            OpCode::from_byte(OpCode::Method.as_byte()),
            Some(OpCode::Method)
        );
    }

    #[test]
    fn chunk_opcode_as_byte_matches_discriminant() {
        // Spot-check several to ensure we haven't desynced `from_byte`
        // against the implicit `#[repr(u8)]` discriminant.
        assert_eq!(OpCode::Constant.as_byte(), 0);
        assert_eq!(OpCode::Nil.as_byte(), 1);
        assert_eq!(OpCode::Return.as_byte(), 33);
        assert_eq!(OpCode::Method.as_byte(), 36);
        for raw in 0u8..=36 {
            let op = OpCode::from_byte(raw).expect("known opcode");
            assert_eq!(op.as_byte(), raw);
        }
    }
}
