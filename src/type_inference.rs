use crate::ast::{Statement, Expression, TypeKind};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct FuncSignature {

    params: Vec<TypeKind>,
 
    return_type: TypeKind,
}


#[derive(Debug, Clone)]
struct TypeEnv {
    
    vars: HashMap<String, TypeKind>,
  
    funcs: HashMap<String, FuncSignature>,
}

impl TypeEnv {
    fn new() -> Self {
        TypeEnv {
            vars: HashMap::new(),
            funcs: HashMap::new(),
        }
    }

    fn child(&self) -> Self {
        TypeEnv {
            vars: self.vars.clone(),
            funcs: self.funcs.clone(),
        }
    }

    fn set_var(&mut self, name: &str, kind: TypeKind) {
        self.vars.insert(name.to_string(), kind);
    }

    fn get_var(&self, name: &str) -> Option<&TypeKind> {
        self.vars.get(name)
    }

    fn set_func(&mut self, name: &str, sig: FuncSignature) {
        self.funcs.insert(name.to_string(), sig);
    }

    fn get_func(&self, name: &str) -> Option<&FuncSignature> {
        self.funcs.get(name)
    }
}


pub struct TypeInferencer {
    env: TypeEnv,
  
    current_func: Option<String>,
   
    warning_count: usize,
   
    error_count: usize,
}

impl TypeInferencer {
    pub fn new() -> Self {
        TypeInferencer {
            env: TypeEnv::new(),
            current_func: None,
            warning_count: 0,
            error_count: 0,
        }
    }


    pub fn run(&mut self, stmts: Vec<Statement>) -> Vec<Statement> {
    
        self.prescan_functions(&stmts);

   
        let result: Vec<Statement> = stmts
            .into_iter()
            .map(|s| self.infer_stmt(s))
            .collect();

        self.report_summary();

        result
    }


    fn prescan_functions(&mut self, stmts: &[Statement]) {
        for stmt in stmts {
            if let Statement::FunctionDefine(name, params, _, return_type) = stmt {
                let sig = FuncSignature {
                    params: params.iter().map(|(_, k)| {
                        if *k == TypeKind::Unknown { TypeKind::U32 } else { k.clone() }
                    }).collect(),
                    return_type: return_type.clone(), // Unknown = void (no return required)
                };
                self.env.set_func(name, sig);
            }
        }
    }


    fn infer_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {

   
            Statement::Let(name, expr, kind) => {
                let inferred_expr = self.infer_expr(expr);
                let expr_type = self.type_of_expr(&inferred_expr);

                let final_kind = if kind == TypeKind::Unknown {
                 
                    if expr_type == TypeKind::Unknown {
                        self.warn(&format!(
                            "variable '{}' has no type annotation, defaulting to u32", name
                        ));
                        TypeKind::U32
                    } else {
                        expr_type.clone()
                    }
                } else {
                  
                    self.check_assignable(&expr_type, &kind, &name);
                    kind
                };

                self.env.set_var(&name, final_kind.clone());
                Statement::Let(name, inferred_expr, final_kind)
            }

            Statement::Root(name, expr, kind) => {
                let inferred_expr = self.infer_expr(expr);
                let expr_type = self.type_of_expr(&inferred_expr);

                let final_kind = if kind == TypeKind::Unknown {
                    TypeKind::U32
                } else {
                    self.check_assignable(&expr_type, &kind, &name);
                    kind
                };

                self.env.set_var(&name, final_kind.clone());
                Statement::Root(name, inferred_expr, final_kind)
            }

            Statement::Assignment(name, expr) => {
                let inferred_expr = self.infer_expr(expr);
                let expr_type = self.type_of_expr(&inferred_expr);

                if let Some(var_type) = self.env.get_var(&name).cloned() {
                    if var_type != TypeKind::Unknown
                        && expr_type != TypeKind::Unknown
                        && !self.types_compatible(&expr_type, &var_type)
                    {
                        self.error(&format!(
                            "cannot assign '{}' to variable '{}' of type '{}'",
                            expr_type.name(), name, var_type.name()
                        ));
                    }
                }

                Statement::Assignment(name, inferred_expr)
            }

            Statement::FunctionDefine(name, params, body, return_type) => {
                // Unknown return type means void — do not default to u32
                let final_return = return_type;

                // resolve param types
                let resolved_params: Vec<(String, TypeKind)> = params
                    .into_iter()
                    .map(|(pname, pkind)| {
                        let resolved = if pkind == TypeKind::Unknown {
                            self.warn(&format!(
                                "parameter '{}' in fn '{}' has no type, defaulting to u32",
                                pname, name
                            ));
                            TypeKind::U32
                        } else {
                            pkind
                        };
                        (pname, resolved)
                    })
                    .collect();

                let mut child_env = self.env.child();
                for (pname, pkind) in &resolved_params {
                    child_env.set_var(pname, pkind.clone());
                }

                let saved_env = std::mem::replace(&mut self.env, child_env);
                let saved_func = self.current_func.replace(name.clone());

                let inferred_body: Vec<Statement> = body
                    .into_iter()
                    .map(|s| self.infer_stmt(s))
                    .collect();

                self.env = saved_env;
                self.current_func = saved_func;

                Statement::FunctionDefine(name, resolved_params, inferred_body, final_return)
            }

            Statement::Return(maybe_expr) => {
                let inferred = maybe_expr.map(|e| {
                    let ie = self.infer_expr(e);

                    if let Some(func_name) = &self.current_func.clone() {
                        if let Some(sig) = self.env.get_func(func_name).cloned() {
                            let ret_type = self.type_of_expr(&ie);
                            if sig.return_type != TypeKind::Unknown
                                && ret_type != TypeKind::Unknown
                                && !self.types_compatible(&ret_type, &sig.return_type)
                            {
                                self.error(&format!(
                                    "fn '{}' return type is '{}' but got '{}'",
                                    func_name,
                                    sig.return_type.name(),
                                    ret_type.name()
                                ));
                            }
                        }
                    }

                    ie
                });
                Statement::Return(inferred)
            }

            Statement::If(cond, then_body, else_body) => {
                let inferred_cond = self.infer_expr(cond);
                let inferred_then: Vec<Statement> = then_body
                    .into_iter()
                    .map(|s| self.infer_stmt(s))
                    .collect();
                let inferred_else = else_body.map(|stmts| {
                    stmts.into_iter().map(|s| self.infer_stmt(s)).collect()
                });
                Statement::If(inferred_cond, inferred_then, inferred_else)
            }

            Statement::While(cond, body) => {
                let inferred_cond = self.infer_expr(cond);
                let inferred_body: Vec<Statement> = body
                    .into_iter()
                    .map(|s| self.infer_stmt(s))
                    .collect();
                Statement::While(inferred_cond, inferred_body)
            }

            Statement::Loop(body) => {
                let inferred_body: Vec<Statement> = body
                    .into_iter()
                    .map(|s| self.infer_stmt(s))
                    .collect();
                Statement::Loop(inferred_body)
            }

            Statement::Call(name, args) => {
                let inferred_args = self.infer_call_args(&name, args);
                Statement::Call(name, inferred_args)
            }

            Statement::Poke(addr, val) => {
                Statement::Poke(self.infer_expr(addr), self.infer_expr(val))
            }

            Statement::Outb(port, val) => {
                Statement::Outb(self.infer_expr(port), self.infer_expr(val))
            }

            Statement::ArrayDefine(name, vals, kind) => {
                let final_kind = if kind == TypeKind::Unknown {
                    self.warn(&format!(
                        "array '{}' has no element type, defaulting to u32", name
                    ));
                    TypeKind::U32
                } else {
                    kind.clone()
                };

                for (i, &v) in vals.iter().enumerate() {
                    if v > final_kind.max_value() {
                        self.error(&format!(
                            "array '{}' element [{}] = {} exceeds max for '{}' (max: {})",
                            name, i, v, final_kind.name(), final_kind.max_value()
                        ));
                    }
                }

                self.env.set_var(&name, final_kind.clone());
                Statement::ArrayDefine(name, vals, final_kind)
            }

            Statement::ArrayAssign(name, idx, val) => {
                Statement::ArrayAssign(
                    name,
                    self.infer_expr(idx),
                    self.infer_expr(val),
                )
            }

            Statement::StructDefine(_, _) => stmt,
            Statement::StructInstance(_, _) => stmt,
            Statement::Bnw(_) => stmt,
            other => other,
        }
    }


    fn infer_expr(&mut self, expr: Expression) -> Expression {
        match expr {

            Expression::Number(n, kind) => {
                let resolved = if kind == TypeKind::Unknown {
                    infer_number_type(n)
                } else {
                    if n > kind.max_value() {
                        self.error(&format!(
                            "value {} exceeds max value for type '{}' (max: {})",
                            n, kind.name(), kind.max_value()
                        ));
                    }
                    kind
                };
                Expression::Number(n, resolved)
            }

            Expression::Variable(name) => {
                Expression::Variable(name)
            }
            Expression::AddressOf(name) => {
                Expression::AddressOf(name)
            }
            Expression::Syscall(args) => {
                let inferred: Vec<_> = args.into_iter().map(|a| self.infer_expr(a)).collect();
                Expression::Syscall(inferred)
            }

            Expression::BinaryOp(left, op, right) => {
                let inferred_left  = self.infer_expr(*left);
                let inferred_right = self.infer_expr(*right);
                let left_type  = self.type_of_expr(&inferred_left);
                let right_type = self.type_of_expr(&inferred_right);

                if left_type != TypeKind::Unknown
                    && right_type != TypeKind::Unknown
                    && left_type != right_type
                {
                    self.warn(&format!(
                        "operation '{}' between '{}' and '{}' — types differ",
                        op, left_type.name(), right_type.name()
                    ));
                }

                Expression::BinaryOp(
                    Box::new(inferred_left),
                    op,
                    Box::new(inferred_right),
                )
            }

            Expression::Call(name, args) => {
                let inferred_args = self.infer_call_args(&name, args);
                Expression::Call(name, inferred_args)
            }

            Expression::Peek(addr) => {
                Expression::Peek(Box::new(self.infer_expr(*addr)))
            }

            Expression::Inb(port) => {
                Expression::Inb(Box::new(self.infer_expr(*port)))
            }

            Expression::ArrayAccess(name, idx) => {
                Expression::ArrayAccess(name, Box::new(self.infer_expr(*idx)))
            }

            Expression::WaitKey => Expression::WaitKey,

            Expression::FieldAccess(var, field) => {
                Expression::FieldAccess(var, field)
            }

            Expression::FieldAssign(var, field, val) => {
                Expression::FieldAssign(var, field, Box::new(self.infer_expr(*val)))
            }
        }
    }


    fn infer_call_args(&mut self, name: &str, args: Vec<Expression>) -> Vec<Expression> {
        let inferred: Vec<Expression> = args
            .into_iter()
            .map(|a| self.infer_expr(a))
            .collect();

        if let Some(sig) = self.env.get_func(name).cloned() {
            if inferred.len() != sig.params.len() {
                self.error(&format!(
                    "fn '{}' expects {} argument(s) but got {}",
                    name, sig.params.len(), inferred.len()
                ));
            } else {
                for (i, (arg, expected)) in inferred.iter().zip(sig.params.iter()).enumerate() {
                    let got = self.type_of_expr(arg);
                    if got != TypeKind::Unknown
                        && *expected != TypeKind::Unknown
                        && !self.types_compatible(&got, expected)
                    {
                        self.error(&format!(
                            "fn '{}' argument {} expects '{}' but got '{}'",
                            name, i + 1, expected.name(), got.name()
                        ));
                    }
                }
            }
        }

        inferred
    }


    fn type_of_expr(&self, expr: &Expression) -> TypeKind {
        match expr {
            Expression::Number(_, kind)     => kind.clone(),
            Expression::Variable(name)      => {
                self.env.get_var(name).cloned().unwrap_or(TypeKind::Unknown)
            }
            Expression::BinaryOp(left, op, right) => {
                match op.as_str() {
                    "==" | "!=" | ">" | "<" | ">=" | "<=" => TypeKind::Bool,
                    _ => {
                        let lt = self.type_of_expr(left);
                        if lt != TypeKind::Unknown { lt }
                        else { self.type_of_expr(right) }
                    }
                }
            }
            Expression::AddressOf(_) => TypeKind::U32,
            Expression::Peek(_)             => TypeKind::Unknown,
            Expression::Inb(_)              => TypeKind::U8,
            Expression::Call(name, _)       => {
                self.env.get_func(name)
                    .map(|sig| sig.return_type.clone())
                    .unwrap_or(TypeKind::U32)
            }
            Expression::ArrayAccess(name, _) => {
                self.env.get_var(name).cloned().unwrap_or(TypeKind::U32)
            }
            _ => TypeKind::Unknown,
        }
    }


    fn types_compatible(&self, from: &TypeKind, to: &TypeKind) -> bool {
        if from == to                           { return true; }
        if *from == TypeKind::Unknown
        || *to   == TypeKind::Unknown           { return true; }

        match (from, to) {
       
            (TypeKind::U8,  TypeKind::U16) |
            (TypeKind::U8,  TypeKind::U32) |
            (TypeKind::U8,  TypeKind::U64) |
            (TypeKind::U16, TypeKind::U32) |
            (TypeKind::U16, TypeKind::U64) |
            (TypeKind::U32, TypeKind::U64) => true,
      
            (TypeKind::I8,  TypeKind::I16) |
            (TypeKind::I8,  TypeKind::I32) |
            (TypeKind::I8,  TypeKind::I64) |
            (TypeKind::I16, TypeKind::I32) |
            (TypeKind::I16, TypeKind::I64) |
            (TypeKind::I32, TypeKind::I64) => true,
    
            (TypeKind::Bool, TypeKind::U8)  |
            (TypeKind::Bool, TypeKind::U32) => true,
            _ => false,
        }
    }


    fn check_assignable(&mut self, from: &TypeKind, to: &TypeKind, context: &str) {
        if !self.types_compatible(from, to) {
            self.error(&format!(
                "type mismatch for '{}': cannot use '{}' as '{}'",
                context, from.name(), to.name()
            ));
        }
    }


    fn warn(&mut self, msg: &str) {
        self.warning_count += 1;
        eprintln!("[TYPE WARNING] {}", msg);
    }

    fn error(&mut self, msg: &str) {
        self.error_count += 1;
        eprintln!("[TYPE ERROR] {}", msg);
        std::process::exit(1);
    }

    fn report_summary(&self) {
        eprintln!("-------------------------------------------");
        if self.warning_count > 0 {
            eprintln!("[TYPE] {} warning(s)", self.warning_count);
        }
        if self.error_count == 0 {
            eprintln!("[TYPE] All types resolved — OK");
        }
        eprintln!("-------------------------------------------");
    }
}


fn infer_number_type(_n: u64) -> TypeKind {
    TypeKind::Unknown
}