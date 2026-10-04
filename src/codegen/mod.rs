pub mod mips;
pub mod arm;
pub mod riscv;
pub mod x86;
pub mod ir_emit;

use crate::ir::IrModule;
#[derive(Debug, Clone)]
pub struct SourceMapEntry {
    pub line:        usize,
    pub address:     u32,
    pub instruction: u32,
    pub source:      String,
}


pub trait Backend {

    fn compile(&mut self, module: &IrModule) -> Vec<u8>;

    fn get_source_map(&self) -> Vec<SourceMapEntry> {
        Vec::new()
    }
}


#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Mips,
    MipsLe,  
    Arm,
    Riscv,
    X86,
    Ir,
}

impl Target {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "mips" | "mips-be"          => Target::Mips,
            "mips-le"                   => Target::MipsLe,  
            "arm"                       => Target::Arm,
            "riscv" | "risc-v" | "rv32" => Target::Riscv,
            "x86" | "x86_64" | "x64" | "amd64" => Target::X86,
            "ir"                        => Target::Ir,
            other => {
                eprintln!("[TARGET ERROR] Unknown target '{}'\n  Available: mips, mips-le, arm, riscv, x86, ir\n  Defaulting to: mips", other);
                Target::Mips
            }
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Target::Mips   => "mips-be",
            Target::MipsLe => "mips-le", 
            Target::Arm    => "arm",
            Target::Riscv  => "riscv32",
            Target::X86    => "x86_64",
            Target::Ir     => "ir",
        }
    }

    pub fn output_extension(&self) -> &str {
        match self {
            Target::Mips | Target::MipsLe => "bin",
            Target::Arm                   => "bin",
            Target::Riscv                 => "bin",
            Target::X86                   => "elf",
            Target::Ir                    => "ir",
        }
    }
}

pub fn select_backend(target: &Target) -> Box<dyn Backend> {
    match target {
        Target::Mips   => Box::new(mips::MipsBackend::new()),
        Target::MipsLe => Box::new(mips::MipsBackend::new_le()),
        Target::Arm    => Box::new(arm::ArmBackend::new()),
        Target::Riscv  => Box::new(riscv::RiscvBackend::new()),
        Target::X86    => Box::new(x86::X86Backend::new()),
        Target::Ir     => Box::new(ir_emit::IrEmitBackend::new()),
    }
}
