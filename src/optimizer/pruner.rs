use crate::ast::{Statement, Expression, TypeKind};
use std::collections::{HashMap, HashSet};

pub struct Pruner {
    constants: HashMap<String, u64>,
}

impl Pruner {
    pub fn new() -> Self {
        Pruner {
            constants: HashMap::new(),
        }
    }


pub fn run(&mut self, program: Vec<Statement>) -> Vec<Statement> {
    let folded: Vec<Statement> = program
        .into_iter()
        .map(|s| self.fold_stmt(s))
        .collect();
    let live = self.compute_live_symbols(&folded);
    self.eliminate_dead(folded, &live)
}

    fn compute_live_symbols(&self, stmts: &[Statement]) -> HashSet<String> {
        let mut live = HashSet::new();
        for stmt in stmts {
            self.collect_used_in_stmt(stmt, &mut live);
        }
        live
    }

    fn collect_used_in_stmt(&self, stmt: &Statement, used: &mut HashSet<String>) {
        match stmt {
            Statement::Let(_, expr, _) => self.collect_used_in_expr(expr, used),
            Statement::Root(_, expr, _) => self.collect_used_in_expr(expr, used),
            Statement::Assignment(name, expr) => {
                used.insert(name.clone());
                self.collect_used_in_expr(expr, used);
            }
            Statement::ArrayAssign(name, idx, val) => {
                used.insert(name.clone());
                self.collect_used_in_expr(idx, used);
                self.collect_used_in_expr(val, used);
            }
            Statement::Return(Some(expr)) => self.collect_used_in_expr(expr, used),
            Statement::If(cond, then_b, else_b) => {
                self.collect_used_in_expr(cond, used);
                for s in then_b { self.collect_used_in_stmt(s, used); }
                if let Some(eb) = else_b {
                    for s in eb { self.collect_used_in_stmt(s, used); }
                }
            }
            Statement::While(cond, body) => {
                self.collect_used_in_expr(cond, used);
                for s in body { self.collect_used_in_stmt(s, used); }
            }
            Statement::Loop(body) => {
                for s in body { self.collect_used_in_stmt(s, used); }
            }
            Statement::FunctionDefine(name, _, body, _) => {
                used.insert(name.clone());
                for s in body { self.collect_used_in_stmt(s, used); }
            }
            Statement::Call(name, args) => {
                used.insert(name.clone());
                for a in args { self.collect_used_in_expr(a, used); }
            }
            Statement::Outb(port, val) => {
                self.collect_used_in_expr(port, used);
                self.collect_used_in_expr(val, used);
            }
            Statement::Poke(addr, val) => {
                self.collect_used_in_expr(addr, used);
                self.collect_used_in_expr(val, used);
            }
            Statement::CallPtr(expr) => self.collect_used_in_expr(expr, used),
            Statement::IntHandler(_, body) => {
    for s in body { self.collect_used_in_stmt(s, used); }
}
Statement::IntEnable(expr, _) => {
    self.collect_used_in_expr(expr, used);
}
Statement::IntDisable => {}
_ => {}
        }
    }

    fn collect_used_in_expr(&self, expr: &Expression, used: &mut HashSet<String>) {
        match expr {
            Expression::Variable(name) => { used.insert(name.clone()); }
            Expression::BinaryOp(l, _, r) => {
                self.collect_used_in_expr(l, used);
                self.collect_used_in_expr(r, used);
            }
            Expression::Call(name, args) => {
                used.insert(name.clone());
                for a in args { self.collect_used_in_expr(a, used); }
            }
            Expression::ArrayAccess(name, idx) => {
                used.insert(name.clone());
                self.collect_used_in_expr(idx, used);
            }
            Expression::Peek(addr) => self.collect_used_in_expr(addr, used),
            Expression::Inb(port) => self.collect_used_in_expr(port, used),
            Expression::FieldAccess(var, _) => { used.insert(var.clone()); }
            Expression::FieldAssign(var, _, val) => {
                used.insert(var.clone());
                self.collect_used_in_expr(val, used);
            }
            _ => {}
        }
    }

    fn eliminate_dead(&self, stmts: Vec<Statement>, live: &HashSet<String>) -> Vec<Statement> {
        stmts.into_iter().filter(|s| match s {
            Statement::Root(_, _, _) => true,
            Statement::Let(name, _, _) => live.contains(name),
            Statement::ArrayDefine(name, _, _) => live.contains(name),
            Statement::StringDefine(name, _) => live.contains(name),
            Statement::StructInstance(name, _) => live.contains(name),
            _ => true,
        }).collect()
    }

    fn fold_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::Let(name, expr, kind) => {
                let folded = self.fold_expr(expr);
                if let Expression::Number(v, _) = &folded {
                    self.constants.insert(name.clone(), *v);
                }
                Statement::Let(name, folded, kind)
            }
            Statement::Root(name, expr, kind) => {
                let folded = self.fold_expr(expr);
                if let Expression::Number(v, _) = &folded {
                    self.constants.insert(name.clone(), *v);
                }
                Statement::Root(name, folded, kind)
            }
            Statement::Assignment(name, expr) => {
                let folded = self.fold_expr(expr);
                if let Expression::Number(v, _) = &folded {
                    self.constants.insert(name.clone(), *v);
                } else {
                    self.constants.remove(&name);
                }
                Statement::Assignment(name, folded)
            }
            Statement::ArrayAssign(name, idx, val) => {
                Statement::ArrayAssign(name, self.fold_expr(idx), self.fold_expr(val))
            }
            Statement::Return(Some(expr)) => Statement::Return(Some(self.fold_expr(expr))),
            Statement::If(cond, then_b, else_b) => {
                let cond_folded = self.fold_expr(cond);
                if let Expression::Number(v, _) = &cond_folded {
                    if *v != 0 {
                        let folded_body: Vec<Statement> = then_b.into_iter().map(|s| self.fold_stmt(s)).collect();
                        return Statement::Loop(folded_body);
                    } else if let Some(eb) = else_b {
                        let folded_else: Vec<Statement> = eb.into_iter().map(|s| self.fold_stmt(s)).collect();
                        return Statement::Loop(folded_else);
                    } else {
                        return Statement::Return(None);
                    }
                }
                let then_folded = then_b.into_iter().map(|s| self.fold_stmt(s)).collect();
                let else_folded = else_b.map(|eb| eb.into_iter().map(|s| self.fold_stmt(s)).collect());
                Statement::If(cond_folded, then_folded, else_folded)
            }
            Statement::While(cond, body) => {
                let cond_folded = self.fold_expr(cond);
                if let Expression::Number(0, _) = &cond_folded {
                    return Statement::Return(None);
                }
                let body_folded = body.into_iter().map(|s| self.fold_stmt(s)).collect();
                Statement::While(cond_folded, body_folded)
            }
            Statement::Loop(body) => {
                Statement::Loop(body.into_iter().map(|s| self.fold_stmt(s)).collect())
            }
            Statement::FunctionDefine(name, params, body, ret) => {
                let body_folded = body.into_iter().map(|s| self.fold_stmt(s)).collect();
                Statement::FunctionDefine(name, params, body_folded, ret)
            }
            Statement::Call(name, args) => {
                Statement::Call(name, args.into_iter().map(|a| self.fold_expr(a)).collect())
            }
            Statement::Outb(port, val) => {
                Statement::Outb(self.fold_expr(port), self.fold_expr(val))
            }
            Statement::Poke(addr, val) => {
                Statement::Poke(self.fold_expr(addr), self.fold_expr(val))
            }
            Statement::CallPtr(expr) => Statement::CallPtr(self.fold_expr(expr)),
            Statement::IntHandler(name, body) => {
    Statement::IntHandler(name, body.into_iter().map(|s| self.fold_stmt(s)).collect())
}
other => other,
        }
    }

    fn fold_expr(&self, expr: Expression) -> Expression {
        match expr {
            Expression::Variable(ref name) => {
                if let Some(&v) = self.constants.get(name) {
                    return Expression::Number(v, TypeKind::Unknown);
                }
                expr
            }
            Expression::BinaryOp(left, op, right) => {
                let l = self.fold_expr(*left);
                let r = self.fold_expr(*right);
                if let (Expression::Number(lv, lk), Expression::Number(rv, _)) = (&l, &r) {
                    let lv = *lv;
                    let rv = *rv;
                    let kind = lk.clone();
                    match op.as_str() {
                        "+" => return Expression::Number(lv.wrapping_add(rv), kind),
                        "-" => return Expression::Number(lv.wrapping_sub(rv), kind),
                        "*" => return Expression::Number(lv.wrapping_mul(rv), kind),
                        "/" if rv != 0 => return Expression::Number(lv / rv, kind),
                        "<<" => return Expression::Number(lv << (rv & 63), kind),
                        ">>" => return Expression::Number(lv >> (rv & 63), kind),
                        "&" => return Expression::Number(lv & rv, kind),
                        "|" => return Expression::Number(lv | rv, kind),
                        "^" => return Expression::Number(lv ^ rv, kind),
                        "==" => return Expression::Number(if lv == rv { 1 } else { 0 }, TypeKind::Bool),
                        "!=" => return Expression::Number(if lv != rv { 1 } else { 0 }, TypeKind::Bool),
                        ">" => return Expression::Number(if lv > rv { 1 } else { 0 }, TypeKind::Bool),
                        "<" => return Expression::Number(if lv < rv { 1 } else { 0 }, TypeKind::Bool),
                        ">=" => return Expression::Number(if lv >= rv { 1 } else { 0 }, TypeKind::Bool),
                        "<=" => return Expression::Number(if lv <= rv { 1 } else { 0 }, TypeKind::Bool),
                        _ => {}
                    }
                }
                Expression::BinaryOp(Box::new(l), op, Box::new(r))
            }
            Expression::Peek(addr) => Expression::Peek(Box::new(self.fold_expr(*addr))),
            Expression::Inb(port) => Expression::Inb(Box::new(self.fold_expr(*port))),
            Expression::ArrayAccess(name, idx) => {
                Expression::ArrayAccess(name, Box::new(self.fold_expr(*idx)))
            }
            Expression::FieldAssign(var, field, val) => {
                Expression::FieldAssign(var, field, Box::new(self.fold_expr(*val)))
            }
            other => other,
        }
    }
}
