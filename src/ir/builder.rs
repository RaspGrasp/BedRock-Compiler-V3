use crate::ast::{Statement, Expression};
use super::{IrModule, IrInstr, IrOp, Operand};

pub struct IrBuilder {
    vreg_counter:  usize,
    label_counter: usize,
    break_labels:  Vec<String>,
    struct_layouts: std::collections::HashMap<String, Vec<(String, usize)>>,
}

impl IrBuilder {
    pub fn new() -> Self {
        IrBuilder {
            vreg_counter:   0,
            label_counter:  0,
            break_labels:   Vec::new(),
            struct_layouts: std::collections::HashMap::new(),
        }
    }

      pub fn build(&mut self, program: Vec<Statement>) -> IrModule {
    let mut module = IrModule::new();

    for stmt in &program {
        if let Statement::StructDefine(name, fields) = stmt {
            let layout: Vec<(String, usize)> = fields
                .iter()
                .enumerate()
                .map(|(i, (fname, _))| (fname.clone(), i * 4))
                .collect();
            self.struct_layouts.insert(name.clone(), layout);
        }
    }

    let mut header = Vec::new();
    let mut funcs  = Vec::new();
    let mut entry  = Vec::new();

    for stmt in program {
        match &stmt {
            Statement::FunctionDefine(_, _, _, _) | Statement::IntHandler(_, _) => {
                funcs.push(stmt);
            }
            Statement::Let(_, _, _)
            | Statement::Root(_, _, _)
            | Statement::ArrayDefine(_, _, _)
            | Statement::StringDefine(_, _)
            | Statement::StructDefine(_, _)
            | Statement::StructInstance(_, _) => {
                header.push(stmt);
            }
            _ => {
                entry.push(stmt);
            }
        }
    }


    let has_main = funcs.iter().any(|s| {
        matches!(s, Statement::FunctionDefine(name, _, _, _) if name == "main")
    });

    // 1) globals / roots
    for stmt in &header {
        self.lower_stmt(stmt, &mut module);
    }


    let start_label = self.fresh_label("main_entry");
    module.push(IrInstr::go(&start_label));


    for stmt in &funcs {
        self.lower_stmt(stmt, &mut module);
    }


    module.push(IrInstr::mk(&start_label));


    if has_main {
        module.push(IrInstr::cal("main"));
    }


    for stmt in &entry {
        self.lower_stmt(stmt, &mut module);
    }

    module.push(IrInstr::halt());
    module
}
    fn fresh_vreg(&mut self) -> Operand {
        let n = self.vreg_counter;
        self.vreg_counter += 1;
        Operand::VReg(format!("t{}", n))
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        let n = self.label_counter;
        self.label_counter += 1;
        format!("{}_{}", prefix, n)
    }

    fn lower_stmt(&mut self, stmt: &Statement, out: &mut IrModule) {
        match stmt {

            Statement::Let(name, expr, _) => {
                let src = self.lower_expr(expr, out);
                out.push(IrInstr::mov(Operand::VReg(name.clone()), src));
            }

            Statement::Root(name, expr, _) => {
                let src = self.lower_expr(expr, out);
                out.push(IrInstr::new(IrOp::Rdf, vec![Operand::VReg(name.clone()), src]));
            }
            Statement::Assignment(name, expr) => {
                let src = self.lower_expr(expr, out);
                let safe_name = name.replace('.', "__");
                out.push(IrInstr::mov(Operand::VReg(safe_name), src));
            }

            Statement::ArrayAssign(name, idx_expr, val_expr) => {
                let idx      = self.lower_expr(idx_expr, out);
                let val      = self.lower_expr(val_expr, out);
                let addr_reg = self.fresh_vreg();
                out.push(IrInstr::add(addr_reg.clone(), Operand::Label(name.clone()), idx));
                out.push(IrInstr::bri(val, addr_reg));
            }

            Statement::ArrayDefine(name, values, _) => {
                for (i, &v) in values.iter().enumerate() {
                    out.push(IrInstr::df(&format!("{}__{}", name, i), v));
                }
            }

            Statement::StringDefine(name, s) => {
                // One contiguous string in rodata; symbol `name` points to first byte
                out.push(IrInstr::new(
                    IrOp::StrData,
                    vec![Operand::Label(name.clone()), Operand::Str(s.clone())],
                ));
            }

            Statement::Poke(addr_expr, val_expr) => {
                let addr = self.lower_expr(addr_expr, out);
                let val  = self.lower_expr(val_expr, out);
                out.push(IrInstr::bri(val, addr));
            }

            Statement::Outb(port_expr, val_expr) => {
                let port = self.lower_expr(port_expr, out);
                let val  = self.lower_expr(val_expr, out);
                out.push(IrInstr::new(IrOp::Outb, vec![val, port]));
            }
                       
            Statement::FunctionDefine(name, params, body, _) => {
                out.push(IrInstr::mf(name));
                for (pname, _) in params {
                    out.push(IrInstr::pop(Operand::VReg(pname.clone())));
                }
                for s in body {
                    self.lower_stmt(s, out);
                }
              
                let ends_with_ret = matches!(
                    body.last(),
                    Some(Statement::Return(_))
                );
                if !ends_with_ret {
                    out.push(IrInstr::ret());
                }
            }

            Statement::Return(None) => {
                out.push(IrInstr::ret());
            }

            Statement::Return(Some(expr)) => {
                let val = self.lower_expr(expr, out);
                out.push(IrInstr::ret_val(val));
            }

            Statement::Call(name, args) => {
                self.lower_call(name, args, out);
            }

            Statement::CallPtr(expr) => {
                let ptr = self.lower_expr(expr, out);
                out.push(IrInstr::new(IrOp::Cal, vec![ptr]));
            }

            Statement::Loop(body) => {
                let head = self.fresh_label("loop_head");
                let exit = self.fresh_label("loop_exit");
                out.push(IrInstr::mk(&head));
                self.break_labels.push(exit.clone());
                for s in body {
                    self.lower_stmt(s, out);
                }
                self.break_labels.pop();
                out.push(IrInstr::go(&head));
                out.push(IrInstr::mk(&exit));
            }

            Statement::While(cond_expr, body) => {
                let head = self.fresh_label("while_head");
                let exit = self.fresh_label("while_exit");
                out.push(IrInstr::mk(&head));
                let (cf_op, inv_cond) = self.lower_condition(cond_expr, out);
                out.push(cf_op);
                out.push(IrInstr::jf(&inv_cond, &exit));
                self.break_labels.push(exit.clone());
                for s in body {
                    self.lower_stmt(s, out);
                }
                self.break_labels.pop();
                out.push(IrInstr::go(&head));
                out.push(IrInstr::mk(&exit));
            }

            Statement::If(cond_expr, then_body, else_body) => {
                let else_lbl = self.fresh_label("if_else");
                let end_lbl  = self.fresh_label("if_end");
                let (cf_op, inv_cond) = self.lower_condition(cond_expr, out);
                out.push(cf_op);
                out.push(IrInstr::jf(&inv_cond, &else_lbl));
                for s in then_body {
                    self.lower_stmt(s, out);
                }
                out.push(IrInstr::go(&end_lbl));
                out.push(IrInstr::mk(&else_lbl));
                if let Some(eb) = else_body {
                    for s in eb {
                        self.lower_stmt(s, out);
                    }
                }
                out.push(IrInstr::mk(&end_lbl));
            }

            Statement::Break => {
                match self.break_labels.last() {
                    Some(lbl) => {
                        let lbl = lbl.clone();
                        out.push(IrInstr::go(&lbl));
                    }
                    None => {
                        eprintln!("[IR ERROR] break outside loop");
                        std::process::exit(1);
                    }
                }
            }

              Statement::IntHandler(name, body) => {
                out.push(IrInstr::mf(name));
                for s in body {
                    self.lower_stmt(s, out);
                }
                out.push(IrInstr::ret());
            }

            Statement::IntEnable(vec_expr, handler_name) => {
                let vec_op = self.lower_expr(vec_expr, out);
                let vector_imm = match &vec_op {
                    Operand::Imm(v) => *v,
                    _ => {
                        eprintln!("[IR WARNING] int_enable with non-constant vector");
                        0
                    }
                };
                out.push(IrInstr::int(vector_imm, handler_name));
            }

            Statement::IntDisable => {
                out.push(IrInstr::new(IrOp::IntDisable, vec![]));
            }

         

           Statement::StructDefine(_, _) => {}

            Statement::StructInstance(_, _) => {}

            Statement::Asm(raw) => {
                out.push(IrInstr::new(IrOp::Asm, vec![Operand::Str(raw.clone())]));
            }

            Statement::Bnw(msg) => {
                out.push(IrInstr::new(IrOp::Bnw, vec![Operand::Str(msg.clone())]));
            }
            Statement::SaveContext(name) => {
                out.push(IrInstr::save_ctx(name));
            }
            Statement::RestoreContext(name) => {
                out.push(IrInstr::restore_ctx(name));
            }
        }
    }

    fn lower_expr(&mut self, expr: &Expression, out: &mut IrModule) -> Operand {
        match expr {

            Expression::Number(v, _) => Operand::Imm(*v),

            Expression::Variable(name) => Operand::VReg(name.clone()),

            Expression::BinaryOp(lhs, op, rhs) => {
                // short-circuit logical operators
                if op == "||" {
                    return self.lower_or_sc(lhs, rhs, out);
                }
                if op == "&&" {
                    return self.lower_and_sc(lhs, rhs, out);
                }

                let l = self.lower_expr(lhs, out);
                let r = self.lower_expr(rhs, out);
                let dest = self.fresh_vreg();
                let instr = match op.as_str() {
                    "+"  => IrInstr::add(dest.clone(), l, r),
                    "-"  => IrInstr::sub(dest.clone(), l, r),
                    "*"  => IrInstr::mul(dest.clone(), l, r),
                    "/"  => IrInstr::div(dest.clone(), l, r),
                    "%"  => IrInstr::rem(dest.clone(), l, r),
                    "&"  => IrInstr::and(dest.clone(), l, r),
                    "|"  => IrInstr::orr(dest.clone(), l, r),
                    "^"  => IrInstr::xor(dest.clone(), l, r),
                    "<<" => IrInstr::shl(dest.clone(), l, r),
                    ">>" => IrInstr::shr(dest.clone(), l, r),
                    "==" | "!=" | "<" | ">" | "<=" | ">=" => {
                        return self.lower_comparison(l, op, r, out);
                    }
                    unknown => {
                        eprintln!("[IR ERROR] Unknown binary op: {}", unknown);
                        return Operand::Imm(0);
                    }
                };
                out.push(instr);
                dest
            }

            Expression::Peek(addr_expr) => {
                let addr = self.lower_expr(addr_expr, out);
                let dest = self.fresh_vreg();
                out.push(IrInstr::get(dest.clone(), addr));
                dest
            }

            Expression::Inb(port_expr) => {
                let port = self.lower_expr(port_expr, out);
                let dest = self.fresh_vreg();
                out.push(IrInstr::new(IrOp::Inb, vec![dest.clone(), port]));
                dest
            }

            Expression::ArrayAccess(name, idx_expr) => {
                let idx  = self.lower_expr(idx_expr, out);
                let addr = self.fresh_vreg();
                out.push(IrInstr::add(addr.clone(), Operand::Label(name.clone()), idx));
                let dest = self.fresh_vreg();
                out.push(IrInstr::get(dest.clone(), addr));
                dest
            }

           Expression::Call(name, args) => {
                self.lower_call(name, args, out);
                let result = self.fresh_vreg();
                out.push(IrInstr::mov(
                    result.clone(),
                    Operand::VReg("__a0".to_string()),
                ));
                result
            }
           Expression::FieldAccess(var, field) => {
                let dest = self.fresh_vreg();
                out.push(IrInstr::mov(
                    dest.clone(),
                    Operand::VReg(format!("{}__{}", var, field)),
                ));
                dest
            }

            Expression::FieldAssign(var, field, val_expr) => {
                let val  = self.lower_expr(val_expr, out);
                let dest = Operand::VReg(format!("{}__{}", var, field));
                out.push(IrInstr::mov(dest.clone(), val));
                dest
            }
            Expression::AddressOf(name) => {
                Operand::Label(name.clone())
            }

            Expression::Syscall(args) => {
                let mut ops = Vec::new();
                for a in args {
                    ops.push(self.lower_expr(a, out));
                }
                let dest = self.fresh_vreg();
                let mut all = vec![dest.clone()];
                all.extend(ops);
                out.push(IrInstr::new(IrOp::Syscall, all));
                dest
            }

            Expression::WaitKey => {
                let dest = self.fresh_vreg();
                out.push(IrInstr::get(
                    dest.clone(),
                    Operand::Label("__waitkey".to_string()),
                ));
                dest
            }
        }
    }

    fn lower_call(&mut self, name: &str, args: &[Expression], out: &mut IrModule) {
        // Evaluate ALL arguments first (including nested calls), then push.
        // Avoids interleaving outer pushes with inner call frames on the stack.
        let mut ops = Vec::new();
        for arg in args {
            ops.push(self.lower_expr(arg, out));
        }
        for op in ops.iter().rev() {
            out.push(IrInstr::psh(op.clone()));
        }
        out.push(IrInstr::cal(name));
    }

    fn lower_condition(
        &mut self,
        expr: &Expression,
        out: &mut IrModule,
    ) -> (IrInstr, String) {
        match expr {
            Expression::BinaryOp(lhs, op, rhs)
                if matches!(op.as_str(), "==" | "!=" | "<" | ">" | "<=" | ">=") =>
            {
                let l   = self.lower_expr(lhs, out);
                let r   = self.lower_expr(rhs, out);
                let inv = invert_cond(op);
                (IrInstr::cf(l, r), inv)
            }
            other => {
                // ||, &&, arithmetic, variables, etc. → evaluate then test != 0
                let val = self.lower_expr(other, out);
                (IrInstr::cf(val, Operand::Imm(0)), "==".to_string())
            }
        }
    }


    // a || b  →  if a != 0 then 1 else (b != 0)
    fn lower_or_sc(&mut self, lhs: &Expression, rhs: &Expression, out: &mut IrModule) -> Operand {
        let dest = self.fresh_vreg();
        let true_lbl = self.fresh_label("or_true");
        let end_lbl  = self.fresh_label("or_end");

        let l = self.lower_expr(lhs, out);
        out.push(IrInstr::cf(l, Operand::Imm(0)));
        out.push(IrInstr::jf("!=", &true_lbl)); // if l != 0 → true

        let r = self.lower_expr(rhs, out);
        out.push(IrInstr::cf(r, Operand::Imm(0)));
        out.push(IrInstr::jf("!=", &true_lbl)); // if r != 0 → true

        out.push(IrInstr::mov(dest.clone(), Operand::Imm(0)));
        out.push(IrInstr::go(&end_lbl));
        out.push(IrInstr::mk(&true_lbl));
        out.push(IrInstr::mov(dest.clone(), Operand::Imm(1)));
        out.push(IrInstr::mk(&end_lbl));
        dest
    }

    // a && b  →  if a == 0 then 0 else (b != 0)
    fn lower_and_sc(&mut self, lhs: &Expression, rhs: &Expression, out: &mut IrModule) -> Operand {
        let dest = self.fresh_vreg();
        let false_lbl = self.fresh_label("and_false");
        let end_lbl   = self.fresh_label("and_end");

        let l = self.lower_expr(lhs, out);
        out.push(IrInstr::cf(l, Operand::Imm(0)));
        out.push(IrInstr::jf("==", &false_lbl)); // if l == 0 → false

        let r = self.lower_expr(rhs, out);
        out.push(IrInstr::cf(r, Operand::Imm(0)));
        out.push(IrInstr::jf("==", &false_lbl)); // if r == 0 → false

        out.push(IrInstr::mov(dest.clone(), Operand::Imm(1)));
        out.push(IrInstr::go(&end_lbl));
        out.push(IrInstr::mk(&false_lbl));
        out.push(IrInstr::mov(dest.clone(), Operand::Imm(0)));
        out.push(IrInstr::mk(&end_lbl));
        dest
    }

    fn lower_comparison(
        &mut self,
        l: Operand,
        op: &str,
        r: Operand,
        out: &mut IrModule,
    ) -> Operand {
        let dest = self.fresh_vreg();
        let skip = self.fresh_label("cmp_skip");
        let inv  = invert_cond(op);
        out.push(IrInstr::cf(l, r));
        out.push(IrInstr::mov(dest.clone(), Operand::Imm(0)));
        out.push(IrInstr::jf(&inv, &skip));
        out.push(IrInstr::mov(dest.clone(), Operand::Imm(1)));
        out.push(IrInstr::mk(&skip));
        dest
    }
}

fn invert_cond(op: &str) -> String {
    match op {
        "==" => "!=",
        "!=" => "==",
        "<"  => ">=",
        ">"  => "<=",
        "<=" => ">",
        ">=" => "<",
        other => {
            eprintln!("[IR WARNING] Unknown condition '{}', defaulting to !=", other);
            "!="
        }
    }.to_string()
}
