use crate::ir::{IrModule, IrOp};
use crate::codegen::{Backend, SourceMapEntry};

pub struct IrEmitBackend;

impl IrEmitBackend {
    pub fn new() -> Self { IrEmitBackend }
}

impl Backend for IrEmitBackend {
    fn compile(&mut self, module: &IrModule) -> Vec<u8> {
        let mut text = String::new();
        text.push_str("; BedRock-IR\n; --target ir\n\n");

        for instr in &module.instructions {
            if instr.op == IrOp::Mk {
                text.push_str(&format!("{}\n", instr));
            } else {
                text.push_str(&format!("    {}\n", instr));
            }
        }

        text.into_bytes()
    }

    fn get_source_map(&self) -> Vec<SourceMapEntry> { Vec::new() }
}
