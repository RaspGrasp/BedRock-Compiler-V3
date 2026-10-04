use crate::ast::{Statement, Expression, TypeKind};
use crate::ir::{IrModule, IrOp, Operand};
use crate::codegen::{Backend, SourceMapEntry};
use std::collections::{HashMap, HashSet, VecDeque};

const BASE_ADDR: u32 = 0x80000000;


const PHYS_REGS: &[u32] = &[10,11,12,13,14,15,16,17,18,19,20,21,24,25];

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
            self.next_spill += 4;
            self.spilled.insert(name.to_string(), off);
            AllocResult::Spill(off)
        }
    }

    fn get(&self, name: &str) -> Option<u32> {
        self.map.get(name).copied()
    }

    fn free_reg(&mut self, name: &str) {
        if let Some(r) = self.map.remove(name) {
            if !self.free.contains(&r) {
                self.free.push_back(r);
            }
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


pub struct MipsBackend {
    base_addr:   u32,
    little_endian: bool,
    code:        Vec<u32>,
    source_map:  Vec<SourceMapEntry>,
    current_line: usize,

    // Symbol tables
    labels:      HashMap<String, usize>,  
    label_patches: Vec<(usize, String)>,  

   
    cf_left:     Option<u32>,   
    cf_right:    Option<u32>,  

    
    alloc:       RegAlloc,

   
    temp_pool:   VecDeque<u32>,

    
next_data:   u32,
    data_symbols: HashMap<String, u32>,
    root_vars:   std::collections::HashSet<String>,
}

impl MipsBackend {
    pub fn new() -> Self {
        MipsBackend {
            base_addr:    BASE_ADDR,
            code:         Vec::new(),
            source_map:   Vec::new(),
            current_line: 0,
            labels:       HashMap::new(),
            label_patches: Vec::new(),
            cf_left:      None,
            cf_right:     None,
            alloc:        RegAlloc::new(),
            temp_pool:    VecDeque::from(vec![1u32, 3, 26, 27]),
            next_data:    BASE_ADDR + 0x10000,
            data_symbols: HashMap::new(),
            root_vars:    std::collections::HashSet::new(),
            little_endian: false,
        }
    }                      

    pub fn new_le() -> Self {
        let mut s = Self::new();
        s.little_endian = true;
        s
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

    fn patch(&mut self, idx: usize, instr: u32) {
        self.code[idx]                = instr;
        self.source_map[idx].instruction = instr;
    }

    fn emit_li(&mut self, reg: u32, imm: u32) {
        let hi = (imm >> 16) & 0xFFFF;
        let lo = imm & 0xFFFF;
        if hi == 0 {
            self.emit(0x34000000 | (reg << 16) | lo);        
        } else {
            self.emit(0x3C000000 | (reg << 16) | hi);        
            if lo != 0 {
                self.emit(0x34000000 | (reg << 21) | (reg << 16) | lo); 
            }
        }
    }

    fn get_jump_target(&self, index: usize) -> u32 {
        let addr = self.base_addr + (index as u32 * 4);
        (addr >> 2) & 0x03FFFFFF
    }


    fn operand_to_reg(&mut self, op: &Operand, temp_reg: u32) -> u32 {
        match op {
            Operand::VReg(name) => {
                if self.root_vars.contains(name) {
                    let addr = *self.data_symbols.get(name).unwrap_or(&0);
                    self.emit_li(temp_reg, addr);
                    self.emit(0x8C000000 | (temp_reg << 21) | (temp_reg << 16));
                    return temp_reg;
                }
                match self.alloc.alloc(name) {
                    AllocResult::Reg(r) => r,
                    AllocResult::Spill(off) => {
                        self.emit(0x8FA00000 | (temp_reg << 16) | (off & 0xFFFF));
                        temp_reg
                    }
                }
            }
            Operand::Imm(v) => {
                self.emit_li(temp_reg, *v as u32);
                temp_reg
            }
            Operand::Label(name) => {
                let addr = if let Some(&data_addr) = self.data_symbols.get(name) {
                    data_addr
                } else if let Some(&code_idx) = self.labels.get(name) {
                    self.base_addr + (code_idx as u32 * 4)
                } else {
                    0
                };
                self.emit_li(temp_reg, addr);
                temp_reg
            }
            _ => {
                self.emit_li(temp_reg, 0);
                temp_reg
            }
        }
    }

  fn dest_reg(&mut self, op: &Operand) -> u32 {
        match op {
            Operand::VReg(name) => {
                if self.root_vars.contains(name) {
                    return 1;
                }
                match self.alloc.alloc(name) {
                    AllocResult::Reg(r) => r,
                    AllocResult::Spill(_) => 1,
                }
            }
            _ => 1,
        }
    }

   fn writeback_if_spilled(&mut self, op: &Operand, result_reg: u32) {
        if let Operand::VReg(name) = op {
            if self.root_vars.contains(name) {
                let addr = *self.data_symbols.get(name).unwrap_or(&0);
                let addr_reg = 9u32;
                if addr_reg != result_reg {
                    self.emit_li(addr_reg, addr);
                    self.emit(0xAC000000 | (addr_reg << 21) | (result_reg << 16));
                }
                return;
            }
            if let Some(&off) = self.alloc.spilled.get(name) {
                self.emit(0xAFA00000 | (result_reg << 16) | (off & 0xFFFF));
            }
        }
    }

  
    fn register_label(&mut self, name: &str) {
        self.labels.insert(name.to_string(), self.code.len());
    }

    fn patch_label(&mut self, site: usize, label: &str, is_branch: bool) {
        if let Some(&target_idx) = self.labels.get(label) {
            if is_branch {
           
                let offset = (target_idx as i32 - site as i32 - 1) as i16;
                self.code[site] |= (offset as u16) as u32;
                self.source_map[site].instruction = self.code[site];
            } else {
                
                let t = self.get_jump_target(target_idx);
                self.code[site] |= t;
                self.source_map[site].instruction = self.code[site];
            }
        }

    }

   fn resolve_patches(&mut self) {
        let patches = self.label_patches.clone();
        for (site, label) in &patches {
            if let Some(&target_idx) = self.labels.get(label.as_str()) {
                let target_addr = self.base_addr + (target_idx as u32 * 4);
                let instr = self.code[*site];
                let opcode = instr >> 26;
                if opcode == 2 || opcode == 3 {
                    let t = self.get_jump_target(target_idx);
                    self.code[*site] = (instr & 0xFC000000) | t;
                } else if opcode == 0x0F {
                    let hi = (target_addr >> 16) & 0xFFFF;
                    let lo = target_addr & 0xFFFF;
                    self.code[*site] = (instr & 0xFFFF0000) | hi;
                    if *site + 1 < self.code.len() {
                        let next_instr = self.code[*site + 1];
                        self.code[*site + 1] = (next_instr & 0xFFFF0000) | lo;
                        self.source_map[*site + 1].instruction = self.code[*site + 1];
                    }
                } else {
                    let offset = (target_idx as i32 - *site as i32 - 1) as i16;
                    self.code[*site] = (instr & 0xFFFF0000) | (offset as u16) as u32;
                }
                self.source_map[*site].instruction = self.code[*site];
            } else {
                eprintln!("[MIPS IR] Unresolved label: '{}'", label);
            }
        }
    }

    fn emit_j_patch(&mut self, label: &str) -> usize {
        let site = self.code.len();
        self.emit(0x08000000);
        self.emit(0x00000000);  
        self.label_patches.push((site, label.to_string()));
        site
    }

    fn emit_branch_patch(&mut self, opcode_bits: u32, rs: u32, rt: u32, label: &str) -> usize {
        let site = self.code.len();
        self.emit(opcode_bits | (rs << 21) | (rt << 16));
        self.emit(0x00000000);  
        self.label_patches.push((site, label.to_string()));
        site
    }

   

   fn emit_module(&mut self, module: &IrModule) {

        
        let mut used_as_label: std::collections::HashSet<String> = std::collections::HashSet::new();
        for instr in &module.instructions {
            for op in &instr.operands {
                if let Operand::Label(name) = op {
                    used_as_label.insert(name.clone());
                }
            }
        }

      
        for instr in &module.instructions {
            if instr.op == IrOp::Rdf {
                if let Operand::VReg(name) = &instr.operands[0] {
                    if !self.data_symbols.contains_key(name) {
                        self.root_vars.insert(name.clone());
                        if used_as_label.contains(name) {
                            if let Operand::Imm(val) = &instr.operands[1] {
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

           
           IrOp::Mk | IrOp::Mf => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    self.register_label(name);
                }
            }

            
            IrOp::Halt => {
                let idx = self.code.len();
                self.emit(0x08000000 | self.get_jump_target(idx));
                self.emit(0x00000000);
            }


     IrOp::Mov => {
    if instr.operands.len() < 2 { return; }
    if let Operand::VReg(name) = &instr.operands[0] {
        if self.root_vars.contains(name) {
            let addr = *self.data_symbols.get(name).unwrap_or(&0);
            let src = self.operand_to_reg(&instr.operands[1], 1);
            let addr_reg = 9u32;
            self.emit_li(addr_reg, addr);
            self.emit(0xAC000000 | (addr_reg << 21) | (src << 16));
            return;
        }
    }
    let src = self.operand_to_reg(&instr.operands[1], 1);
    let dst = self.dest_reg(&instr.operands[0]);
    self.emit(0x00000021 | (src << 21) | (0 << 16) | (dst << 11));
    self.writeback_if_spilled(&instr.operands[0], dst);
}
IrOp::Rdf => {
                if instr.operands.len() < 2 { return; }
                if let Operand::VReg(name) = &instr.operands[0] {
                    let addr = *self.data_symbols.get(name).unwrap_or(&0);
                    let src = self.operand_to_reg(&instr.operands[1], 1);
                    let addr_reg = 9u32;
                    self.emit_li(addr_reg, addr);
                    self.emit(0xAC000000 | (addr_reg << 21) | (src << 16));
                }
            }
    

        
            IrOp::Df => {
                if instr.operands.len() < 2 { return; }
                if let (Operand::Label(name), Operand::Imm(val)) =
                    (&instr.operands[0], &instr.operands[1])
                {
                    let addr = self.next_data;
                    self.data_symbols.insert(name.clone(), addr);
                    
                    let val_reg  = 8u32;
                    let addr_reg = 9u32;
                    self.emit_li(val_reg,  *val as u32);
                    self.emit_li(addr_reg, addr);
                    self.emit(0xAC000000 | (addr_reg << 21) | (val_reg << 16));
                    self.next_data += 4;
                }
            }

           
            IrOp::Bri => {
                if instr.operands.len() < 2 { return; }
                let val  = self.operand_to_reg(&instr.operands[0], 8);
                let addr = self.operand_to_reg(&instr.operands[1], 9);
            
                self.emit(0xAC000000 | (addr << 21) | (val << 16));
            }

           
            IrOp::Get => {
                if instr.operands.len() < 2 { return; }
                let dst  = self.dest_reg(&instr.operands[0]);
                let addr = self.operand_to_reg(&instr.operands[1], 9);
                
                self.emit(0x8C000000 | (addr << 21) | (dst << 16));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

           
        IrOp::Add | IrOp::Sub | IrOp::And | IrOp::Orr | IrOp::Xor => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], 8);
                let r   = self.operand_to_reg(&instr.operands[2], 9);
                let mips_instr = match &instr.op {
                    IrOp::Add => 0x00000021 | (l << 21) | (r << 16) | (dst << 11),
                    IrOp::Sub => 0x00000023 | (l << 21) | (r << 16) | (dst << 11),
                    IrOp::And => 0x00000024 | (l << 21) | (r << 16) | (dst << 11),
                    IrOp::Orr => 0x00000025 | (l << 21) | (r << 16) | (dst << 11),
                    IrOp::Xor => 0x00000026 | (l << 21) | (r << 16) | (dst << 11),
                    _ => unreachable!(),
                };
                self.emit(mips_instr);
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Mul => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], 8);
                let r   = self.operand_to_reg(&instr.operands[2], 9);
                self.emit(0x00000018 | (l << 21) | (r << 16));  
                self.emit(0x00000012 | (dst << 11));              
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Div => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], 8);
                let r   = self.operand_to_reg(&instr.operands[2], 9);
                self.emit(0x0000001B | (l << 21) | (r << 16));  // DIVU
                self.emit(0x00000012 | (dst << 11));              // MFLO
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Rem => {
                if instr.operands.len() < 3 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let l   = self.operand_to_reg(&instr.operands[1], 8);
                let r   = self.operand_to_reg(&instr.operands[2], 9);
                self.emit(0x0000001B | (l << 21) | (r << 16));  // DIVU
                self.emit(0x00000010 | (dst << 11));              // MFHI (remainder)
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

IrOp::Shl => {
    if instr.operands.len() < 3 { return; }
    let dst = self.dest_reg(&instr.operands[0]);
    let l   = self.operand_to_reg(&instr.operands[1], 8);
    let op2 = instr.operands[2].clone();
    match op2 {
        Operand::Imm(sa) => {
            self.emit(0x00000000 | (l << 16) | (dst << 11) | ((sa as u32 & 31) << 6));
        }
        _ => {
            let r = self.operand_to_reg(&op2, 9);
            self.emit(0x00000004 | (r << 21) | (l << 16) | (dst << 11));
        }
    }
    self.writeback_if_spilled(&instr.operands[0], dst);
}

IrOp::Shr => {
    if instr.operands.len() < 3 { return; }
    let dst = self.dest_reg(&instr.operands[0]);
    let l   = self.operand_to_reg(&instr.operands[1], 8);
    let op2 = instr.operands[2].clone();
    match op2 {
        Operand::Imm(sa) => {
            self.emit(0x00000002 | (l << 16) | (dst << 11) | ((sa as u32 & 31) << 6));
        }
        _ => {
            let r = self.operand_to_reg(&op2, 9);
            self.emit(0x00000006 | (r << 21) | (l << 16) | (dst << 11));
        }
    }
    self.writeback_if_spilled(&instr.operands[0], dst);
}
            IrOp::Not => {
                if instr.operands.len() < 2 { return; }
                let dst = self.dest_reg(&instr.operands[0]);
                let src = self.operand_to_reg(&instr.operands[1], 8);
                
                self.emit(0x00000027 | (src << 21) | (0 << 16) | (dst << 11));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Cf => {
                if instr.operands.len() < 2 { return; }
                let l = self.operand_to_reg(&instr.operands[0], 8);
                let r = self.operand_to_reg(&instr.operands[1], 9);
              
                self.emit(0x00000021 | (l << 21) | (0 << 16) | (22 << 11)); 
                self.emit(0x00000021 | (r << 21) | (0 << 16) | (23 << 11)); 
                self.cf_left  = Some(22);
                self.cf_right = Some(23);
            }

            
            IrOp::Jf => {
                
                if instr.operands.len() < 2 { return; }
                let cond = match &instr.operands[0] {
                    Operand::Str(s) => s.clone(),
                    _ => "==".to_string(),
                };
                let label = match &instr.operands[1] {
                    Operand::Label(l) => l.clone(),
                    _ => return,
                };

                let l = self.cf_left.unwrap_or(22);
                let r = self.cf_right.unwrap_or(23);

               
                match cond.as_str() {
                    "==" => { self.emit_branch_patch(0x10000000, l, r, &label); }  
                    "!=" => { self.emit_branch_patch(0x14000000, l, r, &label); } 
                    "<"  => {
              
                        self.emit(0x0000002A | (l << 21) | (r << 16) | (1 << 11));
                        self.emit_branch_patch(0x14000000, 1, 0, &label);
                    }
                    ">=" => {
       
                        self.emit(0x0000002A | (l << 21) | (r << 16) | (1 << 11));
                        self.emit_branch_patch(0x10000000, 1, 0, &label);
                    }
                    ">"  => {
               
                        self.emit(0x0000002A | (r << 21) | (l << 16) | (1 << 11));
                        self.emit_branch_patch(0x14000000, 1, 0, &label);
                    }
                    "<=" => {
         
                        self.emit(0x0000002A | (r << 21) | (l << 16) | (1 << 11));
                        self.emit_branch_patch(0x10000000, 1, 0, &label);
                    }
                    _ => {
                        eprintln!("[MIPS IR] Unknown JF condition: '{}'", cond);
                        self.emit_branch_patch(0x14000000, l, r, &label);
                    }
                }
            }


            IrOp::Go => {
                if let Some(Operand::Label(label)) = instr.operands.first() {
                    self.emit_j_patch(label);
                }
            }

       
          IrOp::Psh => {
    if instr.operands.is_empty() { return; }
    let src = self.operand_to_reg(&instr.operands[0], 8);
    self.emit((0x09 << 26) | (28 << 21) | (28 << 16) | (((-4i16) as u16) as u32));
    self.emit(0xAF800000 | (28 << 21) | (src << 16));
}

        IrOp::Pop => {
    if instr.operands.is_empty() { return; }
    let dst = self.dest_reg(&instr.operands[0]);
    self.emit(0x8F800000 | (28 << 21) | (dst << 16));
    self.emit((0x09 << 26) | (28 << 21) | (28 << 16) | 4);
    self.writeback_if_spilled(&instr.operands[0], dst);

}

           IrOp::Cal => {
                if let Some(Operand::Label(label)) = instr.operands.first() {
                    if label.is_empty() { return; }
                    self.emit(0x00000021 | (31 << 21) | (0 << 16) | (26 << 11));
                    let site = self.code.len();
                    self.emit(0x0C000000);
                    self.emit(0x00000000);
                    self.label_patches.push((site, label.clone()));
                    self.emit(0x00000021 | (26 << 21) | (0 << 16) | (31 << 11));
                }
            }

            IrOp::Ret => {
                if let Some(op) = instr.operands.first() {
                    let src = self.operand_to_reg(op, 2);
                    if src != 2 {
                        self.emit(0x00000021 | (src << 21) | (0 << 16) | (2 << 11));
                    }
                }
                self.emit(0x03E00008);
                self.emit(0x00000000);
            }

            IrOp::Int => {
                if instr.operands.len() < 2 { return; }
                if let Operand::Label(handler) = &instr.operands[1] {
            
                    let site = self.code.len();
                    self.emit(0x3C1A0000);  // LUI 
                    self.emit(0x375A0000);  // ORI 
                    self.label_patches.push((site, handler.clone()));
                }
            }

          
            IrOp::Inb => {
                if instr.operands.len() < 2 { return; }
                let dst  = self.dest_reg(&instr.operands[0]);
                let addr = self.operand_to_reg(&instr.operands[1], 9);
                self.emit(0x8C000000 | (addr << 21) | (dst << 16));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            IrOp::Outb | IrOp::Poke => {
                if instr.operands.len() < 2 { return; }
                let val  = self.operand_to_reg(&instr.operands[0], 8);
                let addr = self.operand_to_reg(&instr.operands[1], 9);
                self.emit(0xAC000000 | (addr << 21) | (val << 16));
            }

            IrOp::Peek => {
                if instr.operands.len() < 2 { return; }
                let dst  = self.dest_reg(&instr.operands[0]);
                let addr = self.operand_to_reg(&instr.operands[1], 9);
                self.emit(0x8C000000 | (addr << 21) | (dst << 16));
                self.writeback_if_spilled(&instr.operands[0], dst);
            }

            // raw hex assembler
            IrOp::Asm => {
    if let Some(Operand::Str(hex)) = instr.operands.first() {
        if let Ok(word) = u32::from_str_radix(hex.trim(), 16) {
            self.emit(word);
        }
    }
}



IrOp::Const => {
    if instr.operands.len() < 2 { return; }
    let dst = self.dest_reg(&instr.operands[0]);
    let val = self.operand_to_reg(&instr.operands[1], 1);
    self.emit(0x00000021 | (val << 21) | (0 << 16) | (dst << 11));
    self.writeback_if_spilled(&instr.operands[0], dst);
}

IrOp::Bnw => {}


IrOp::IntDisable => {
    self.emit(0x400C8000);
    self.emit(0x310CFFFE);
    self.emit(0x410C6000);
}

IrOp::SaveCtx => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    let base = *self.data_symbols.get(name).unwrap_or(&self.next_data);
                    let regs: &[u32] = &[16,17,18,19,20,21,22,23,31,29];
                    for (i, &r) in regs.iter().enumerate() {
                        let addr = base + (i as u32 * 4);
                        self.emit_li(9, addr);
                        self.emit(0xAC000000 | (9 << 21) | (r << 16));
                    }
                }
            }

            IrOp::RestoreCtx => {
                if let Some(Operand::Label(name)) = instr.operands.first() {
                    let base = *self.data_symbols.get(name).unwrap_or(&self.next_data);
                    let regs: &[u32] = &[16,17,18,19,20,21,22,23,31,29];
                    for (i, &r) in regs.iter().enumerate() {
                        let addr = base + (i as u32 * 4);
                        self.emit_li(9, addr);
                        self.emit(0x8C000000 | (9 << 21) | (r << 16));
                    }
                    self.emit(0x03E00008);
                    self.emit(0x00000000);
                }
            }

            IrOp::Comment | IrOp::Syscall | IrOp::StrData => {}
        }
    }
}

impl Backend for MipsBackend {
fn compile(&mut self, module: &IrModule) -> Vec<u8> {
    self.code.clear();
    self.source_map.clear();
    self.labels.clear();
    self.label_patches.clear();
    self.alloc.reset();
    self.emit_li(28, BASE_ADDR + 0x30000);
    self.cf_left  = None;
    self.cf_right = None;


    for instr in &module.instructions {
        if instr.op == crate::ir::IrOp::Rdf {
            if let (Some(crate::ir::Operand::VReg(name)), Some(crate::ir::Operand::Imm(val))) =
                (instr.operands.get(0), instr.operands.get(1))
            {
                match name.as_str() {
                    "BASE"  => { self.base_addr  = *val as u32; }
                    "STACK" => { self.next_data  = *val as u32; }
                    "DATA"  => { self.next_data  = *val as u32; }
                    _ => {}
                }
            }
        }
    }

   
    self.emit(0x00000000);
    self.emit_li(29, self.base_addr + 0x20000); 

    self.emit_module(module);

    self.code.iter()
        .flat_map(|&w| if self.little_endian { w.to_le_bytes().to_vec() } else { w.to_be_bytes().to_vec() })
        .collect()
}

    fn get_source_map(&self) -> Vec<SourceMapEntry> {
        self.source_map.clone()
    }
}


pub struct LegacyCodegen {
    base_addr: u32,
    code: Vec<u32>,
    symbols: HashMap<String, u32>,
    root_symbols: HashMap<String, u32>,
    bnw_entries: Vec<String>,
    bnw_seen_above: bool,
    bnw_seen_below: bool,
    functions: HashMap<String, usize>,
    loop_stack: Vec<LoopContext>,
    current_func_exit: Option<usize>,
    current_params: HashMap<String, u32>,
    local_vars: HashMap<String, u32>,
    reg_pool: VecDeque<u32>,
    next_addr: u32,
    static_data: Vec<(u32, Vec<u8>)>,
    in_function: bool,
    local_offset: u32,
    source_map: Vec<SourceMapEntry>,
    current_line: usize,
    struct_layouts: HashMap<String, Vec<(String, u32)>>,
    struct_sizes: HashMap<String, u32>,
    var_structs: HashMap<String, String>,
}

struct LoopContext {
    start_addr: usize,
    break_patches: Vec<usize>,
}

impl LegacyCodegen {
    pub fn new() -> Self {
        let mut pool = VecDeque::new();
        for i in [8u32,9,10,11,12,13,14,15,16,17,18,19,24,25] {
            pool.push_back(i);
        }
        LegacyCodegen {
            base_addr: BASE_ADDR,
            code: Vec::new(),
            symbols: HashMap::new(),
            root_symbols: HashMap::new(),
            functions: HashMap::new(),
            loop_stack: Vec::new(),
            current_func_exit: None,
            current_params: HashMap::new(),
            local_vars: HashMap::new(),
            reg_pool: pool,
            next_addr: BASE_ADDR,
            static_data: Vec::new(),
            in_function: false,
            local_offset: 0,
            source_map: Vec::new(),
            current_line: 0,
            struct_layouts: HashMap::new(),
            bnw_entries: Vec::new(),
            bnw_seen_above: false,
            bnw_seen_below: false,
            struct_sizes: HashMap::new(),
            var_structs: HashMap::new(),
        }
    }

    fn get_jump_target(&self, index: usize) -> u32 {
        let absolute_addr = self.base_addr + (index as u32 * 4);
        (absolute_addr >> 2) & 0x03FFFFFF
    }

    pub fn compile(&mut self, stmts: &[Statement]) -> Vec<u8> {
        self.code.clear(); self.symbols.clear(); self.static_data.clear();
        self.root_symbols.clear(); self.source_map.clear(); self.functions.clear();
        self.local_vars.clear(); self.current_params.clear(); self.local_offset = 0;
        self.in_function = false; self.loop_stack.clear(); self.current_line = 0;
        self.reg_pool.clear();
        for i in [8u32,9,10,11,12,13,14,15,16,17,18,19,24,25] { self.reg_pool.push_back(i); }
        self.bnw_entries.clear(); self.bnw_seen_above = false; self.bnw_seen_below = false;

        let mut found_non_bnw = false;
        for s in stmts {
            match s {
                Statement::Bnw(text) => {
                    if found_non_bnw { self.bnw_seen_below = true; }
                    else             { self.bnw_seen_above = true; }
                    self.bnw_entries.push(text.clone());
                }
                Statement::Root(_, _, _) => {}
                _ => { found_non_bnw = true; }
            }
        }
        if self.bnw_seen_above && self.bnw_seen_below {
            eprintln!("[BNW ERROR] Cannot mix 'bnw' above and below code!");
            std::process::exit(1);
        }

        self.struct_layouts.clear(); self.struct_sizes.clear(); self.var_structs.clear();
        for s in stmts {
            if let Statement::StructDefine(name, fields) = s {
                let mut offset = 0u32;
                let mut layout = Vec::new();
                for (fname, _) in fields { layout.push((fname.clone(), offset)); offset += 4; }
                self.struct_sizes.insert(name.clone(), offset);
                self.struct_layouts.insert(name.clone(), layout);
            }
        }
        for s in stmts {
            if let Statement::Root(name, Expression::Number(val, _), _) = s {
                self.root_symbols.insert(name.clone(), *val as u32);
            }
        }

        self.base_addr = *self.root_symbols.get("BASE").unwrap_or(&0x80000000);
        self.next_addr = *self.root_symbols.get("DATA").unwrap_or(&(self.base_addr + 0x10000));
        let stack_ptr  = *self.root_symbols.get("STACK").unwrap_or(&(self.base_addr + 0x20000));

        self.emit(0x00000000);
        self.emit_li(29, stack_ptr);
        let gjp = self.code.len();
        self.emit(0x08000000); self.emit(0x00000000);

        for text in &self.bnw_entries.clone() {
            let bytes = text.as_bytes();
            let padded_len = (bytes.len() + 3) & !3;
            let mut padded = bytes.to_vec(); padded.resize(padded_len, 0);
            for chunk in padded.chunks(4) {
                let word = u32::from_be_bytes([chunk[0],chunk[1],chunk[2],chunk[3]]);
                self.emit(word);
            }
        }

        for s in stmts {
            match s {
                Statement::ArrayDefine(name, vals, _) => {
                    let addr = self.next_addr; self.symbols.insert(name.clone(), addr);
                    let mut bytes = Vec::new();
                    for v in vals { bytes.extend_from_slice(&(*v as u32).to_be_bytes()); }
                    self.static_data.push((addr, bytes));
                    self.next_addr += (vals.len() * 4) as u32;
                }
                Statement::StringDefine(name, sv) => {
                    let addr = self.next_addr; self.symbols.insert(name.clone(), addr);
                    let mut bytes = sv.as_bytes().to_vec(); bytes.push(0);
                    self.static_data.push((addr, bytes.clone()));
                    self.next_addr += ((bytes.len() + 3) & !3) as u32;
                }
                Statement::StructInstance(name, st) => {
                    if let Some(&size) = self.struct_sizes.get(st) {
                        let addr = self.next_addr; self.symbols.insert(name.clone(), addr);
                        self.var_structs.insert(name.clone(), st.clone());
                        self.next_addr += size;
                    }
                }
                _ => {}
            }
        }
        for s in stmts {
            if let Statement::FunctionDefine(name,_,_,_) = s { self.functions.entry(name.clone()).or_insert(0); }
        }
        for s in stmts {
            if let Statement::Let(name,_,_) = s {
                self.symbols.entry(name.clone()).or_insert_with(|| { let a=self.next_addr; self.next_addr+=4; a });
            }
        }
        for s in stmts {
            if matches!(s, Statement::FunctionDefine(_,_,_,_)|Statement::IntHandler(_,_)) { self.generate_stmt(s); }
        }

        let init_start = self.code.len();
        self.patch(gjp, 0x08000000 | self.get_jump_target(init_start));

        let sd = self.static_data.clone();
        for (addr, bytes) in sd {
            let mut offset = 0;
            for chunk in bytes.chunks(4) {
                let mut value = 0u32;
                for (i, &b) in chunk.iter().enumerate() { value |= (b as u32) << ((3-i)*8); }
                let vr = self.alloc_reg(); self.emit_li(vr, value);
                let ar = self.alloc_reg(); self.emit_li(ar, addr+offset);
                self.emit(0xAC000000 | (ar << 21) | (vr << 16));
                self.free_reg(ar); self.free_reg(vr);
                offset += 4;
            }
        }

        for s in stmts {
            if !matches!(s, Statement::Root(_,_,_)|Statement::FunctionDefine(_,_,_,_)
                |Statement::ArrayDefine(_,_,_)|Statement::StringDefine(_,_)
                |Statement::StructDefine(_,_)|Statement::StructInstance(_,_)|Statement::IntHandler(_,_))
            { self.generate_stmt(s); }
        }

        let hi = self.code.len();
        self.emit(0x08000000 | self.get_jump_target(hi));
        self.emit(0x00000000);

        self.code.iter().flat_map(|&w| w.to_be_bytes().to_vec()).collect()
    }

    pub fn get_source_map(&self) -> &Vec<SourceMapEntry> { &self.source_map }

    fn generate_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let(_,_,_)|Statement::Assignment(_,_)|Statement::Bnw(_)|
            Statement::Call(_,_)|Statement::Return(_)|Statement::Poke(_,_)|
            Statement::Outb(_,_)|Statement::Asm(_)|Statement::Break|
            Statement::CallPtr(_)|Statement::ArrayAssign(_,_,_) => { self.current_line += 1; }
            _ => {}
        }
        match stmt {
            Statement::FunctionDefine(name,params,body,_) => {
                self.functions.insert(name.clone(), self.code.len());
                self.in_function=true; self.local_offset=0; self.local_vars.clear();
                self.emit(0x27BDFFE0); self.emit(0xAFBF001C);
                self.current_params.clear();
                for (i,(p,_)) in params.iter().enumerate() { self.current_params.insert(p.clone(),(32+i*4) as u32); }
                let ep = self.code.len(); self.emit(0x08000000); self.emit(0x00000000);
                self.current_func_exit = Some(ep);
                for s in body { self.generate_stmt(s); }
                let ei = self.code.len();
                self.patch(ep, 0x08000000 | self.get_jump_target(ei));
                self.emit(0x8FBF001C); self.emit(0x27BD0020); self.emit(0x03E00008); self.emit(0x00000000);
                self.in_function=false; self.current_func_exit=None;
            }
            Statement::Call(func_name,args) => {
                let mut ar = Vec::new();
                for a in args { let r=self.alloc_reg(); self.gen_expr(a,r); ar.push(r); }
                if !args.is_empty() {
                    let sp=(args.len()*4) as u32; let ns=(-(sp as i32)) as u32;
                    self.emit(0x27BD0000|(29<<21)|(29<<16)|(ns&0xFFFF));
                    for (i,&r) in ar.iter().enumerate() { self.emit(0xAC000000|(29<<21)|(r<<16)|((i*4) as u32&0xFFFF)); self.free_reg(r); }
                }
                let pi=self.code.len(); self.emit(0x0C000000); self.emit(0x00000000);
                if let Some(&idx)=self.functions.get(func_name) { let t=self.get_jump_target(idx); self.patch(pi,0x0C000000|t); }
                else { eprintln!("[BRIDGE ERROR] Function '{}' not defined!",func_name); std::process::exit(1); }
                if !args.is_empty() { let sp=(args.len()*4) as u32; self.emit(0x27BD0000|(29<<21)|(29<<16)|(sp&0xFFFF)); }
            }
            Statement::Let(name,value,kind) => {
                let vr=self.alloc_reg(); self.gen_expr(value,vr);
                match value { Expression::Number(_,_)=>{} _=>{ self.apply_type_mask(vr,kind); } }
                if self.in_function {
                    let off=*self.local_vars.entry(name.clone()).or_insert_with(||{ self.local_offset+=4; self.local_offset });
                    self.emit(0xAFA00000|(vr<<16)|(off&0xFFFF));
                } else {
                    let addr=*self.symbols.entry(name.clone()).or_insert_with(||{ let a=self.next_addr; self.next_addr+=4; a });
                    let ar=self.alloc_reg(); self.emit_li(ar,addr); self.emit(0xAC000000|(ar<<21)|(vr<<16)); self.free_reg(ar);
                }
                self.free_reg(vr);
            }
            Statement::StructDefine(_,_) => {}
            Statement::Bnw(text) => {
                if self.in_function { eprintln!("[BNW ERROR] 'bnw' not allowed in functions: '{}'",text); std::process::exit(1); }
            }
            Statement::StructInstance(name,st) => {
                if self.in_function {
                    if let Some(&size)=self.struct_sizes.get(st).cloned().as_ref() {
                        self.var_structs.insert(name.clone(),st.clone());
                        self.local_offset+=size; self.local_vars.insert(name.clone(),self.local_offset);
                    }
                }
            }
            Statement::Assignment(name,value) => {
                if let Some(dp)=name.find('.') {
                    let vn=name[..dp].to_string(); let fn_=name[dp+1..].to_string();
                    let off=self.get_field_offset(&vn,&fn_); let vr=self.alloc_reg(); self.gen_expr(value,vr);
                    if self.in_function { if let Some(&bo)=self.local_vars.get(&vn) { self.emit(0xAFA00000|(vr<<16)|((bo+off)&0xFFFF)); } }
                    else { let ba=*self.symbols.get(&vn).unwrap_or(&0x80010000); let ar=self.alloc_reg(); self.emit_li(ar,ba+off); self.emit(0xAC000000|(ar<<21)|(vr<<16)); self.free_reg(ar); }
                    self.free_reg(vr); return;
                }
                let vr=self.alloc_reg(); self.gen_expr(value,vr);
                if self.in_function {
                    let off=*self.local_vars.entry(name.clone()).or_insert_with(||{ self.local_offset+=4; self.local_offset });
                    self.emit(0xAFA00000|(vr<<16)|(off&0xFFFF));
                } else {
                    let addr=*self.symbols.entry(name.clone()).or_insert_with(||{ let a=self.next_addr; self.next_addr+=4; a });
                    let ar=self.alloc_reg(); self.emit_li(ar,addr); self.emit(0xAC000000|(ar<<21)|(vr<<16)); self.free_reg(ar);
                }
                self.free_reg(vr);
            }
            Statement::ArrayAssign(name,idx_e,val_e) => {
                let ir=self.alloc_reg(); self.gen_expr(idx_e,ir);
                self.emit(0x00000000|(ir<<16)|(ir<<11)|(2<<6));
                let ba=*self.symbols.get(name).unwrap_or(&0x80010000);
                let ar=self.alloc_reg(); self.emit_li(ar,ba);
                self.emit(0x00000021|(ar<<21)|(ir<<16)|(ar<<11));
                let vr=self.alloc_reg(); self.gen_expr(val_e,vr);
                self.emit(0xAC000000|(ar<<21)|(vr<<16));
                self.free_reg(vr); self.free_reg(ar); self.free_reg(ir);
            }
            Statement::If(cond,then_b,else_b) => {
                let cr=self.alloc_reg(); self.gen_expr(cond,cr);
                let bp=self.code.len(); self.emit(0x10000000|(cr<<21)); self.emit(0x00000000); self.free_reg(cr);
                for s in then_b { self.generate_stmt(s); }
                if let Some(eb)=else_b {
                    let jp=self.code.len(); self.emit(0x08000000); self.emit(0x00000000);
                    let es=self.code.len(); self.code[bp]|=((es as i32-bp as i32-1) as u16) as u32;
                    for s in eb { self.generate_stmt(s); }
                    let end=self.code.len(); self.patch(jp,0x08000000|self.get_jump_target(end));
                } else {
                    let end=self.code.len(); self.code[bp]|=((end as i32-bp as i32-1) as u16) as u32;
                }
            }
            Statement::While(cond,body) => {
                let start=self.code.len(); let cr=self.alloc_reg(); self.gen_expr(cond,cr);
                let ep=self.code.len(); self.emit(0x10000000|(cr<<21)); self.emit(0x00000000); self.free_reg(cr);
                self.loop_stack.push(LoopContext{start_addr:start,break_patches:Vec::new()});
                for s in body { self.generate_stmt(s); }
                self.emit(0x08000000|self.get_jump_target(start)); self.emit(0x00000000);
                let end=self.code.len(); let off=((end as i32-ep as i32-1) as u16) as u32;
                self.patch(ep,self.code[ep]|off);
                if let Some(ctx)=self.loop_stack.pop() { for p in ctx.break_patches { self.patch(p,0x08000000|self.get_jump_target(end)); } }
            }
            Statement::Loop(body) => {
                let start=self.code.len();
                self.loop_stack.push(LoopContext{start_addr:start,break_patches:Vec::new()});
                for s in body { self.generate_stmt(s); }
                self.emit(0x08000000|self.get_jump_target(start)); self.emit(0x00000000);
                if let Some(ctx)=self.loop_stack.pop() { let end=self.code.len(); for p in ctx.break_patches { self.patch(p,0x08000000|self.get_jump_target(end)); } }
            }
            Statement::Break => {
                if let Some(ctx)=self.loop_stack.last_mut() { let p=self.code.len(); ctx.break_patches.push(p); self.emit(0x08000000); self.emit(0x00000000); }
            }
            Statement::Return(me) => {
                if let Some(e)=me { self.gen_expr(e,2); }
                if let Some(ex)=self.current_func_exit { self.emit(0x08000000|self.get_jump_target(ex)); self.emit(0x00000000); }
            }
            Statement::Poke(ae,ve) => {
                let ar=self.alloc_reg(); self.gen_expr(ae,ar); let vr=self.alloc_reg(); self.gen_expr(ve,vr);
                self.emit(0xAC000000|(ar<<21)|(vr<<16)); self.free_reg(vr); self.free_reg(ar);
            }
            Statement::Outb(pe,ve) => {
                let pr=self.alloc_reg(); self.gen_expr(pe,pr); let vr=self.alloc_reg(); self.gen_expr(ve,vr);
                self.emit(0xAC000000|(pr<<21)|(vr<<16)); self.free_reg(vr); self.free_reg(pr);
            }
            Statement::Asm(hex) => { if let Ok(i)=u32::from_str_radix(hex,16) { self.emit(i); } }
            Statement::CallPtr(e) => {
                let r=self.alloc_reg(); self.gen_expr(e,r);
                self.emit(0x00000009|(r<<21)|(31<<11)); self.emit(0x00000000); self.free_reg(r);
            }
            Statement::IntHandler(name,body) => {
                self.functions.insert(name.clone(),self.code.len());
                self.emit(0x27BDFFA4); self.emit(0xAFBF0058); self.emit(0xAFB80054);
                self.emit(0xAFB00050); self.emit(0xAFB1004C); self.emit(0xAFB20048); self.emit(0xAFB30044);
                for s in body { self.generate_stmt(s); }
                self.emit(0x8FBF0058); self.emit(0x8FB80054); self.emit(0x8FB00050);
                self.emit(0x8FB1004C); self.emit(0x8FB20048); self.emit(0x8FB30044);
                self.emit(0x27BD005C); self.emit(0x42000018);
            }
            Statement::IntEnable(ve,hn) => {
                if let Some(&addr)=self.functions.get(hn) {
                    let abs=self.base_addr+(addr as u32*4); let vr=self.alloc_reg();
                    self.gen_expr(ve,vr); self.emit_li(vr,abs); self.free_reg(vr);
                } else { eprintln!("[BRIDGE ERROR] Handler '{}' not defined!",hn); std::process::exit(1); }
            }
            Statement::IntDisable => { self.emit(0x400C8000); self.emit(0x310CFFFE); self.emit(0x410C6000); }
            Statement::Root(_,_,_) => {}
            _ => {}
        }
    }

    fn apply_type_mask(&mut self, reg: u32, kind: &TypeKind) {
        match kind {
            TypeKind::U8   => { self.emit(0x30000000|(reg<<21)|(reg<<16)|0xFF); }
            TypeKind::U16  => { self.emit(0x30000000|(reg<<21)|(reg<<16)|0xFFFF); }
            TypeKind::Bool => { self.emit(0x30000000|(reg<<21)|(reg<<16)|0x1); }
            _ => {}
        }
    }

    fn gen_expr(&mut self, expr: &Expression, dest: u32) {
        match expr {
            Expression::Number(n,kind) => { self.emit_li(dest,*n as u32); self.apply_type_mask(dest,kind); }
            Expression::Variable(name) => {
                if let Some(&addr)=self.root_symbols.get(name) { self.emit_li(dest,addr); }
                else if self.in_function {
                    if let Some(&off)=self.current_params.get(name) { self.emit(0x8F000000|(29<<21)|(dest<<16)|(off&0xFFFF)); }
                    else if let Some(&off)=self.local_vars.get(name) { self.emit(0x8F000000|(29<<21)|(dest<<16)|(off&0xFFFF)); }
                    else { let addr=*self.symbols.get(name).unwrap_or(&0x80010000); let ar=self.alloc_reg(); self.emit_li(ar,addr); self.emit(0x8C000000|(ar<<21)|(dest<<16)); self.free_reg(ar); }
                } else {
                    let addr=match self.symbols.get(name) { Some(&a)=>a, None=>{ eprintln!("[BRIDGE ERROR] Var '{}' not defined!",name); std::process::exit(1); } };
                    let ar=self.alloc_reg(); self.emit_li(ar,addr); self.emit(0x8C000000|(ar<<21)|(dest<<16)); self.free_reg(ar);
                }
            }
            Expression::BinaryOp(l,op,r) => {
                let lr=self.alloc_reg(); self.gen_expr(l,lr);
                if (op=="<<" || op==">>") { if let Expression::Number(s,_)=r.as_ref() { let sa=(*s&31) as u32; if op=="<<" { self.emit(0x00000000|(lr<<16)|(dest<<11)|(sa<<6)); } else { self.emit(0x00000002|(lr<<16)|(dest<<11)|(sa<<6)); } self.free_reg(lr); return; } }
                let rr=self.alloc_reg(); self.gen_expr(r,rr);
                match op.as_str() {
                    "+" => self.emit(0x00000021|(lr<<21)|(rr<<16)|(dest<<11)),
                    "-" => self.emit(0x00000023|(lr<<21)|(rr<<16)|(dest<<11)),
                    "*" => { self.emit(0x00000018|(lr<<21)|(rr<<16)); self.emit(0x00000012|(dest<<11)); }
                    "/" => { self.emit(0x0000001A|(lr<<21)|(rr<<16)); self.emit(0x00000012|(dest<<11)); }
                    "^" => self.emit(0x00000026|(lr<<21)|(rr<<16)|(dest<<11)),
                    "&" => self.emit(0x00000024|(lr<<21)|(rr<<16)|(dest<<11)),
                    "|" => self.emit(0x00000025|(lr<<21)|(rr<<16)|(dest<<11)),
                    "==" => { self.emit(0x00000026|(lr<<21)|(rr<<16)|(dest<<11)); self.emit(0x28000001|(dest<<21)|(dest<<16)); }
                    "!=" => { self.emit(0x00000026|(lr<<21)|(rr<<16)|(dest<<11)); self.emit(0x0000002B|(dest<<16)|(dest<<11)); }
                    ">"  => self.emit(0x0000002A|(rr<<21)|(lr<<16)|(dest<<11)),
                    "<"  => self.emit(0x0000002A|(lr<<21)|(rr<<16)|(dest<<11)),
                    ">=" => { self.emit(0x0000002A|(lr<<21)|(rr<<16)|(dest<<11)); self.emit(0x38000001|(dest<<21)|(dest<<16)); }
                    "<=" => { self.emit(0x0000002A|(rr<<21)|(lr<<16)|(dest<<11)); self.emit(0x38000001|(dest<<21)|(dest<<16)); }
                    _ => {}
                }
                self.free_reg(rr); self.free_reg(lr);
            }
            Expression::ArrayAccess(name,idx) => {
                let ir=self.alloc_reg(); self.gen_expr(idx,ir);
                self.emit(0x00000000|(ir<<16)|(ir<<11)|(2<<6));
                let ba=*self.symbols.get(name).unwrap_or(&0x80010000);
                let ar=self.alloc_reg(); self.emit_li(ar,ba);
                self.emit(0x00000021|(ar<<21)|(ir<<16)|(ar<<11));
                self.emit(0x8C000000|(ar<<21)|(dest<<16));
                self.free_reg(ar); self.free_reg(ir);
            }
            Expression::Peek(ae) => { let ar=self.alloc_reg(); self.gen_expr(ae,ar); self.emit(0x8C000000|(ar<<21)|(dest<<16)); self.free_reg(ar); }
            Expression::WaitKey => { self.emit_li(dest,0x80020000); self.emit(0x8C000000|(dest<<21)|(dest<<16)); }
            Expression::Inb(ae) => { let ar=self.alloc_reg(); self.gen_expr(ae,ar); self.emit(0x8C000000|(ar<<21)|(dest<<16)); self.free_reg(ar); }
            Expression::FieldAccess(vn,fn_) => {
                let off=self.get_field_offset(vn,fn_);
                if self.in_function { if let Some(&bo)=self.local_vars.get(vn) { self.emit(0x8FA00000|(dest<<16)|((bo+off)&0xFFFF)); return; } }
                let ba=*self.symbols.get(vn).unwrap_or(&0x80010000); let ar=self.alloc_reg(); self.emit_li(ar,ba+off); self.emit(0x8C000000|(ar<<21)|(dest<<16)); self.free_reg(ar);
            }
            Expression::Call(name,args) => {
                let mut ar_v=Vec::new();
                for a in args { let r=self.alloc_reg(); self.gen_expr(a,r); ar_v.push(r); }
                if !args.is_empty() { let sp=(args.len()*4) as u32; let ns=(-(sp as i32)) as u32; self.emit(0x27BD0000|(29<<21)|(29<<16)|(ns&0xFFFF)); for (i,&r) in ar_v.iter().enumerate() { self.emit(0xAC000000|(29<<21)|(r<<16)|((i*4) as u32&0xFFFF)); self.free_reg(r); } }
                let pi=self.code.len(); self.emit(0x0C000000); self.emit(0x00000000);
                if let Some(&idx)=self.functions.get(name) { let t=self.get_jump_target(idx); self.patch(pi,0x0C000000|t); } else { eprintln!("[BRIDGE ERROR] Function '{}' not defined!",name); std::process::exit(1); }
                self.emit(0x00000021|(0<<21)|(2<<16)|(dest<<11));
                if !args.is_empty() { let sp=(args.len()*4) as u32; self.emit(0x27BD0000|(29<<21)|(29<<16)|(sp&0xFFFF)); }
            }
            Expression::FieldAssign(vn,fn_,ve) => {
                let off=self.get_field_offset(vn,fn_); let vr=self.alloc_reg(); self.gen_expr(ve,vr);
                if self.in_function { if let Some(&bo)=self.local_vars.get(vn) { self.emit(0xAFA00000|(vr<<16)|((bo+off)&0xFFFF)); } }
                else { let ba=*self.symbols.get(vn).unwrap_or(&0x80010000); let ar=self.alloc_reg(); self.emit_li(ar,ba+off); self.emit(0xAC000000|(ar<<21)|(vr<<16)); self.free_reg(ar); }
                self.free_reg(vr);
            }
           Expression::Syscall(_) => {},
            Expression::AddressOf(name) => {
                if let Some(&idx) = self.functions.get(name) {
                    let abs = self.base_addr + (idx as u32 * 4);
                    self.emit_li(dest, abs);
                } else {
                    eprintln!("[BRIDGE ERROR] '&{}' refers to undefined function", name);
                    std::process::exit(1);
                }
            }
        }
    }

    fn get_field_offset(&self, var: &str, field: &str) -> u32 {
        let st=match self.var_structs.get(var) { Some(t)=>t, None=>{ eprintln!("[BRIDGE ERROR] '{}' not a struct",var); std::process::exit(1); } };
        let layout=match self.struct_layouts.get(st) { Some(l)=>l, None=>{ eprintln!("[BRIDGE ERROR] Unknown struct '{}'",st); std::process::exit(1); } };
        layout.iter().find(|(f,_)|f==field).map(|(_,o)|*o).unwrap_or_else(||{ eprintln!("[BRIDGE ERROR] No field '{}'",field); std::process::exit(1); })
    }

    fn emit(&mut self, instr: u32) {
        let addr=self.base_addr+(self.code.len() as u32*4);
        self.source_map.push(SourceMapEntry { line:self.current_line, address:addr, instruction:instr, source:String::new() });
        self.code.push(instr);
    }

    fn patch(&mut self, idx: usize, instr: u32) {
        self.code[idx]=instr; self.source_map[idx].instruction=instr;
    }

    fn emit_li(&mut self, reg: u32, imm: u32) {
        let hi=(imm>>16)&0xFFFF; let lo=imm&0xFFFF;
        if hi==0 { self.emit(0x34000000|(reg<<16)|lo); }
        else { self.emit(0x3C000000|(reg<<16)|hi); if lo!=0 { self.emit(0x34000000|(reg<<21)|(reg<<16)|lo); } }
    }

    fn alloc_reg(&mut self) -> u32 {
        self.reg_pool.pop_front().unwrap_or_else(|| { eprintln!("[BRIDGE WARNING] Register pool exhausted!"); 8 })
    }

    fn free_reg(&mut self, reg: u32) {
        if (8..=15).contains(&reg) && !self.reg_pool.contains(&reg) { self.reg_pool.push_back(reg); }
    }
}
