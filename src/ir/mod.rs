pub mod builder;

// ── IrOp ─────────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq)]
pub enum IrOp {
    Mov, Bri, Cal, Ret, Mk, Mf, Halt, Asm,
    Add, Sub, Mul, Div, Rem,
    And, Orr, Xor, Not, Shl, Shr,
    Cf, Jf, Go,
    Psh, Pop, Get, Df,
    Int, Inb, Outb, Poke, Peek, Syscall,
Const, Bnw, IntDisable, SaveCtx, RestoreCtx, Rdf, StrData,
Comment,
}

// ── Operand ───────────────────────────────────────────────
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    VReg(String),
    Imm(u64),
    Label(String),
    Str(String),
    None,
}

// ── IrInstr ───────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct IrInstr {
    pub op:       IrOp,
    pub operands: Vec<Operand>,
    pub comment:  Option<String>,
}

impl IrInstr {
    pub fn new(op: IrOp, operands: Vec<Operand>) -> Self {
        IrInstr { op, operands, comment: None }
    }

    pub fn with_comment(mut self, c: impl Into<String>) -> Self {
        self.comment = Some(c.into()); self
    }

    // ── Constructors ─────────────────────────────────────
    pub fn halt() -> Self { IrInstr::new(IrOp::Halt, vec![]) }
    pub fn ret()  -> Self { IrInstr::new(IrOp::Ret,  vec![]) }
    pub fn ret_val(val: Operand) -> Self { IrInstr::new(IrOp::Ret, vec![val]) }

    pub fn mk(name: &str)  -> Self { IrInstr::new(IrOp::Mk,  vec![Operand::Label(name.to_string())]) }

    pub fn mf(name: &str)  -> Self { IrInstr::new(IrOp::Mf,  vec![Operand::Label(name.to_string())]) }
    pub fn go(label: &str) -> Self { IrInstr::new(IrOp::Go,  vec![Operand::Label(label.to_string())]) }
    pub fn jf(cond: &str, label: &str) -> Self {
        IrInstr::new(IrOp::Jf, vec![Operand::Str(cond.to_string()), Operand::Label(label.to_string())])
    }

    pub fn mov(dst: Operand, src: Operand) -> Self { IrInstr::new(IrOp::Mov, vec![dst, src]) }
    pub fn bri(val: Operand, addr: Operand) -> Self { IrInstr::new(IrOp::Bri, vec![val, addr]) }
    pub fn get(dst: Operand, addr: Operand) -> Self { IrInstr::new(IrOp::Get, vec![dst, addr]) }

    pub fn cal(name: &str) -> Self { IrInstr::new(IrOp::Cal, vec![Operand::Label(name.to_string())]) }
    pub fn psh(val: Operand) -> Self { IrInstr::new(IrOp::Psh, vec![val]) }
    pub fn pop(dst: Operand) -> Self { IrInstr::new(IrOp::Pop, vec![dst]) }

    pub fn df(label: &str, val: u64) -> Self {
        IrInstr::new(IrOp::Df, vec![Operand::Label(label.to_string()), Operand::Imm(val)])
    }
    pub fn cf(l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Cf, vec![l, r]) }

    pub fn add(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Add, vec![dst, l, r]) }
    pub fn sub(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Sub, vec![dst, l, r]) }
    pub fn mul(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Mul, vec![dst, l, r]) }
    pub fn div(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Div, vec![dst, l, r]) }
    pub fn rem(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Rem, vec![dst, l, r]) }
    pub fn and(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::And, vec![dst, l, r]) }
    pub fn orr(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Orr, vec![dst, l, r]) }
    pub fn xor(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Xor, vec![dst, l, r]) }
    pub fn shl(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Shl, vec![dst, l, r]) }
    pub fn shr(dst: Operand, l: Operand, r: Operand) -> Self { IrInstr::new(IrOp::Shr, vec![dst, l, r]) }

    pub fn int(vector: u64, handler: &str) -> Self {
        IrInstr::new(IrOp::Int, vec![Operand::Imm(vector), Operand::Label(handler.to_string())])
    }
    pub fn save_ctx(name: &str) -> Self {
        IrInstr::new(IrOp::SaveCtx, vec![Operand::Label(name.to_string())])
    }
    pub fn restore_ctx(name: &str) -> Self {
        IrInstr::new(IrOp::RestoreCtx, vec![Operand::Label(name.to_string())])
    }
}

impl std::fmt::Display for IrInstr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let cmt = self.comment.as_deref().map(|c| format!("  ; {}", c)).unwrap_or_default();
        let ops: Vec<String> = self.operands.iter().map(fmt_op).collect();
        match &self.op {
            IrOp::Mk => write!(f, "{}:{}", ops.join(""), cmt),
            IrOp::Mf => write!(f, "Mf {}{}", ops.join(""), cmt),
            _        => write!(f, "{:?} {}{}", self.op, ops.join(", "), cmt),
        }
    }
}

fn fmt_op(op: &Operand) -> String {
    match op {
        Operand::VReg(s)  => format!("%{}", s),
        Operand::Imm(n)   => format!("{}", n),
        Operand::Label(s) => s.clone(),
        Operand::Str(s)   => format!("\"{}\"", s),
        Operand::None     => "_".to_string(),
    }
}

// ── IrModule ──────────────────────────────────────────────
pub struct IrModule {
    pub instructions: Vec<IrInstr>,
}

impl IrModule {
    pub fn new() -> Self { IrModule { instructions: Vec::new() } }
    pub fn push(&mut self, instr: IrInstr) { self.instructions.push(instr); }
    pub fn dump(&self) {
        for instr in &self.instructions {
            if instr.op == IrOp::Mk { println!("{}", instr); }
            else { println!("    {}", instr); }
        }
    }
}
