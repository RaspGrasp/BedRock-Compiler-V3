use crate::ast::{Statement, Expression, TypeKind};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
struct VarInfo {
    kind:      TypeKind,
    used:      bool,
    written:   bool,
    is_global: bool,
    is_param:  bool,
}

#[derive(Debug, Clone)]
struct FuncInfo {
    params:      Vec<(String, TypeKind)>,
    return_type: TypeKind,
    used:        bool,
    has_return:  bool,
}

#[derive(Debug, Clone)]
struct HandlerInfo {
    registered:       bool,
    empty:            bool,
    modifies_globals: Vec<String>,
}

pub struct Verifier {
    scopes:           Vec<HashMap<String, VarInfo>>,
    funcs:            HashMap<String, FuncInfo>,
    handlers:         HashMap<String, HandlerInfo>,
    errors:           Vec<String>,
    warnings:         Vec<String>,
    in_loop:          usize,
    in_handler:       bool,
    in_func:          bool,
    current_func:     Option<String>,
    current_handler:  Option<String>,
    int_disable_seen: bool,
    call_graph:       HashMap<String, HashSet<String>>,
    unreachable:      bool,
}

impl Verifier {
    pub fn new() -> Self {
        Verifier {
            scopes:           vec![HashMap::new()],
            funcs:            HashMap::new(),
            handlers:         HashMap::new(),
            errors:           Vec::new(),
            warnings:         Vec::new(),
            in_loop:          0,
            in_handler:       false,
            in_func:          false,
            current_func:     None,
            current_handler:  None,
            int_disable_seen: false,
            call_graph:       HashMap::new(),
            unreachable:      false,
        }
    }

    pub fn run(&mut self, program: &[Statement]) -> bool {
        self.pre_scan(program);
        for stmt in program {
            self.check_stmt(stmt);
        }
        self.post_checks();
        self.report()
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.unreachable = false;
    }

    fn pop_scope(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            for (name, info) in &scope {
                if !info.used && !info.is_global && !info.is_param {
                    self.warnings.push(format!(
                        "[VERIFIER] '{}' defined but never used", name
                    ));
                }
            }
        }
    }

    fn define_var(&mut self, name: &str, kind: TypeKind, is_global: bool, is_param: bool) {
        if let Some(scope) = self.scopes.last_mut() {
            if scope.contains_key(name) {
                self.warnings.push(format!(
                    "[VERIFIER] '{}' redefined in the same scope", name
                ));
            }
            scope.insert(name.to_string(), VarInfo {
                kind,
                used: is_param,
                written: false,
                is_global,
                is_param,
            });
        }
    }

    fn lookup_var(&self, name: &str) -> Option<&VarInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) { return Some(info); }
        }
        None
    }

    fn mark_used(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(name) {
                info.used = true;
                return;
            }
        }
    }

    fn mark_written(&mut self, name: &str) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(name) {
                info.written = true;
                return;
            }
        }
    }

    fn get_var_kind(&self, name: &str) -> TypeKind {
        self.lookup_var(name)
            .map(|i| i.kind.clone())
            .unwrap_or(TypeKind::Unknown)
    }

    fn is_var_global(&self, name: &str) -> bool {
        self.lookup_var(name).map(|i| i.is_global).unwrap_or(false)
    }

    fn pre_scan(&mut self, stmts: &[Statement]) {
        for stmt in stmts {
            match stmt {
                Statement::FunctionDefine(name, params, body, ret) => {
                    let has_return = Self::body_has_return(body);
                    self.funcs.insert(name.clone(), FuncInfo {
                        params:      params.clone(),
                        return_type: ret.clone(),
                        used:        false,
                        has_return,
                    });
                }
                Statement::IntHandler(name, body) => {
                    self.handlers.insert(name.clone(), HandlerInfo {
                        registered:       false,
                        empty:            body.is_empty(),
                        modifies_globals: Vec::new(),
                    });
                }
                _ => {}
            }
        }
    }

    fn body_has_return(body: &[Statement]) -> bool {
        for stmt in body {
            match stmt {
                Statement::Return(_) => return true,
                Statement::If(_, then_b, Some(else_b)) => {
                    if Self::body_has_return(then_b) && Self::body_has_return(else_b) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn body_has_break(body: &[Statement]) -> bool {
        for stmt in body {
            match stmt {
                Statement::Break => return true,
                Statement::If(_, then_b, else_b) => {
                    if Self::body_has_break(then_b) { return true; }
                    if let Some(eb) = else_b {
                        if Self::body_has_break(eb) { return true; }
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn check_stmt(&mut self, stmt: &Statement) {
        if self.unreachable {
            self.warnings.push("[VERIFIER] Unreachable code detected after return/break".to_string());
            self.unreachable = false;
        }

        match stmt {
            Statement::Let(name, expr, kind) => {
                let expr_type = self.type_of_expr(expr);
                self.check_expr(expr);
                self.check_type_compat(&expr_type, kind, &format!("let '{}'", name));
                self.define_var(name, kind.clone(), false, false);
            }

            Statement::Root(name, expr, kind) => {
                let expr_type = self.type_of_expr(expr);
                self.check_expr(expr);
                self.check_type_compat(&expr_type, kind, &format!("root '{}'", name));
                self.define_var(name, kind.clone(), true, false);
            }

            Statement::Assignment(name, expr) => {
                let expr_type = self.type_of_expr(expr);
                self.check_expr(expr);
                if self.lookup_var(name).is_some() {
                    let var_kind  = self.get_var_kind(name);
                    let is_global = self.is_var_global(name);
                    self.check_type_compat(&expr_type, &var_kind, &format!("assignment to '{}'", name));
                    if self.in_handler && is_global {
                        let handler = self.current_handler.clone().unwrap_or_default();
                        if let Some(h) = self.handlers.get_mut(&handler) {
                            if !h.modifies_globals.contains(name) {
                                h.modifies_globals.push(name.clone());
                            }
                        }
                    }
                    self.mark_written(name);
                    self.mark_used(name);
                } else if !name.contains('.') {
    self.errors.push(format!(
        "[VERIFIER] Assignment to undefined variable '{}'", name
    ));
}
            }

            Statement::ArrayAssign(name, idx, val) => {
                if self.lookup_var(name).is_none() {
                    self.errors.push(format!(
                        "[VERIFIER] Array assign to undefined variable '{}'", name
                    ));
                } else {
                    self.mark_used(name);
                }
                let idx_type = self.type_of_expr(idx);
                self.check_expr(idx);
                self.check_expr(val);
                if !self.is_integer_type(&idx_type) && idx_type != TypeKind::Unknown {
                    self.errors.push(format!(
                        "[VERIFIER] Array index must be integer, got '{}'", idx_type.name()
                    ));
                }
            }

            Statement::Call(name, args) => {
                if let Some(info) = self.funcs.get(name).cloned() {
                    if args.len() != info.params.len() {
                        self.errors.push(format!(
                            "[VERIFIER] '{}' expects {} args, got {}",
                            name, info.params.len(), args.len()
                        ));
                    } else {
                        for (i, (arg, (_, expected_kind))) in args.iter().zip(info.params.iter()).enumerate() {
                            let got = self.type_of_expr(arg);
                            self.check_type_compat(&got, expected_kind, &format!("'{}' arg {}", name, i + 1));
                        }
                    }
                    if let Some(caller) = self.current_func.clone() {
                        self.call_graph.entry(caller).or_default().insert(name.clone());
                    }
                    if let Some(f) = self.funcs.get_mut(name) { f.used = true; }
                } else {
                    self.errors.push(format!(
                        "[VERIFIER] Call to undefined function '{}'", name
                    ));
                }
                for a in args { self.check_expr(a); }
            }

            Statement::Return(expr_opt) => {
                if !self.in_func && !self.in_handler {
                    self.warnings.push("[VERIFIER] return outside function or handler".to_string());
                }
                if let Some(expr) = expr_opt {
                    let ret_type = self.type_of_expr(expr);
                    self.check_expr(expr);
                    if let Some(fname) = self.current_func.clone() {
                        if let Some(info) = self.funcs.get(&fname) {
                            let expected = info.return_type.clone();
                            self.check_type_compat(&ret_type, &expected, &format!("return in '{}'", fname));
                        }
                    }
                }
                self.unreachable = true;
            }

            Statement::If(cond, then_b, else_b) => {
                let cond_type = self.type_of_expr(cond);
                self.check_expr(cond);
                if let Expression::Number(v, _) = cond {
                    self.warnings.push(format!(
                        "[VERIFIER] Condition is always {} — branch may be dead code",
                        if *v != 0 { "true" } else { "false" }
                    ));
                }
                if cond_type != TypeKind::Bool && cond_type != TypeKind::Unknown {
                    self.warnings.push(format!(
                        "[VERIFIER] Condition has type '{}', expected bool", cond_type.name()
                    ));
                }
                self.push_scope();
                for s in then_b { self.check_stmt(s); }
                self.pop_scope();
                self.unreachable = false;
                if let Some(eb) = else_b {
                    self.push_scope();
                    for s in eb { self.check_stmt(s); }
                    self.pop_scope();
                    self.unreachable = false;
                }
            }

            Statement::While(cond, body) => {
                self.check_expr(cond);
                if let Expression::Number(0, _) = cond {
                    self.warnings.push("[VERIFIER] while(0) — loop body never executes".to_string());
                }
                let prev = self.in_loop;
                self.in_loop += 1;
                self.push_scope();
                for s in body { self.check_stmt(s); }
                self.pop_scope();
                self.unreachable = false;
                self.in_loop = prev;
            }

            Statement::Loop(body) => {
                if !Self::body_has_break(body) {
                    self.warnings.push("[VERIFIER] Infinite loop with no break detected".to_string());
                }
                let prev = self.in_loop;
                self.in_loop += 1;
                self.push_scope();
                for s in body { self.check_stmt(s); }
                self.pop_scope();
                self.unreachable = false;
                self.in_loop = prev;
            }

            Statement::Break => {
                if self.in_loop == 0 {
                    self.errors.push("[VERIFIER] break outside of loop".to_string());
                }
                self.unreachable = true;
            }

            Statement::FunctionDefine(name, params, body, ret_type) => {
                if let Some(info) = self.funcs.get(name) {
                    if !info.has_return && ret_type != &TypeKind::Unknown {
                        self.warnings.push(format!(
                            "[VERIFIER] Function '{}' has return type '{}' but no guaranteed return path",
                            name, ret_type.name()
                        ));
                    }
                }
                let prev_func    = self.current_func.clone();
                let prev_in_func = self.in_func;
                self.current_func = Some(name.clone());
                self.in_func = true;
                self.push_scope();
                for (p, kind) in params {
                    self.define_var(p, kind.clone(), false, true);
                }
                for s in body { self.check_stmt(s); }
                self.pop_scope();
                self.unreachable  = false;
                self.in_func      = prev_in_func;
                self.current_func = prev_func;
            }

            Statement::IntHandler(name, body) => {
                if body.is_empty() {
                    self.warnings.push(format!(
                        "[VERIFIER] Handler '{}' has empty body", name
                    ));
                }
                if Self::body_has_break(body) {
                    self.errors.push(format!(
                        "[VERIFIER] Handler '{}' contains break — invalid in interrupt context", name
                    ));
                }
                let prev_handler    = self.current_handler.clone();
                let prev_in_handler = self.in_handler;
                self.current_handler = Some(name.clone());
                self.in_handler = true;
                self.push_scope();
                for s in body { self.check_stmt(s); }
                self.pop_scope();
                self.unreachable     = false;
                self.in_handler      = prev_in_handler;
                self.current_handler = prev_handler;
            }

            Statement::IntEnable(expr, handler_name) => {
                if !self.handlers.contains_key(handler_name.as_str()) {
                    self.errors.push(format!(
                        "[VERIFIER] int_enable references undefined handler '{}'", handler_name
                    ));
                } else if let Some(h) = self.handlers.get_mut(handler_name.as_str()) {
                    h.registered = true;
                }
                if self.int_disable_seen {
                    self.warnings.push(format!(
                        "[VERIFIER] int_enable('{}') called after int_disable — possible logic error",
                        handler_name
                    ));
                }
                self.check_expr(expr);
            }

            Statement::IntDisable => {
                self.int_disable_seen = true;
            }

            Statement::Outb(port, val) => {
                self.check_expr(port);
                self.check_expr(val);
            }

            Statement::Poke(addr, val) => {
                self.check_expr(addr);
                self.check_expr(val);
            }

            Statement::CallPtr(expr) => {
                self.check_expr(expr);
            }

            Statement::Asm(_) => {}

Statement::StructDefine(_, _) => {}

Statement::StructInstance(name, _) => {
    self.define_var(name, TypeKind::Unknown, false, false);
}

_ => {}
        }
    }

    fn check_expr(&mut self, expr: &Expression) {
        match expr {
            Expression::Variable(name) => {
                if self.lookup_var(name).is_none() && !self.funcs.contains_key(name.as_str()) {
                    self.errors.push(format!(
                        "[VERIFIER] Use of undefined variable '{}'", name
                    ));
                } else {
                    self.mark_used(name);
                }
            }

            Expression::BinaryOp(l, op, r) => {
                let lt = self.type_of_expr(l);
                let rt = self.type_of_expr(r);
                if op == "/" {
                    if let Expression::Number(0, _) = r.as_ref() {
                        self.errors.push("[VERIFIER] Division by zero".to_string());
                    }
                }
                if op == ">>" || op == "<<" {
                    if let Expression::Number(v, _) = r.as_ref() {
                        if *v >= 32 {
                            self.warnings.push(format!(
                                "[VERIFIER] Shift by {} exceeds 32-bit width — undefined behavior", v
                            ));
                        }
                    }
                }
                if lt != TypeKind::Unknown && rt != TypeKind::Unknown && lt != rt
                    && !matches!(op.as_str(), "==" | "!=" | ">" | "<" | ">=" | "<=")
                {
                    self.warnings.push(format!(
                        "[VERIFIER] Type mismatch in '{}': '{}' vs '{}'", op, lt.name(), rt.name()
                    ));
                }
                self.check_expr(l);
                self.check_expr(r);
            }

            Expression::Call(name, args) => {
                if let Some(info) = self.funcs.get(name).cloned() {
                    if args.len() != info.params.len() {
                        self.errors.push(format!(
                            "[VERIFIER] '{}' expects {} args, got {}",
                            name, info.params.len(), args.len()
                        ));
                    } else {
                        for (i, (arg, (_, expected))) in args.iter().zip(info.params.iter()).enumerate() {
                            let got = self.type_of_expr(arg);
                            self.check_type_compat(&got, expected, &format!("'{}' arg {}", name, i + 1));
                        }
                    }
                    if let Some(caller) = self.current_func.clone() {
                        self.call_graph.entry(caller).or_default().insert(name.clone());
                    }
                    if let Some(f) = self.funcs.get_mut(name) { f.used = true; }
                } else {
                    self.errors.push(format!(
                        "[VERIFIER] Call to undefined function '{}'", name
                    ));
                }
                for a in args { self.check_expr(a); }
            }

            Expression::ArrayAccess(name, idx) => {
                if self.lookup_var(name).is_none() {
                    self.errors.push(format!(
                        "[VERIFIER] Access to undefined array '{}'", name
                    ));
                } else {
                    self.mark_used(name);
                }
                let idx_type = self.type_of_expr(idx);
                if !self.is_integer_type(&idx_type) && idx_type != TypeKind::Unknown {
                    self.errors.push(format!(
                        "[VERIFIER] Array index must be integer, got '{}'", idx_type.name()
                    ));
                }
                self.check_expr(idx);
            }

            Expression::Peek(addr) | Expression::Inb(addr) => {
                self.check_expr(addr);
            }

            Expression::FieldAccess(var, _) => {
                if self.lookup_var(var).is_none() {
                    self.errors.push(format!(
                        "[VERIFIER] Field access on undefined variable '{}'", var
                    ));
                } else {
                    self.mark_used(var);
                }
            }

            Expression::FieldAssign(var, _, val) => {
                if self.lookup_var(var).is_none() {
                    self.errors.push(format!(
                        "[VERIFIER] Field assign on undefined variable '{}'", var
                    ));
                } else {
                    self.mark_used(var);
                }
                self.check_expr(val);
            }

              
              Expression::AddressOf(name) => {
                // may refer to a function or a data/string symbol — both OK
                if let Some(f) = self.funcs.get_mut(name) {
                    f.used = true;
                }
            }

            _ => {}
        }
    }

    fn type_of_expr(&self, expr: &Expression) -> TypeKind {
        match expr {
            Expression::Number(_, kind) => kind.clone(),
            Expression::Variable(name)  => self.get_var_kind(name),
            Expression::BinaryOp(l, op, r) => {
                match op.as_str() {
                    "==" | "!=" | ">" | "<" | ">=" | "<=" | "||" | "&&" => TypeKind::Bool,
                    _ => {
                        let lt = self.type_of_expr(l);
                        if lt != TypeKind::Unknown { lt } else { self.type_of_expr(r) }
                    }
                }
            }
            Expression::Call(name, _) => {
                self.funcs.get(name.as_str())
                    .map(|f| f.return_type.clone())
                    .unwrap_or(TypeKind::Unknown)
            }
            Expression::Peek(_)              => TypeKind::U32,
            Expression::Inb(_)               => TypeKind::U8,
            Expression::ArrayAccess(name, _) => self.get_var_kind(name),
            _                                => TypeKind::Unknown,
        }
    }

    fn check_type_compat(&mut self, from: &TypeKind, to: &TypeKind, ctx: &str) {
        if *from == TypeKind::Unknown || *to == TypeKind::Unknown { return; }
        if from == to { return; }
        let ok = matches!((from, to),
            (TypeKind::U8,   TypeKind::U16) | (TypeKind::U8,   TypeKind::U32) |
            (TypeKind::U8,   TypeKind::U64) | (TypeKind::U16,  TypeKind::U32) |
            (TypeKind::U16,  TypeKind::U64) | (TypeKind::U32,  TypeKind::U64) |
            (TypeKind::I8,   TypeKind::I16) | (TypeKind::I8,   TypeKind::I32) |
            (TypeKind::I8,   TypeKind::I64) | (TypeKind::I16,  TypeKind::I32) |
            (TypeKind::I16,  TypeKind::I64) | (TypeKind::I32,  TypeKind::I64) |
            (TypeKind::Bool, TypeKind::U8)  | (TypeKind::Bool, TypeKind::U32)
        );
        if !ok {
            self.warnings.push(format!(
                "[VERIFIER] Type mismatch in {}: '{}' used as '{}'", ctx, from.name(), to.name()
            ));
        }
    }

    fn is_integer_type(&self, kind: &TypeKind) -> bool {
        matches!(kind,
            TypeKind::U8  | TypeKind::U16 | TypeKind::U32 | TypeKind::U64 |
            TypeKind::I8  | TypeKind::I16 | TypeKind::I32 | TypeKind::I64
        )
    }

   fn post_checks(&mut self) {
    let handlers: Vec<(String, HandlerInfo)> = self.handlers.clone().into_iter().collect();
    for (name, info) in &handlers {
        if !info.registered {
            self.warnings.push(format!(
                "[VERIFIER] Handler '{}' defined but never registered with int_enable", name
            ));
        }
        for g in &info.modifies_globals {
            self.warnings.push(format!(
                "[VERIFIER] Handler '{}' modifies global '{}' — ensure atomic access", name, g
            ));
        }
    }

    let funcs: Vec<(String, FuncInfo)> = self.funcs.clone().into_iter().collect();
    for (name, info) in &funcs {
        // main is the program entry point (IR builder calls it automatically)
        if !info.used && name != "main" {
            self.warnings.push(format!(
                "[VERIFIER] Function '{}' defined but never called", name
            ));
        }
    }

    let graph = self.call_graph.clone();
    for (func, callees) in &graph {
        if callees.contains(func) {
            self.warnings.push(format!(
                "[VERIFIER] '{}' calls itself recursively — stack overflow risk on bare metal", func
            ));
        }
        for callee in callees {
            if callee != func {
                if let Some(callee_calls) = graph.get(callee) {
                    if callee_calls.contains(func) {
                        self.warnings.push(format!(
                            "[VERIFIER] Mutual recursion: '{}' ↔ '{}' — stack overflow risk", func, callee
                        ));
                    }
                }
            }
        }
    }
}

    fn report(&self) -> bool {
        println!("-------------------------------------------");
        if self.errors.is_empty() && self.warnings.is_empty() {
            println!("[VERIFIER] All checks passed — OK");
            println!("-------------------------------------------");
            return true;
        }
        for w in &self.warnings { eprintln!("{}", w); }
        for e in &self.errors   { eprintln!("{}", e); }
        println!("-------------------------------------------");
        if !self.errors.is_empty() {
            eprintln!("[VERIFIER] {} error(s), {} warning(s)", self.errors.len(), self.warnings.len());
            std::process::exit(1);
        }
        true
    }
}