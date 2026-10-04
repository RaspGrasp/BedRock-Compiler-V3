//RISC-V backend
use crate::ir::{IrModule, IrOp, Operand};
use crate::codegen::{Backend, SourceMapEntry};
use std::collections::{HashMap, HashSet, VecDeque};

const BASE_ADDR: u32 = 0x80000000;
// Defaults below are only fallbacks for when the source doesn't declare
// STACK/DATA explicitly. They carry no assumption about any specific chip —
// any target (MIPS-style flat memory, ESP32-C3 split flash/SRAM, etc.)
// overrides them via `root BASE`, `root STACK`, `root DATA` in source.
const DEFAULT_STACK_OFFSET: u32 = 0x20000;
const DEFAULT_DATA_OFFSET:  u32 = 0x10000;

/// Minimum spill frame (bytes). Actual frame grows with register pressure.
const MIN_SPILL_FRAME: i32 = 0x200; // default; grows if register pressure is higher

const ZERO: u32 = 0;
const RA:   u32 = 1;
const SP:   u32 = 2;
const GP:   u32 = 3;
const T0:   u32 = 5;
const T1:   u32 = 6;
const T2:   u32 = 7;
const A0:   u32 = 10;
const CF_L: u32 = 28;
const CF_R: u32 = 29;
const SCR1: u32 = 30;
const SCR2: u32 = 31;




const PHYS_REGS: &[u32] = &[8,9,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27];


const OP_LOAD:   u32 = 0x03;
const OP_IMM:    u32 = 0x13;
const OP_STORE:  u32 = 0x23;
const OP_REG:    u32 = 0x33;
const OP_LUI:    u32 = 0x37;
const OP_BRANCH: u32 = 0x63;
const OP_JALR:   u32 = 0x67;
const OP_JAL:    u32 = 0x6F;
const OP_SYSTEM: u32 = 0x73;


fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    ((funct7 & 0x7F) << 25) | ((rs2 & 0x1F) << 20) | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12) | ((rd & 0x1F) << 7) | (opcode & 0x7F)
}
fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (((imm as u32) & 0xFFF) << 20) | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12) | ((rd & 0x1F) << 7) | (opcode & 0x7F)
}
fn s_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
    let imm = (imm as u32) & 0xFFF;
    let hi = (imm >> 5) & 0x7F;
    let lo = imm & 0x1F;
    (hi << 25) | ((rs2 & 0x1F) << 20) | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12) | (lo << 7) | (opcode & 0x7F)
}
fn b_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
    let imm = imm as u32;
    let b12   = (imm >> 12) & 0x1;
    let b11   = (imm >> 11) & 0x1;
    let b10_5 = (imm >> 5)  & 0x3F;
    let b4_1  = (imm >> 1)  & 0xF;
    (b12 << 31) | (b10_5 << 25) | ((rs2 & 0x1F) << 20) | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12) | (b4_1 << 8) | (b11 << 7) | (opcode & 0x7F)
}
fn u_type(imm20: u32, rd: u32, opcode: u32) -> u32 {
    ((imm20 & 0xFFFFF) << 12) | ((rd & 0x1F) << 7) | (opcode & 0x7F)
}
fn j_type(imm: i32, rd: u32, opcode: u32) -> u32 {
    let imm = imm as u32;
    let b20    = (imm >> 20) & 0x1;
    let b19_12 = (imm >> 12) & 0xFF;
    let b11    = (imm >> 11) & 0x1;
    let b10_1  = (imm >> 1)  & 0x3FF;
    (b20 << 31) | (b10_1 << 21) | (b11 << 20) | (b19_12 << 12) | ((rd & 0x1F) << 7) | (opcode & 0x7F)
}


fn nop() -> u32 { i_type(0, ZERO, 0x0, ZERO, OP_IMM) }
fn mv(rd: u32, rs: u32) -> u32 { i_type(0, rs, 0x0, rd, OP_IMM) }
fn addi(rd: u32, rs: u32, imm: i32) -> u32 { i_type(imm, rs, 0x0, rd, OP_IMM) }
fn lw(rd: u32, base: u32, off: i32) -> u32 { i_type(off, base, 0x2, rd, OP_LOAD) }
fn sw(rs2: u32, base: u32, off: i32) -> u32 { s_type(off, rs2, base, 0x2, OP_STORE) }
fn jr(rs: u32) -> u32 { i_type(0, rs, 0x0, ZERO, OP_JALR) }
fn lui(rd: u32, imm20: u32) -> u32 { u_type(imm20, rd, OP_LUI) }
fn xori(rd: u32, rs: u32, imm: i32) -> u32 { i_type(imm, rs, 0x4, rd, OP_IMM) }


struct RegAlloc {
    map:        HashMap<String, u32>,
    spilled:    HashMap<String, u32>,
    free:       VecDeque<u32>,
    spill_base: u32,
    next_spill: u32,
}

impl RegAlloc {
    fn new() -> Self {
        RegAlloc {
            map:        HashMap::new(),
            spilled:    HashMap::new(),
            free:       PHYS_REGS.iter().copied().collect(),
            spill_base: 0x100,
            next_spill: 0x100,
        }
    }

    fn alloc(&mut self, name: &str) -> AllocResult {
        if let Some(&r) = self.map.get(name) { return AllocResult::Reg(r); }
        if let Some(&off) = self.spilled.get(name) { return AllocResult::Spill(off); }
        if let Some(r) = self.free.pop_front() {
            self.map.insert(name.to_string(), r);
            AllocResult::Reg(r)
        } else {
            let off = self.next_spill;
            self.next_spill += 4;
            self.spilled.insert(name.to_string(), off);
            AllocResult::Spill(off)
        }
    }

    fn reset(&mut self) {
        self.map.clear();
        self.spilled.clear();
        self.free = PHYS_REGS.iter().copied().collect();
        self.next_spill = self.spill_base;
    }
}

#[derive(Debug)]
enum AllocResult {
    Reg(u32),
    Spill(u32),
}


enum PatchKind {
    Jal { rd: u32 },
    Branch { funct3: u32, rs1: u32, rs2: u32 },
    LuiAddi { rd: u32 },
}


pub struct RiscvBackend {
    base_addr:    u32,   // where CODE is located/executed from
    stack_addr:   u32,   // NEW: independent SP init value — never derived from base_addr
    data_addr:    u32,   // where globals/root vars are stored — independent of base_addr and stack_addr
    hosted:       bool,  // NEW: true when `root HOSTED = 1;` is present — the binary
                         // is a callable function for a host (e.g. C/ESP-IDF), not a
                         // freestanding program that owns the machine forever.
    code:         Vec<u32>,
    source_map:   Vec<SourceMapEntry>,
    current_line: usize,

    labels:        HashMap<String, usize>,
    label_patches: Vec<(usize, String, PatchKind)>,

    cf_left:  Option<u32>,
    cf_right: Option<u32>,

   alloc: RegAlloc,

    pending_frame: bool,
    frame_size:    i32,
    frame_active:  bool,
    arg_bytes:     u32,
    next_data:    u32,
    data_symbols: HashMap<String, u32>,
    root_vars:    HashSet<String>,
}

impl RiscvBackend {
    pub fn new() -> Self {
        RiscvBackend {
            base_addr:    BASE_ADDR,
            stack_addr:   BASE_ADDR.wrapping_add(DEFAULT_STACK_OFFSET),
            data_addr:    BASE_ADDR.wrapping_add(DEFAULT_DATA_OFFSET),
            hosted:       false,
            code:         Vec::new(),
            source_map:   Vec::new(),
            current_line: 0,
            labels:        HashMap::new(),
            label_patches: Vec::new(),
            cf_left:  None,
            cf_right: None,
           alloc: RegAlloc::new(),
            pending_frame: false,
            frame_size:    MIN_SPILL_FRAME,
            frame_active:  false,
            arg_bytes:     0,
            next_data:    BASE_ADDR.wrapping_add(DEFAULT_DATA_OFFSET),
            data_symbols: HashMap::new(),
            root_vars:    HashSet::new(),
        }
    }


    fn emit(&mut self, instr: u32) {
        let addr = self.base_addr + (self.code.len() as u32 * 4);
        self.source_map.push(SourceMapEntry {
            line:        self.current_line,
            address:     addr,
            instruction: instr,
            source:      String::new(),
        });
        self.code.push(instr);
    }


    fn compute_frame_size(&self) -> i32 {
        let used = self.alloc.next_spill.saturating_sub(self.alloc.spill_base);
        let need = (used as i32) + 0x20;
        if need > MIN_SPILL_FRAME { ((need + 15) / 16) * 16 } else { MIN_SPILL_FRAME }
    }

    fn ensure_spill_frame(&mut self) {
        if self.pending_frame {
            self.frame_size = self.compute_frame_size();
            self.emit(addi(SP, SP, -self.frame_size));
            self.pending_frame = false;
            self.frame_active = true;
        }
    }

    fn leave_spill_frame(&mut self) {
        if self.frame_active {
            self.emit(addi(SP, SP, self.frame_size));
            self.frame_active = false;
        }
    }

    fn patch(&mut self, idx: usize, instr: u32) {
        self.code[idx] = instr;
        self.source_map[idx].instruction = instr;
    }


    fn emit_li(&mut self, reg: u32, imm: u32) {
        let upper = imm.wrapping_add(0x800) >> 12;
        let lower = (imm as i32).wrapping_sub(((upper as i32) & 0xFFFFF) << 12);
        if upper == 0 {
            self.emit(addi(reg, ZERO, lower));
        } else {
            self.emit(lui(reg, upper));
            if lower != 0 { self.emit(addi(reg, reg, lower)); } else { self.emit(nop()); }
        }
    }

    fn get_jump_target(&self, index: usize) -> u32 {
        self.base_addr + (index as u32 * 4)
    }


        fn operand_to_reg(&mut self, op: &Operand, temp_reg: u32) -> u32 {
        match op {
            Operand::VReg(name) => {
              
                if name == "__a0" {
                    return A0;
                }
                if self.root_vars.contains(name) {
                    let addr = *self.data_symbols.get(name).unwrap_or(&0);
                    self.emit_li(temp_reg, addr);
                    self.emit(lw(temp_reg, temp_reg, 0));
                    return temp_reg;
                }
                match self.alloc.alloc(name) {
                    AllocResult::Reg(r) => r,
                    AllocResult::Spill(off) => {
                        self.emit(lw(temp_reg, SP, off as i32));
                        temp_reg
                    }
                }
            }
            Operand::Imm(v) => { self.emit_li(temp_reg, *v as u32); temp_reg }
            Operand::Label(name) => {
                let addr = if let Some(&d) = self.data_symbols.get(name) { d }
                    else if let Some(&idx) = self.labels.get(name) { self.get_jump_target(idx) }
                    else { 0 };
                self.emit_li(temp_reg, addr);
                temp_reg
            }
            _ => { self.emit_li(temp_reg, 0); temp_reg }
        }
    }

        fn dest_reg(&mut self, op: &Operand) -> u32 {
        match op {
            Operand::VReg(name) => {
                if name == "__a0" {
                    return A0;
                }
                if self.root_vars.contains(name) { return T0; }
                match self.alloc.alloc(name) {
                    AllocResult::Reg(r) => r,
                    AllocResult::Spill(_) => T0,
                }
            }
            _ => T0,
        }
    }

    fn writeback_if_spilled(&mut self, op: &Operand, result_reg: u32) {
        if let Operand::VReg(name) = op {
            if self.root_vars.contains(name) {
                let addr = *self.data_symbols.get(name).unwrap_or(&0);
                if T2 != result_reg {
                    self.emit_li(T2, addr);
                    self.emit(sw(result_reg, T2, 0));
                }
                return;
            }
            if let Some(&off) = self.alloc.spilled.get(name) {
                self.emit(sw(result_reg, SP, off as i32));
            }
        }
    }


    fn register_label(&mut self, name: &str) {
        self.labels.insert(name.to_string(), self.code.len());
    }

    fn resolve_patches(&mut self) {
        let patches = std::mem::take(&mut self.label_patches);
        for (site, label, kind) in patches {
            let Some(&target_idx) = self.labels.get(label.as_str()) else {
                eprintln!("[RISCV IR] Unresolved label: '{}'", label);
                continue;
            };
            let target_addr = self.get_jump_target(target_idx);
            let site_addr   = self.get_jump_target(site);
            let offset      = (target_addr as i64 - site_addr as i64) as i32;
            match kind {
                PatchKind::Jal { rd } => {
                    self.patch(site, j_type(offset, rd, OP_JAL));
                }
                PatchKind::Branch { funct3, rs1, rs2 } => {
                    self.patch(site, b_type(offset, rs2, rs1, funct3, OP_BRANCH));
                }
                PatchKind::LuiAddi { rd } => {
                    let upper = target_addr.wrapping_add(0x800) >> 12;
                    let lower = (target_addr as i32).wrapping_sub(((upper as i32) & 0xFFFFF) << 12);
                    self.patch(site,     lui(rd, upper));
                    self.patch(site + 1, addi(rd, rd, lower));
                }
            }
        }
    }

    fn emit_jal_patch(&mut self, rd: u32, label: &str) {
        let site = self.code.len();
        self.emit(0);
        self.label_patches.push((site, label.to_string(), PatchKind::Jal { rd }));
    }

    fn emit_branch_patch(&mut self, funct3: u32, rs1: u32, rs2: u32, label: &str) {
        let site = self.code.len();
        self.emit(0);
        self.label_patches.push((site, label.to_string(), PatchKind::Branch { funct3, rs1, rs2 }));
    }


    fn emit_module(&mut self, module: &IrModule) {
        let mut used_as_label: HashSet<String> = HashSet::new();
        for instr in &module.instructions {
            for op in &instr.operands {
                if let Operand::Label(name) = op { used_as_label.insert(name.clone()); }
            }
        }

        for instr in &module.instructions {
            if instr.op == IrOp::Rdf {
                if let Operand::VReg(name) = &instr.operands[0] {
                    if !self.data_symbols.contains_key(name) {
                        self.root_vars.insert(name.clone());
                        if used_as_label.contains(name) {
                            if let Some(Operand::Imm(val)) = instr.operands.get(1) {
                                self.data_symbols.insert(name.clone(), *val as u32);
                            }
                        } else {
                            let addr = self.next_data;
                            self.data_symbols.insert(name.clone(), addr);
                            self.next_data += 4;
                        }
                    }
                }
            }
        }

        for (i, instr) in module.instructions.iter().enumerate() {
              if matches!(instr.op, IrOp::Mk | IrOp::Mf) {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    self.labels.insert(name.clone(), i);
                }
            }
        }

        for instr in &module.instructions {
            self.current_line += 1;
            self.emit_instr(instr);
        }

        self.resolve_patches();
    }

      fn emit_instr(&mut self, instr: &crate::ir::IrInstr) {
    
        match &instr.op {
            IrOp::Mk | IrOp::Mf | IrOp::Pop | IrOp::Comment | IrOp::Rdf | IrOp::StrData => {}
            _ => self.ensure_spill_frame(),
        }

        match &instr.op {
              
           IrOp::Mk => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    self.register_label(name);
                    if name.starts_with("main_entry") {
                        self.leave_spill_frame();
                        self.alloc.reset();
                        self.pending_frame = true;
                        self.frame_active  = false;
                        self.arg_bytes = 0;
                    }
                }
            }

               IrOp::Mf => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    self.leave_spill_frame();
                    self.register_label(name);
                    self.alloc.reset();
                    self.pending_frame = true;
                    self.frame_active  = false;
                    self.arg_bytes = 0;
                }
            }

            IrOp::Halt => {
                if self.hosted {
                    // Hosted mode: give the host's sp/gp back exactly as
                    // they were, then do a REAL return — never trap the
                    // host in an infinite loop or leave its stack pointer
                    // clobbered by our own stack/RA regions.
                    let save_sp_addr = self.stack_addr.wrapping_add(0x1000);
                    let save_gp_addr = self.stack_addr.wrapping_add(0x1004);
                    self.emit_li(T0, save_sp_addr);
                    self.emit(lw(SP, T0, 0));
                    self.emit_li(T0, save_gp_addr);
                    self.emit(lw(GP, T0, 0));
                    self.emit(jr(RA));
                } else {
                    // Freestanding mode (unchanged): we own the machine
                    // forever, so an infinite loop is the correct "end".
                    self.emit(j_type(0, ZERO, OP_JAL));
                }
            }

            IrOp::Mov => {
                if instr.operands.len() < 2 { return; }
                if let Operand::VReg(name) = &instr.operands[0] {
                    if self.root_vars.contains(name) {
                        let addr = *self.data_symbols.get(name).unwrap_or(&0);
                        let src = self.operand_to_reg(&instr.operands[1], T1);
                        self.emit_li(T2, addr);
                        self.emit(sw(src, T2, 0));
                        return;
                    }
                }
                let src = self.operand_to_reg(&instr.operands[1], T1);
                let dst = self.dest_reg(&instr.operands[0]);
                self.emit(mv(dst, src));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Rdf => {
                if instr.operands.len() < 2 { return; }
                if let Operand::VReg(name) = &instr.operands[0] {
                    let addr = *self.data_symbols.get(name).unwrap_or(&0);
                    let src = self.operand_to_reg(&instr.operands[1], T1);
                    self.emit_li(T2, addr);
                    self.emit(sw(src, T2, 0));
                }
            }

            IrOp::Df => {
                if instr.operands.len() < 2 { return; }
                if let (Operand::Label(name), Operand::Imm(val)) = (&instr.operands[0], &instr.operands[1]) {
                    let addr = self.next_data;
                    self.data_symbols.insert(name.clone(), addr);
                    self.emit_li(T1, *val as u32);
                    self.emit_li(T2, addr);
                    self.emit(sw(T1, T2, 0));
                    self.next_data += 4;
                }
            }

            IrOp::Bri => {
                if instr.operands.len() < 2 { return; }
                let val  = self.operand_to_reg(&instr.operands[0], T1);
                let addr = self.operand_to_reg(&instr.operands[1], T2);
                self.emit(sw(val, addr, 0));
            }

            IrOp::Get => {
                if instr.operands.len() < 2 { return; }
                let dst  = self.dest_reg(&instr.operands[0]);
                let addr = self.operand_to_reg(&instr.operands[1], T2);
                self.emit(lw(dst, addr, 0));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Add | IrOp::Sub | IrOp::And | IrOp::Orr | IrOp::Xor => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], T1);
                let r   = self.operand_to_reg(&instr.operands[2], T2);
                let w = match &instr.op {
                    IrOp::Add => r_type(0x00, r, l, 0x0, dst, OP_REG),
                    IrOp::Sub => r_type(0x20, r, l, 0x0, dst, OP_REG),
                    IrOp::And => r_type(0x00, r, l, 0x7, dst, OP_REG),
                    IrOp::Orr => r_type(0x00, r, l, 0x6, dst, OP_REG),
                    IrOp::Xor => r_type(0x00, r, l, 0x4, dst, OP_REG),
                    _ => unreachable!(),
                };
                self.emit(w);
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Mul => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], T1);
                let r   = self.operand_to_reg(&instr.operands[2], T2);
                self.emit(r_type(0x01, r, l, 0x0, dst, OP_REG)); // MUL
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Div => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], T1);
                let r   = self.operand_to_reg(&instr.operands[2], T2);
                // DIVU: unsigned division (language defaults to u32)
                self.emit(r_type(0x01, r, l, 0x5, dst, OP_REG));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Rem => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], T1);
                let r   = self.operand_to_reg(&instr.operands[2], T2);
                // REMU: unsigned remainder
                self.emit(r_type(0x01, r, l, 0x7, dst, OP_REG));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Shl => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], T1);
                match &instr.operands[2] {
                    Operand::Imm(sa) => self.emit(i_type((*sa as i32) & 31, l, 0x1, dst, OP_IMM)), // SLLI
                    op2 => {
                        let r = self.operand_to_reg(op2, T2);
                        self.emit(r_type(0x00, r, l, 0x1, dst, OP_REG)); // SLL
                    }
                }
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Shr => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], T1);
                match &instr.operands[2] {
                    Operand::Imm(sa) => self.emit(i_type((*sa as i32) & 31, l, 0x5, dst, OP_IMM)), // SRLI+
                    op2 => {
                        let r = self.operand_to_reg(op2, T2);
                        self.emit(r_type(0x00, r, l, 0x5, dst, OP_REG)); // SRL+
                    }
                }
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Not => {
                if instr.operands.len() < 2 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let src = self.operand_to_reg(&instr.operands[1], T1);
                self.emit(xori(dst, src, -1)); // bitwise NOT
                self.writeback_if_spilled(&instr.operands[0], dst);
            }
            IrOp::Syscall | IrOp::StrData => {
                // OS syscalls are x86-hosted only for now
            }

            IrOp::Cf => {
                if instr.operands.len() < 2 { return; }
                let l = self.operand_to_reg(&instr.operands[0], T1);
                let r = self.operand_to_reg(&instr.operands[1], T2);
                self.emit(mv(CF_L, l));
                self.emit(mv(CF_R, r));
                self.cf_left  = Some(CF_L);
                self.cf_right = Some(CF_R);
            }

            IrOp::Jf => {
                if instr.operands.len() < 2 { return; }
                let cond = match &instr.operands[0] { Operand::Str(s) => s.clone(), _ => "==".to_string() };
                let label = match &instr.operands[1] { Operand::Label(l) => l.clone(), _ => return };
                let l = self.cf_left.unwrap_or(CF_L);
                let r = self.cf_right.unwrap_or(CF_R);
                match cond.as_str() {
                    // equality
                    "==" => self.emit_branch_patch(0x0, l, r, &label),
                    "!=" => self.emit_branch_patch(0x1, l, r, &label),
                    // signed compares: works with two's-complement unary minus
                    // (e.g. -5 < 0 is true). Use u32 addresses carefully.
                    "<"  => self.emit_branch_patch(0x4, l, r, &label), // BLT
                    ">=" => self.emit_branch_patch(0x5, l, r, &label), // BGE
                    ">"  => self.emit_branch_patch(0x4, r, l, &label),
                    "<=" => self.emit_branch_patch(0x5, r, l, &label),
                    _ => {
                        eprintln!("[RISCV IR] Unknown JF condition: '{}'", cond);
                        self.emit_branch_patch(0x1, l, r, &label);
                    }
                }
            }

            IrOp::Go => {
                if let Some(Operand::Label(label)) = instr.operands.first() {
                    self.emit_jal_patch(ZERO, label);
                }
            }

            IrOp::Psh => {
                if instr.operands.is_empty() { return; }
                let src = self.operand_to_reg(&instr.operands[0], T1);
                self.emit(addi(SP, SP, -4));
                self.emit(sw(src, SP, 0));
                self.arg_bytes = self.arg_bytes.saturating_add(4);
            }

            IrOp::Pop => {
                if instr.operands.is_empty() { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                self.emit(lw(dst, SP, 0));
                self.emit(addi(SP, SP, 4));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

           IrOp::Cal => {
                if let Some(Operand::Label(label)) = instr.operands.first() {
                    if label.is_empty() { return; }

                    let adj = self.arg_bytes as i32;

                    let mut saved: Vec<(u32, i32)> = Vec::new();
                    let mut used = HashSet::new();
                    let mut off: i32 = 0x40;
                    for &reg in self.alloc.map.values() {
                        if used.insert(reg) {
                            saved.push((reg, off));
                            off += 4;
                        }
                    }
                    for &(reg, o) in &saved {
                        self.emit(sw(reg, SP, o));
                    }

                    self.emit(addi(GP, GP, -4));
                    self.emit(sw(RA, GP, 0));
                    self.emit_jal_patch(RA, label);
                    self.emit(lw(RA, GP, 0));
                    self.emit(addi(GP, GP, 4));

                    for &(reg, o) in &saved {
                        self.emit(lw(reg, SP, o - adj));
                    }

                    self.arg_bytes = 0;
                }
            }

               IrOp::Ret => {
                if let Some(op) = instr.operands.first() {
                    let src = self.operand_to_reg(op, A0);
                    if src != A0 { self.emit(mv(A0, src)); }
                }
                self.leave_spill_frame();
                self.emit(jr(RA));
            }

            IrOp::Int => {
                if instr.operands.len() < 2 { return; }
                if let Operand::Label(handler) = &instr.operands[1] {
                    let site = self.code.len();
                    self.emit(0);
                    self.emit(0);
                    self.label_patches.push((site, handler.clone(), PatchKind::LuiAddi { rd: SCR1 }));
                }
            }

            IrOp::Inb | IrOp::Peek => {
                if instr.operands.len() < 2 { return; }
                let dst  = self.dest_reg(&instr.operands[0]);
                let addr = self.operand_to_reg(&instr.operands[1], T2);
                self.emit(lw(dst, addr, 0));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Outb | IrOp::Poke => {
                if instr.operands.len() < 2 { return; }
                let val  = self.operand_to_reg(&instr.operands[0], T1);
                let addr = self.operand_to_reg(&instr.operands[1], T2);
                self.emit(sw(val, addr, 0));
            }

            IrOp::Asm => {
                if let Some(Operand::Str(hex)) = instr.operands.first() {
                    if let Ok(word) = u32::from_str_radix(hex.trim(), 16) { self.emit(word); }
                }
            }

            IrOp::Const => {
                if instr.operands.len() < 2 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let val = self.operand_to_reg(&instr.operands[1], T0);
                self.emit(mv(dst, val));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Bnw => {}

            IrOp::IntDisable => {
                let csr_mstatus: i32 = 0x300;
                self.emit(i_type(csr_mstatus, 0x8, 0x7, ZERO, OP_SYSTEM));
            }

            IrOp::SaveCtx => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    let base = *self.data_symbols.get(name).unwrap_or(&self.next_data);
                    let regs: &[u32] = &[8,9,18,19,20,21,22,23,1,2];
                    for (i, &r) in regs.iter().enumerate() {
                        let addr = base + (i as u32 * 4);
                        self.emit_li(T2, addr);
                        self.emit(sw(r, T2, 0));
                    }
                }
            }

            IrOp::RestoreCtx => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    let base = *self.data_symbols.get(name).unwrap_or(&self.next_data);
                    let regs: &[u32] = &[8,9,18,19,20,21,22,23,1,2];
                    for (i, &r) in regs.iter().enumerate() {
                        let addr = base + (i as u32 * 4);
                        self.emit_li(T2, addr);
                        self.emit(lw(r, T2, 0));
                    }
                    self.emit(jr(RA));
                }
            }

            IrOp::Comment => {}
        }
    }
}

impl Backend for RiscvBackend {
    fn compile(&mut self, module: &IrModule) -> Vec<u8> {
        self.code.clear();
        self.source_map.clear();
        self.labels.clear();
        self.label_patches.clear();
        self.alloc.reset();
        self.pending_frame = false;
        self.frame_active  = false;
        self.cf_left  = None;
        self.cf_right = None;
        self.root_vars.clear();
        self.data_symbols.clear();
        self.hosted = false;

        // Reset to defaults; each is independently overridable below.
        self.stack_addr = self.base_addr.wrapping_add(DEFAULT_STACK_OFFSET);
        self.data_addr  = self.base_addr.wrapping_add(DEFAULT_DATA_OFFSET);

        // First pass: let the SOURCE decide every address explicitly.
        // No chip is assumed here — BASE/STACK/DATA are independent knobs.
        for instr in &module.instructions {
            if instr.op == IrOp::Rdf {
                if let (Some(Operand::VReg(name)), Some(Operand::Imm(val))) =
                    (instr.operands.get(0), instr.operands.get(1))
                {
                    match name.as_str() {
                        "BASE"   => self.base_addr  = *val as u32,
                        "STACK"  => self.stack_addr = *val as u32,  // now drives SP directly
                        "DATA"   => self.data_addr  = *val as u32,  // now independent of STACK
                        "HOSTED" => self.hosted = *val != 0,        // NEW: opt into hosted mode
                        _ => {}
                    }
                }
            }
        }
        self.next_data = self.data_addr;

        // GP now serves as a dedicated return-address stack, kept far
        // away from SP's region (arguments/spills) so the two can never
        // collide for any realistic program size.
        let ra_stack_addr = self.stack_addr.wrapping_add(0x2000);

        if self.hosted {
            // We are being called as a FUNCTION from an existing host
            // (e.g. C/ESP-IDF) that already has its own valid sp/gp and
            // expects us to give them back unharmed on return. Save the
            // host's sp/gp into fixed slots reserved well away from our
            // own stack/data/RA regions before we touch either register.
            let save_sp_addr = self.stack_addr.wrapping_add(0x1000);
            let save_gp_addr = self.stack_addr.wrapping_add(0x1004);
            self.emit_li(T0, save_sp_addr);
            self.emit(sw(SP, T0, 0));
            self.emit_li(T0, save_gp_addr);
            self.emit(sw(GP, T0, 0));
        }

        self.emit_li(GP, ra_stack_addr);
        self.emit(nop());
        self.emit_li(SP, self.stack_addr);

        self.emit_module(module);

        self.code.iter()
            .flat_map(|&w| w.to_le_bytes().to_vec())
            .collect()
    }

    fn get_source_map(&self) -> Vec<SourceMapEntry> {
        self.source_map.clone()
    }
}
