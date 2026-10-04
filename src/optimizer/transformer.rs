use crate::ast::{Statement, Expression, TypeKind};
use std::collections::HashSet;

pub struct Transformer;

impl Transformer {
    pub fn new() -> Self {
        Transformer
    }

    pub fn run(&self, program: Vec<Statement>) -> Vec<Statement> {
        program.into_iter().map(|s| self.transform_stmt(s)).collect()
    }

    fn transform_stmt(&self, stmt: Statement) -> Statement {
        match stmt {
            Statement::FunctionDefine(name, params, body, ret) => {
                let body = self.transform_body(body);
                Statement::FunctionDefine(name, params, body, ret)
            }
            Statement::Loop(body) => {
                let (hoisted, body) = self.licm(body);
                let body = self.transform_body(body);
                if hoisted.is_empty() {
                    Statement::Loop(body)
                } else {
                    let mut outer = hoisted;
                    outer.push(Statement::Loop(body));
                    Statement::Loop(outer)
                }
            }
Statement::While(cond, body) => {
    let cond = self.strength_reduce_expr(cond);
    let (_, body) = self.licm(body);
    let body = self.transform_body(body);
    Statement::While(cond, body)
}
            Statement::If(cond, then_b, else_b) => {
                let cond = self.strength_reduce_expr(cond);
                let then_b = self.transform_body(then_b);
                let else_b = else_b.map(|eb| self.transform_body(eb));
                Statement::If(cond, then_b, else_b)
            }
            Statement::Let(name, expr, kind) => {
                Statement::Let(name, self.strength_reduce_expr(expr), kind)
            }
            Statement::Root(name, expr, kind) => {
                Statement::Root(name, self.strength_reduce_expr(expr), kind)
            }
            Statement::Assignment(name, expr) => {
                Statement::Assignment(name, self.strength_reduce_expr(expr))
            }
            Statement::ArrayAssign(name, idx, val) => {
                Statement::ArrayAssign(
                    name,
                    self.strength_reduce_expr(idx),
                    self.strength_reduce_expr(val),
                )
            }
            Statement::Return(Some(expr)) => {
                Statement::Return(Some(self.strength_reduce_expr(expr)))
            }
            Statement::Outb(port, val) => {
                Statement::Outb(
                    self.strength_reduce_expr(port),
                    self.strength_reduce_expr(val),
                )
            }
            Statement::Poke(addr, val) => {
                Statement::Poke(
                    self.strength_reduce_expr(addr),
                    self.strength_reduce_expr(val),
                )
            }
            Statement::Call(name, args) => {
                Statement::Call(
                    name,
                    args.into_iter().map(|a| self.strength_reduce_expr(a)).collect(),
                )
            }
            Statement::CallPtr(expr) => {
                Statement::CallPtr(self.strength_reduce_expr(expr))
            }
            Statement::IntHandler(name, body) => {
    Statement::IntHandler(name, self.transform_body(body))
}
other => other,
        }
    }

    fn transform_body(&self, body: Vec<Statement>) -> Vec<Statement> {
        body.into_iter().map(|s| self.transform_stmt(s)).collect()
    }

    fn licm(&self, body: Vec<Statement>) -> (Vec<Statement>, Vec<Statement>) {
        let loop_modified = self.collect_modified(&body);
        let mut hoisted = Vec::new();
        let mut remaining = Vec::new();

        for stmt in body {
            if self.is_hoistable(&stmt, &loop_modified) {
                hoisted.push(stmt);
            } else {
                remaining.push(stmt);
            }
        }

        (hoisted, remaining)
    }

    fn is_hoistable(&self, stmt: &Statement, modified: &HashSet<String>) -> bool {
        match stmt {
            Statement::Let(_, expr, _) => self.expr_loop_invariant(expr, modified),
            Statement::Assignment(name, expr) => {
                !modified.contains(name) && self.expr_loop_invariant(expr, modified)
            }
            _ => false,
        }
    }

    fn expr_loop_invariant(&self, expr: &Expression, modified: &HashSet<String>) -> bool {
        match expr {
            Expression::Number(_, _) => true,
            Expression::Variable(name) => !modified.contains(name),
            Expression::BinaryOp(l, _, r) => {
                self.expr_loop_invariant(l, modified) && self.expr_loop_invariant(r, modified)
            }
            Expression::ArrayAccess(name, idx) => {
                !modified.contains(name) && self.expr_loop_invariant(idx, modified)
            }
            _ => false,
        }
    }

    fn collect_modified(&self, body: &[Statement]) -> HashSet<String> {
        let mut set = HashSet::new();
        for stmt in body {
            match stmt {
                Statement::Let(name, _, _) => { set.insert(name.clone()); }
                Statement::Assignment(name, _) => { set.insert(name.clone()); }
                Statement::ArrayAssign(name, _, _) => { set.insert(name.clone()); }
                Statement::If(_, then_b, else_b) => {
                    set.extend(self.collect_modified(then_b));
                    if let Some(eb) = else_b {
                        set.extend(self.collect_modified(eb));
                    }
                }
                Statement::While(_, b) | Statement::Loop(b) => {
                    set.extend(self.collect_modified(b));
                }
                _ => {}
            }
        }
        set
    }

    fn strength_reduce_expr(&self, expr: Expression) -> Expression {
        match expr {
            Expression::BinaryOp(left, op, right) => {
                let left = self.strength_reduce_expr(*left);
                let right = self.strength_reduce_expr(*right);

                match op.as_str() {
                    "*" => {
                        if let Expression::Number(v, _) = &right {
                            if *v > 0 && v.is_power_of_two() {
                                let shift = v.trailing_zeros() as u64;
                                return Expression::BinaryOp(
                                    Box::new(left),
                                    "<<".to_string(),
                                    Box::new(Expression::Number(shift, TypeKind::U8)),
                                );
                            }
                        }
                        if let Expression::Number(v, _) = &left {
                            if *v > 0 && v.is_power_of_two() {
                                let shift = v.trailing_zeros() as u64;
                                return Expression::BinaryOp(
                                    Box::new(right),
                                    "<<".to_string(),
                                    Box::new(Expression::Number(shift, TypeKind::U8)),
                                );
                            }
                        }
                        Expression::BinaryOp(Box::new(left), op, Box::new(right))
                    }
                    "/" => {
                        if let Expression::Number(v, _) = &right {
                            if *v > 0 && v.is_power_of_two() {
                                let shift = v.trailing_zeros() as u64;
                                return Expression::BinaryOp(
                                    Box::new(left),
                                    ">>".to_string(),
                                    Box::new(Expression::Number(shift, TypeKind::U8)),
                                );
                            }
                        }
                        Expression::BinaryOp(Box::new(left), op, Box::new(right))
                    }
                    "%" => {
                        if let Expression::Number(v, _) = &right {
                            if *v > 0 && v.is_power_of_two() {
                                let mask = v - 1;
                                return Expression::BinaryOp(
                                    Box::new(left),
                                    "&".to_string(),
                                    Box::new(Expression::Number(mask, TypeKind::U32)),
                                );
                            }
                        }
                        Expression::BinaryOp(Box::new(left), op, Box::new(right))
                    }
                    "+" => {
                        if let Expression::Number(0, _) = &left {
                            return right;
                        }
                        if let Expression::Number(0, _) = &right {
                            return left;
                        }
                        Expression::BinaryOp(Box::new(left), op, Box::new(right))
                    }
                    "-" => {
                        if let Expression::Number(0, _) = &right {
                            return left;
                        }
                        Expression::BinaryOp(Box::new(left), op, Box::new(right))
                    }
                    _ => Expression::BinaryOp(Box::new(left), op, Box::new(right)),
                }
            }
            Expression::Peek(addr) => Expression::Peek(Box::new(self.strength_reduce_expr(*addr))),
            Expression::Inb(port) => Expression::Inb(Box::new(self.strength_reduce_expr(*port))),
            Expression::ArrayAccess(name, idx) => {
                Expression::ArrayAccess(name, Box::new(self.strength_reduce_expr(*idx)))
            }
            Expression::FieldAssign(var, field, val) => {
                Expression::FieldAssign(var, field, Box::new(self.strength_reduce_expr(*val)))
            }
            Expression::Call(name, args) => {
                Expression::Call(
                    name,
                    args.into_iter().map(|a| self.strength_reduce_expr(a)).collect(),
                )
            }
            other => other,
        }
    }
}
