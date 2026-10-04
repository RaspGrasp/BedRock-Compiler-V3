//! x86-64 backend for BedRock
//! Emits a minimal Linux ELF64 executable (System V AMD64).
//! Goal: hosted foundation for eventual self-hosting.

use crate::ir::{IrModule, IrOp, Operand};
use crate::codegen::{Backend, SourceMapEntry};
use std::collections::{HashMap, HashSet, VecDeque};

// ── Physical registers (System V AMD64) ───────────────────
// We allocate from callee-saved + some caller-saved temps.
const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RBX: u8 = 3;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RSI: u8 = 6;
const RDI: u8 = 7;
const R8:  u8 = 8;
const R9:  u8 = 9;
const R10: u8 = 10;
const R11: u8 = 11;
const R12: u8 = 12;
const R13: u8 = 13;
const R14: u8 = 14;
const R15: u8 = 15;

/// Allocatable GPRs (avoid RSP/RBP; keep RAX/RDX as temps often)
const PHYS_REGS: &[u8] = &[
    RBX, R12, R13, R14, R15,
    R8, R9, R10, R11,
    RSI, RDI, RCX,
];

const CF_L: u8 = R14;
const CF_R: u8 = R15;

const DEFAULT_BASE: u64 = 0x400000;
const DEFAULT_STACK_SIZE: u64 = 0x10000;

enum AllocResult {
    Reg(u8),
    Spill(u32),
}

struct RegAlloc {
    map: HashMap<String, u8>,
    spilled: HashMap<String, u32>,
    free: VecDeque<u8>,
    spill_base: u32,
    next_spill: u32,
}

impl RegAlloc {
    fn new() -> Self {
        RegAlloc {
            map: HashMap::new(),
            spilled: HashMap::new(),
            free: PHYS_REGS.iter().copied().collect(),
            spill_base: 0x20,
            next_spill: 0x20,
        }
    }

    fn alloc(&mut self, name: &str) -> AllocResult {
        if let Some(&r) = self.map.get(name) {
            return AllocResult::Reg(r);
        }
        if let Some(&off) = self.spilled.get(name) {
            return AllocResult::Spill(off);
        }
        if let Some(r) = self.free.pop_front() {
            self.map.insert(name.to_string(), r);
            AllocResult::Reg(r)
        } else {
            let off = self.next_spill;
            self.next_spill += 8;
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

enum PatchKind {
    Rel32 { at: usize },          // patch 4-byte relative displacement at `at`
    Abs64 { at: usize },          // patch 8-byte absolute address at `at`
}

pub struct X86Backend {
    code: Vec<u8>,
    source_map: Vec<SourceMapEntry>,
    labels: HashMap<String, usize>,
    /// (patch_offset_in_code, label, kind)
    patches: Vec<(usize, String, PatchKind)>,
    alloc: RegAlloc,
    cf_left: Option<u8>,
    cf_right: Option<u8>,
    data_symbols: HashMap<String, u64>,
    root_vars: HashSet<String>,
    next_data: u64,
    base_addr: u64,
    stack_addr: u64,
    data_addr: u64,
    data_bytes: Vec<u8>,
    pending_frame: bool,
    frame_active: bool,
    frame_size: i32,
    param_index: u32,
    hosted: bool,
}

impl X86Backend {
    pub fn new() -> Self {
        X86Backend {
            code: Vec::new(),
            source_map: Vec::new(),
            labels: HashMap::new(),
            patches: Vec::new(),
            alloc: RegAlloc::new(),
            cf_left: None,
            cf_right: None,
            data_symbols: HashMap::new(),
            root_vars: HashSet::new(),
            next_data: 0,
            base_addr: DEFAULT_BASE,
            stack_addr: DEFAULT_BASE + 0x200000,
            data_addr: DEFAULT_BASE + 0x100000,
            data_bytes: Vec::new(),
            pending_frame: false,
            frame_active: false,
            frame_size: 0x200,
            param_index: 0,
            hosted: false,
        }
    }

    fn emit_bytes(&mut self, bytes: &[u8]) {
        self.code.extend_from_slice(bytes);
    }

    fn emit_u8(&mut self, b: u8) {
        self.code.push(b);
    }

    fn emit_u32(&mut self, v: u32) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn emit_u64(&mut self, v: u64) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn pos(&self) -> usize {
        self.code.len()
    }

    // ── Encoding helpers ──────────────────────────────────

    fn rex(&mut self, w: bool, r: u8, x: u8, b: u8) {
        let mut rex = 0x40;
        if w {
            rex |= 0x08;
        }
        if r >= 8 {
            rex |= 0x04;
        }
        if x >= 8 {
            rex |= 0x02;
        }
        if b >= 8 {
            rex |= 0x01;
        }
        // Always emit REX.W for 64-bit ops we care about
        if w || r >= 8 || x >= 8 || b >= 8 {
            self.emit_u8(rex);
        }
    }

    fn modrm(&mut self, mode: u8, reg: u8, rm: u8) {
        self.emit_u8((mode << 6) | ((reg & 7) << 3) | (rm & 7));
    }

    /// mov rax, imm64
    fn emit_mov_reg_imm64(&mut self, reg: u8, imm: u64) {
        self.rex(true, 0, 0, reg);
        self.emit_u8(0xB8 + (reg & 7));
        self.emit_u64(imm);
    }

    /// mov reg, reg
    fn emit_mov_rr(&mut self, dst: u8, src: u8) {
        if dst == src {
            return;
        }
        self.rex(true, src, 0, dst);
        self.emit_u8(0x89); // MOV r/m64, r64
        self.modrm(3, src, dst);
    }

    /// mov reg, [reg + disp32]
    fn emit_load64(&mut self, dst: u8, base: u8, disp: i32) {
        self.rex(true, dst, 0, base);
        self.emit_u8(0x8B);
        if disp == 0 && (base & 7) != RBP && (base & 7) != 5 {
            self.modrm(0, dst, base);
            if (base & 7) == 4 {
                self.emit_u8(0x24); // SIB for RSP
            }
        } else if disp >= -128 && disp <= 127 {
            self.modrm(1, dst, base);
            if (base & 7) == 4 {
                self.emit_u8(0x24);
            }
            self.emit_u8(disp as u8);
        } else {
            self.modrm(2, dst, base);
            if (base & 7) == 4 {
                self.emit_u8(0x24);
            }
            self.emit_u32(disp as u32);
        }
    }

    /// mov [reg + disp], reg
    fn emit_store64(&mut self, base: u8, disp: i32, src: u8) {
        self.rex(true, src, 0, base);
        self.emit_u8(0x89);
        if disp == 0 && (base & 7) != RBP && (base & 7) != 5 {
            self.modrm(0, src, base);
            if (base & 7) == 4 {
                self.emit_u8(0x24);
            }
        } else if disp >= -128 && disp <= 127 {
            self.modrm(1, src, base);
            if (base & 7) == 4 {
                self.emit_u8(0x24);
            }
            self.emit_u8(disp as u8);
        } else {
            self.modrm(2, src, base);
            if (base & 7) == 4 {
                self.emit_u8(0x24);
            }
            self.emit_u32(disp as u32);
        }
    }

    /// mov reg, [imm64] via rax temp path: mov r10, imm; mov reg, [r10]
    fn emit_load_abs(&mut self, dst: u8, addr: u64) {
        self.emit_mov_reg_imm64(R10, addr);
        self.emit_load64(dst, R10, 0);
    }

    fn emit_store_abs(&mut self, addr: u64, src: u8) {
        self.emit_mov_reg_imm64(R10, addr);
        // if src is R10, need temp
        if src == R10 {
            self.emit_mov_rr(R11, src);
            self.emit_store64(R10, 0, R11);
        } else {
            self.emit_store64(R10, 0, src);
        }
    }

    fn emit_add_rr(&mut self, dst: u8, src: u8) {
        self.rex(true, src, 0, dst);
        self.emit_u8(0x01);
        self.modrm(3, src, dst);
    }
    fn emit_sub_rr(&mut self, dst: u8, src: u8) {
        self.rex(true, src, 0, dst);
        self.emit_u8(0x29);
        self.modrm(3, src, dst);
    }
    fn emit_and_rr(&mut self, dst: u8, src: u8) {
        self.rex(true, src, 0, dst);
        self.emit_u8(0x21);
        self.modrm(3, src, dst);
    }
    fn emit_or_rr(&mut self, dst: u8, src: u8) {
        self.rex(true, src, 0, dst);
        self.emit_u8(0x09);
        self.modrm(3, src, dst);
    }
    fn emit_xor_rr(&mut self, dst: u8, src: u8) {
        self.rex(true, src, 0, dst);
        self.emit_u8(0x31);
        self.modrm(3, src, dst);
    }
    fn emit_cmp_rr(&mut self, left: u8, right: u8) {
        self.rex(true, right, 0, left);
        self.emit_u8(0x39);
        self.modrm(3, right, left);
    }

    /// imul dst, src  (2-operand)
    fn emit_imul_rr(&mut self, dst: u8, src: u8) {
        self.rex(true, dst, 0, src);
        self.emit_u8(0x0F);
        self.emit_u8(0xAF);
        self.modrm(3, dst, src);
    }

    /// xor edx,edx ; div r/m  → unsigned div rax/reg, quot in rax, rem in rdx
    fn emit_div_rr(&mut self, divisor: u8) {
        // clear rdx
        self.rex(true, RDX, 0, RDX);
        self.emit_u8(0x31);
        self.modrm(3, RDX, RDX);
        self.rex(true, 0, 0, divisor);
        self.emit_u8(0xF7);
        self.modrm(3, 6, divisor); // /6 = DIV
    }

    fn emit_shl_cl(&mut self, dst: u8) {
        self.rex(true, 0, 0, dst);
        self.emit_u8(0xD3);
        self.modrm(3, 4, dst); // /4 SHL
    }
    fn emit_shr_cl(&mut self, dst: u8) {
        self.rex(true, 0, 0, dst);
        self.emit_u8(0xD3);
        self.modrm(3, 5, dst); // /5 SHR
    }

    fn emit_push(&mut self, reg: u8) {
        if reg >= 8 {
            self.emit_u8(0x41);
        }
        self.emit_u8(0x50 + (reg & 7));
    }
    fn emit_pop(&mut self, reg: u8) {
        if reg >= 8 {
            self.emit_u8(0x41);
        }
        self.emit_u8(0x58 + (reg & 7));
    }

    fn emit_ret(&mut self) {
        self.emit_u8(0xC3);
    }

    fn emit_syscall(&mut self) {
        self.emit_u8(0x0F);
        self.emit_u8(0x05);
    }

    /// call rel32 (patched later) — returns patch site of disp
    fn emit_call_rel_placeholder(&mut self) -> usize {
        self.emit_u8(0xE8);
        let at = self.pos();
        self.emit_u32(0);
        at
    }

    /// jmp rel32 placeholder
    fn emit_jmp_rel_placeholder(&mut self) -> usize {
        self.emit_u8(0xE9);
        let at = self.pos();
        self.emit_u32(0);
        at
    }

    /// jcc rel32 — opcode 0F 8x
    fn emit_jcc_rel_placeholder(&mut self, cc: u8) -> usize {
        self.emit_u8(0x0F);
        self.emit_u8(0x80 | cc);
        let at = self.pos();
        self.emit_u32(0);
        at
    }

    fn emit_sub_rsp(&mut self, imm: i32) {
        self.rex(true, 0, 0, RSP);
        self.emit_u8(0x81);
        self.modrm(3, 5, RSP); // SUB
        self.emit_u32(imm as u32);
    }
    fn emit_add_rsp(&mut self, imm: i32) {
        self.rex(true, 0, 0, RSP);
        self.emit_u8(0x81);
        self.modrm(3, 0, RSP); // ADD
        self.emit_u32(imm as u32);
    }

    // ── Operand helpers ───────────────────────────────────

    fn operand_to_reg(&mut self, op: &Operand, temp: u8) -> u8 {
        match op {
            Operand::VReg(name) => {
                if name == "__a0" {
                    return RAX;
                }
                if self.root_vars.contains(name) {
                    let addr = *self.data_symbols.get(name).unwrap_or(&0);
                    self.emit_load_abs(temp, addr);
                    return temp;
                }
                match self.alloc.alloc(name) {
                    AllocResult::Reg(r) => r,
                    AllocResult::Spill(off) => {
                        self.emit_load64(temp, RBP, -(off as i32));
                        temp
                    }
                }
            }
            Operand::Imm(v) => {
                self.emit_mov_reg_imm64(temp, *v);
                temp
            }
            Operand::Label(name) => {
                let addr = if let Some(&d) = self.data_symbols.get(name) {
                    d
                } else if let Some(&off) = self.labels.get(name) {
                    self.base_addr + off as u64
                } else {
                    0
                };
                self.emit_mov_reg_imm64(temp, addr);
                temp
            }
            _ => {
                self.emit_mov_reg_imm64(temp, 0);
                temp
            }
        }
    }

    fn dest_reg(&mut self, op: &Operand) -> u8 {
        match op {
            Operand::VReg(name) => {
                if name == "__a0" {
                    return RAX;
                }
                if self.root_vars.contains(name) {
                    return R11;
                }
                match self.alloc.alloc(name) {
                    AllocResult::Reg(r) => r,
                    AllocResult::Spill(_) => R11,
                }
            }
            _ => R11,
        }
    }

    fn writeback(&mut self, op: &Operand, result: u8) {
        if let Operand::VReg(name) = op {
            if self.root_vars.contains(name) {
                let addr = *self.data_symbols.get(name).unwrap_or(&0);
                self.emit_store_abs(addr, result);
                return;
            }
            if let Some(&off) = self.alloc.spilled.get(name) {
                self.emit_store64(RBP, -(off as i32), result);
            }
        }
    }

    fn ensure_frame(&mut self) {
        if self.pending_frame {
            self.emit_push(RBP);
            self.emit_mov_rr(RBP, RSP);
            self.frame_size = 0x80; // small frame
            self.emit_sub_rsp(self.frame_size);
            self.pending_frame = false;
            self.frame_active = true;
        }
    }

    fn leave_frame(&mut self) {
        if self.frame_active {
            self.emit_mov_rr(RSP, RBP);
            self.emit_pop(RBP);
            self.frame_active = false;
        }
    }

    fn register_label(&mut self, name: &str) {
        self.labels.insert(name.to_string(), self.pos());
    }

    fn patch_rel32(&mut self, at: usize, target_off: usize) {
        // rel = target - (at + 4)
        let rel = target_off as i64 - (at as i64 + 4);
        let bytes = (rel as i32).to_le_bytes();
        self.code[at..at + 4].copy_from_slice(&bytes);
    }

    fn resolve_patches(&mut self) {
        let patches = std::mem::take(&mut self.patches);
        for (at, label, kind) in patches {
            match kind {
                PatchKind::Rel32 { .. } => {
                    if let Some(&toff) = self.labels.get(&label) {
                        self.patch_rel32(at, toff);
                    } else {
                        eprintln!("[X86] Unresolved label '{}'", label);
                    }
                }
                PatchKind::Abs64 { .. } => {
                    if let Some(&toff) = self.labels.get(&label) {
                        let abs = self.base_addr + toff as u64;
                        let bytes = abs.to_le_bytes();
                        self.code[at..at + 8].copy_from_slice(&bytes);
                    } else {
                        eprintln!("[X86] Unresolved abs label '{}'", label);
                    }
                }
            }
        }
    }

    fn emit_exit_syscall(&mut self, code_reg: u8) {
        // mov rdi, code; mov rax, 60; syscall
        self.emit_mov_rr(RDI, code_reg);
        self.emit_mov_reg_imm64(RAX, 60);
        self.emit_syscall();
    }

    fn emit_instr(&mut self, instr: &crate::ir::IrInstr) {
        match &instr.op {
            IrOp::Mk | IrOp::Mf | IrOp::Comment | IrOp::Rdf => {}
            _ => self.ensure_frame(),
        }

        match &instr.op {
            IrOp::Mk => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    self.register_label(name);
                    if name.starts_with("main_entry") {
                        self.leave_frame();
                        self.alloc.reset();
                        self.pending_frame = true;
                        self.frame_active = false;
                    }
                }
            }
            IrOp::Mf => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    self.leave_frame();
                    self.register_label(name);
                    self.alloc.reset();
                    self.pending_frame = true;
                    self.frame_active = false;
                    self.param_index = 0;
                }
            }
            IrOp::Mov => {
                if instr.operands.len() < 2 {
                    return;
                }
                let dst = self.dest_reg(&instr.operands[0]);
                let src = self.operand_to_reg(&instr.operands[1], RAX);
                self.emit_mov_rr(dst, src);
                self.writeback(&instr.operands[0], dst);
            }
            IrOp::Add | IrOp::Sub | IrOp::And | IrOp::Orr | IrOp::Xor => {
                if instr.operands.len() < 3 {
                    return;
                }
                let dst = self.dest_reg(&instr.operands[0]);
                let l = self.operand_to_reg(&instr.operands[1], RAX);
                let r = self.operand_to_reg(&instr.operands[2], RCX);
                self.emit_mov_rr(dst, l);
                match instr.op {
                    IrOp::Add => self.emit_add_rr(dst, r),
                    IrOp::Sub => self.emit_sub_rr(dst, r),
                    IrOp::And => self.emit_and_rr(dst, r),
                    IrOp::Orr => self.emit_or_rr(dst, r),
                    IrOp::Xor => self.emit_xor_rr(dst, r),
                    _ => {}
                }
                self.writeback(&instr.operands[0], dst);
            }
            IrOp::Mul => {
                if instr.operands.len() < 3 {
                    return;
                }
                let dst = self.dest_reg(&instr.operands[0]);
                let l = self.operand_to_reg(&instr.operands[1], RAX);
                let r = self.operand_to_reg(&instr.operands[2], RCX);
                self.emit_mov_rr(dst, l);
                self.emit_imul_rr(dst, r);
                self.writeback(&instr.operands[0], dst);
            }
            IrOp::Div | IrOp::Rem => {
                if instr.operands.len() < 3 {
                    return;
                }
                let dst = self.dest_reg(&instr.operands[0]);
                let l = self.operand_to_reg(&instr.operands[1], R11);
                let r = self.operand_to_reg(&instr.operands[2], RCX);
                self.emit_mov_rr(RAX, l);
                self.emit_div_rr(r);
                if matches!(instr.op, IrOp::Div) {
                    self.emit_mov_rr(dst, RAX);
                } else {
                    self.emit_mov_rr(dst, RDX);
                }
                self.writeback(&instr.operands[0], dst);
            }
            IrOp::Shl | IrOp::Shr => {
                if instr.operands.len() < 3 {
                    return;
                }
                let dst = self.dest_reg(&instr.operands[0]);
                let l = self.operand_to_reg(&instr.operands[1], RAX);
                let r = self.operand_to_reg(&instr.operands[2], RCX);
                self.emit_mov_rr(dst, l);
                self.emit_mov_rr(RCX, r);
                match instr.op {
                    IrOp::Shl => self.emit_shl_cl(dst),
                    IrOp::Shr => self.emit_shr_cl(dst),
                    _ => {}
                }
                self.writeback(&instr.operands[0], dst);
            }
            IrOp::Get | IrOp::Peek => {
                if instr.operands.len() < 2 {
                    return;
                }
                let dst = self.dest_reg(&instr.operands[0]);
                let addr = self.operand_to_reg(&instr.operands[1], R10);
                self.emit_load64(dst, addr, 0);
                self.writeback(&instr.operands[0], dst);
            }
            IrOp::Bri | IrOp::Poke => {
                if instr.operands.len() < 2 {
                    return;
                }
                // Bri(val, addr) or similar ordering — match RISC-V backend
                let val = self.operand_to_reg(&instr.operands[0], RAX);
                let addr = self.operand_to_reg(&instr.operands[1], R10);
                self.emit_store64(addr, 0, val);
            }
            IrOp::Cf => {
                if instr.operands.len() < 2 {
                    return;
                }
                let l = self.operand_to_reg(&instr.operands[0], RAX);
                let r = self.operand_to_reg(&instr.operands[1], RCX);
                self.emit_mov_rr(CF_L, l);
                self.emit_mov_rr(CF_R, r);
                self.cf_left = Some(CF_L);
                self.cf_right = Some(CF_R);
            }
            IrOp::Jf => {
                if instr.operands.len() < 2 {
                    return;
                }
                let cond = match &instr.operands[0] {
                    Operand::Str(s) => s.as_str(),
                    _ => "==",
                };
                let label = match &instr.operands[1] {
                    Operand::Label(l) => l.clone(),
                    _ => return,
                };
                let l = self.cf_left.unwrap_or(CF_L);
                let r = self.cf_right.unwrap_or(CF_R);
                self.emit_cmp_rr(l, r);
                // CC: 0=O 1=NO 2=B/C 3=AE 4=E 5=NE 6=BE 7=A 8=S 9=NS C=L D=GE E=LE F=G
                let cc = match cond {
                    "==" => 0x4, // JE — but Jf means jump if condition (invert for branch-false?)
                    // Looking at RISC-V: jf means jump if condition is TRUE (the inverted cond is stored for false path)
                    // In IR builder invert_cond is passed to jf for the "skip" path
                    // So jf "==" means jump if equal
                    "!=" => 0x5, // JNE
                    "<" => 0xC,  // JL signed
                    ">=" => 0xD, // JGE
                    ">" => 0xF,  // JG
                    "<=" => 0xE, // JLE
                    _ => 0x5,
                };
                let at = self.emit_jcc_rel_placeholder(cc);
                self.patches
                    .push((at, label, PatchKind::Rel32 { at }));
            }
            IrOp::Go => {
                if let Some(Operand::Label(label)) = instr.operands.first() {
                    let at = self.emit_jmp_rel_placeholder();
                    self.patches
                        .push((at, label.clone(), PatchKind::Rel32 { at }));
                }
            }
            IrOp::Cal => {
                if let Some(Operand::Label(label)) = instr.operands.first() {
                    self.ensure_frame();
                    // Save live GPRs to frame (not on stack — args already pushed)
                    let saves: Vec<u8> = self.alloc.map.values().copied().collect();
                    for (i, &r) in saves.iter().enumerate() {
                        let off = -((0x50 + i * 8) as i32);
                        self.emit_store64(RBP, off, r);
                    }
                    let at = self.emit_call_rel_placeholder();
                    self.patches
                        .push((at, label.clone(), PatchKind::Rel32 { at }));
                    // Preserve return value in RAX before restoring live regs
                    // Fixed slot [rbp-0x48] holds the call result
                    self.emit_store64(RBP, -0x48, RAX);
                    for (i, &r) in saves.iter().enumerate() {
                        let off = -((0x50 + i * 8) as i32);
                        self.emit_load64(r, RBP, off);
                    }
                    // Reload return into RAX so subsequent Mov %dst, __a0 works
                    self.emit_load64(RAX, RBP, -0x48);
                }
            }
            IrOp::Ret => {
                if let Some(val) = instr.operands.first() {
                    let r = self.operand_to_reg(val, RAX);
                    self.emit_mov_rr(RAX, r);
                }
                self.leave_frame();
                self.emit_ret();
            }
            IrOp::Psh => {
                if let Some(op) = instr.operands.first() {
                    // Use R11 scratch — never RAX (return / intermediate)
                    let r = self.operand_to_reg(op, R11);
                    self.emit_push(r);
                }
            }
            IrOp::Pop => {
                // System V stack args: after prologue, arg i is at [rbp+16+8*i]
                self.ensure_frame();
                if let Some(dest_op) = instr.operands.first() {
                    let dst = self.dest_reg(dest_op);
                    let off = 16 + (self.param_index as i32) * 8;
                    self.param_index += 1;
                    self.emit_load64(dst, RBP, off);
                    self.writeback(dest_op, dst);
                }
            }
            IrOp::Syscall => {
                // operands: dest, nr [,arg0..arg5]
                if instr.operands.is_empty() { return; }
                let dest_op = &instr.operands[0];
                let nr_op = instr.operands.get(1);
                let arg_ops: Vec<&Operand> = instr.operands.iter().skip(2).collect();
                let arg_regs = [RDI, RSI, RDX, R10, R8, R9];

                // Save live allocated regs clobbered by syscall (rax,rcx,rdx,rsi,rdi,r8-r11)
                let clobbered = [RAX, RCX, RDX, RSI, RDI, R8, R9, R10, R11];
                let saves: Vec<u8> = self
                    .alloc
                    .map
                    .values()
                    .copied()
                    .filter(|r| clobbered.contains(r))
                    .collect();
                for &r in &saves {
                    self.emit_push(r);
                }

                for (i, a) in arg_ops.iter().enumerate() {
                    if i >= 6 { break; }
                    let reg = arg_regs[i];
                    match a {
                        Operand::Imm(v) => self.emit_mov_reg_imm64(reg, *v),
                        _ => {
                            let r = self.operand_to_reg(a, R11);
                            if r != reg { self.emit_mov_rr(reg, r); }
                        }
                    }
                }
                if let Some(nr) = nr_op {
                    match nr {
                        Operand::Imm(v) => self.emit_mov_reg_imm64(RAX, *v),
                        _ => {
                            let r = self.operand_to_reg(nr, R11);
                            self.emit_mov_rr(RAX, r);
                        }
                    }
                }
                self.emit_syscall();
                // Keep result in R11 temporarily while restoring
                self.emit_mov_rr(R11, RAX);
                for &r in saves.iter().rev() {
                    self.emit_pop(r);
                }
                let dst = self.dest_reg(dest_op);
                self.emit_mov_rr(dst, R11);
                self.writeback(dest_op, dst);
            }

            IrOp::StrData => {
                // already materialized in compile() prepass
            }

            IrOp::Halt => {
                // Use RAX as exit status (return value of main if present)
                self.emit_exit_syscall(RAX);
            }
            IrOp::Rdf => {
                // root define: name, value
                if instr.operands.len() >= 2 {
                    if let Operand::VReg(name) = &instr.operands[0] {
                        let val = match &instr.operands[1] {
                            Operand::Imm(v) => *v,
                            _ => 0,
                        };
                        // special roots
                        match name.as_str() {
                            "BASE" => self.base_addr = val,
                            "STACK" => self.stack_addr = val,
                            "DATA" => {
                                self.data_addr = val;
                                self.next_data = val;
                            }
                            "HOSTED" => self.hosted = val != 0,
                            _ => {
                                if !self.data_symbols.contains_key(name) {
                                    let addr = self.data_addr + self.data_bytes.len() as u64;
                                    // pad data with 8 bytes zero; value applied at runtime via init
                                    self.data_symbols.insert(name.clone(), addr);
                                    self.root_vars.insert(name.clone());
                                    let mut bytes = val.to_le_bytes().to_vec();
                                    bytes.resize(8, 0);
                                    self.data_bytes.extend_from_slice(&bytes);
                                }
                            }
                        }
                    }
                }
            }
            IrOp::Df => {
                if instr.operands.len() >= 2 {
                    if let (Operand::Label(name), Operand::Imm(v)) =
                        (&instr.operands[0], &instr.operands[1])
                    {
                        let addr = self.data_addr + self.data_bytes.len() as u64;
                        self.data_symbols.insert(name.clone(), addr);
                        let mut bytes = (*v).to_le_bytes().to_vec();
                        bytes.resize(8, 0);
                        self.data_bytes.extend_from_slice(&bytes);
                    }
                }
            }
            IrOp::Not => {
                if instr.operands.len() < 2 {
                    return;
                }
                let dst = self.dest_reg(&instr.operands[0]);
                let src = self.operand_to_reg(&instr.operands[1], RAX);
                self.emit_mov_rr(dst, src);
                // not dst
                self.rex(true, 0, 0, dst);
                self.emit_u8(0xF7);
                self.modrm(3, 2, dst);
                self.writeback(&instr.operands[0], dst);
            }
            IrOp::Asm => {
                // ignore raw asm strings on x86 for now
            }
            _ => {
                // unsupported ops: nop
            }
        }
    }

    fn emit_startup(&mut self) {
        // _start: set up rsp if freestanding; for ELF Linux, rsp is already set by kernel
        // We still align and jump to main_entry via IR's go instruction
        if !self.hosted {
            // optional: mov rsp, stack_addr for freestanding
            // On Linux ELF, leave rsp as loader provided
        }
    }

    fn build_elf(&self, text: &[u8]) -> Vec<u8> {
        // ELF64 ET_EXEC with two PT_LOAD: RX text, R data
        let ehdr_size = 64usize;
        let phdr_size = 56usize;
        let phnum = if self.data_bytes.is_empty() { 1usize } else { 2usize };
        let headers = ehdr_size + phdr_size * phnum;
        let text_off = ((headers + 15) / 16) * 16;
        let text_size = text.len();
        let data_off = ((text_off + text_size + 0xFFF) / 0x1000) * 0x1000; // page-align for p_vaddr congruence
        let data_size = self.data_bytes.len();
        let file_size = data_off + data_size;

        let text_vaddr = self.base_addr + text_off as u64;
        let data_vaddr = self.data_addr; // assigned in compile prepass as base+0x20000

        let mut out = vec![0u8; file_size.max(headers)];

        out[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        out[4] = 2; out[5] = 1; out[6] = 1; out[7] = 0;
        out[16] = 2; out[17] = 0; // ET_EXEC
        out[18] = 0x3e; out[19] = 0; // EM_X86_64
        out[20..24].copy_from_slice(&1u32.to_le_bytes());
        let entry = text_vaddr; // may be patched to main_entry
        out[24..32].copy_from_slice(&entry.to_le_bytes());
        out[32..40].copy_from_slice(&(ehdr_size as u64).to_le_bytes());
        out[52..54].copy_from_slice(&(ehdr_size as u16).to_le_bytes());
        out[54..56].copy_from_slice(&(phdr_size as u16).to_le_bytes());
        out[56..58].copy_from_slice(&(phnum as u16).to_le_bytes());

        // PHDR0: text RX — from file offset 0 covering headers+text for simplicity
        let ph0 = ehdr_size;
        out[ph0..ph0+4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        out[ph0+4..ph0+8].copy_from_slice(&5u32.to_le_bytes()); // R|X
        out[ph0+16..ph0+24].copy_from_slice(&self.base_addr.to_le_bytes());
        out[ph0+24..ph0+32].copy_from_slice(&self.base_addr.to_le_bytes());
        let text_filesz = (text_off + text_size) as u64;
        out[ph0+32..ph0+40].copy_from_slice(&text_filesz.to_le_bytes());
        out[ph0+40..ph0+48].copy_from_slice(&text_filesz.to_le_bytes());
        out[ph0+48..ph0+56].copy_from_slice(&0x1000u64.to_le_bytes());

        // PHDR1: data R
        let ph1 = ehdr_size + phdr_size;
        out[ph1..ph1+4].copy_from_slice(&1u32.to_le_bytes());
        out[ph1+4..ph1+8].copy_from_slice(&6u32.to_le_bytes()); // R|W — allows poke/runtime buffers
        out[ph1+8..ph1+16].copy_from_slice(&(data_off as u64).to_le_bytes()); // p_offset
        out[ph1+16..ph1+24].copy_from_slice(&data_vaddr.to_le_bytes());
        out[ph1+24..ph1+32].copy_from_slice(&data_vaddr.to_le_bytes());
        out[ph1+32..ph1+40].copy_from_slice(&(data_size as u64).to_le_bytes());
        out[ph1+40..ph1+48].copy_from_slice(&(data_size as u64).to_le_bytes());
        out[ph1+48..ph1+56].copy_from_slice(&0x1000u64.to_le_bytes());

        if text_off + text_size > out.len() {
            out.resize(text_off + text_size, 0);
        }
        out[text_off..text_off+text_size].copy_from_slice(text);
        if data_size > 0 {
            if data_off + data_size > out.len() {
                out.resize(data_off + data_size, 0);
            }
            out[data_off..data_off+data_size].copy_from_slice(&self.data_bytes);
        }
        out
    }
}

impl Backend for X86Backend {
    fn compile(&mut self, module: &IrModule) -> Vec<u8> {
        self.code.clear();
        self.labels.clear();
        self.patches.clear();
        self.alloc.reset();
        self.data_bytes.clear();
        self.data_symbols.clear();
        self.root_vars.clear();
        self.pending_frame = true;
        self.frame_active = false;

        // Pass 0: roots
        for instr in &module.instructions {
            if instr.op == IrOp::Rdf && instr.operands.len() >= 2 {
                if let Operand::VReg(name) = &instr.operands[0] {
                    let val = match &instr.operands[1] {
                        Operand::Imm(v) => *v,
                        _ => 0,
                    };
                    match name.as_str() {
                        "BASE" => self.base_addr = val,
                        "STACK" => self.stack_addr = val,
                        "DATA" => { self.data_addr = val; self.next_data = val; }
                        "HOSTED" => self.hosted = val != 0,
                        _ => {}
                    }
                }
            }
        }

        // Fixed data region for absolute addresses during codegen
        // (must match ELF layout: after ~64KB text budget from base)
        if self.data_addr == DEFAULT_BASE + 0x100000 || self.data_addr == 0 {
            self.data_addr = self.base_addr + 0x20000;
        }

        // Pass 1: materialize string/data symbols so AddressOf works during emit
        for instr in &module.instructions {
            match &instr.op {
                IrOp::StrData => {
                    if instr.operands.len() >= 2 {
                        if let (Operand::Label(name), Operand::Str(s)) =
                            (&instr.operands[0], &instr.operands[1])
                        {
                            let addr = self.data_addr + self.data_bytes.len() as u64;
                            self.data_symbols.insert(name.clone(), addr);
                            self.data_bytes.extend(s.bytes());
                            self.data_bytes.push(0);
                        }
                    }
                }
                IrOp::Df => {
                    if instr.operands.len() >= 2 {
                        if let (Operand::Label(name), Operand::Imm(v)) =
                            (&instr.operands[0], &instr.operands[1])
                        {
                            let addr = self.data_addr + self.data_bytes.len() as u64;
                            self.data_symbols.insert(name.clone(), addr);
                            let mut b = v.to_le_bytes().to_vec();
                            b.resize(8, 0);
                            self.data_bytes.extend_from_slice(&b);
                        }
                    }
                }
                _ => {}
            }
        }

        // Emit code — label offsets are relative to code buffer start
        for instr in &module.instructions {
            self.emit_instr(instr);
        }
        self.resolve_patches();

        // Build ELF; code offsets are from start of text segment
        // Rebuild with correct base: labels stored as offsets in code[]
        // ELF places text at text_off; rel32 patches already relative — OK
        // For entry: first byte of code

        let text = self.code.clone();
        let mut elf = self.build_elf(&text);

        // CRITICAL: rel32 are correct within text; but we need e_entry =
        // base + text_off, and any absolute data addresses independent.

        // If main_entry exists, entry should point there
        // e_entry already set correctly inside build_elf to start of text


        eprintln!(
            "[X86] ELF64 executable: {} code bytes, {} labels",
            text.len(),
            self.labels.len()
        );
        elf
    }

    fn get_source_map(&self) -> Vec<SourceMapEntry> {
        self.source_map.clone()
    }
}
