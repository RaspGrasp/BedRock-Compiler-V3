use crate::ast::{Statement, Expression};
use std::collections::{HashMap, HashSet, BTreeMap};

const MIPS_REGISTERS: usize = 18;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct VarId(String);

#[derive(Debug, Clone)]
struct LiveInterval {
    var: VarId,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
pub struct RegisterMap {
    pub assignments: HashMap<String, usize>,
    pub spilled: HashSet<String>,
}

impl RegisterMap {
    fn new() -> Self {
        RegisterMap {
            assignments: HashMap::new(),
            spilled: HashSet::new(),
        }
    }

    pub fn get_reg(&self, var: &str) -> Option<usize> {
        self.assignments.get(var).copied()
    }

    pub fn is_spilled(&self, var: &str) -> bool {
        self.spilled.contains(var)
    }
}

pub struct Quantum {
    counter: usize,
}

impl Quantum {
    pub fn new() -> Self {
        Quantum { counter: 0 }
    }

    pub fn run(&mut self, program: Vec<Statement>) -> Vec<Statement> {
        let intervals = self.compute_intervals(&program);
        let reg_map = self.allocate_registers(intervals);
        let program = self.schedule(program, &reg_map);
        program
    }

    pub fn get_register_map(&mut self, stmts: &[Statement]) -> RegisterMap {
        let intervals = self.compute_intervals(stmts);
        self.allocate_registers(intervals)
    }

    fn next_id(&mut self) -> usize {
        let id = self.counter;
        self.counter += 1;
        id
    }

    fn compute_intervals(&mut self, stmts: &[Statement]) -> Vec<LiveInterval> {
        let mut defs: HashMap<String, usize> = HashMap::new();
        let mut last_use: HashMap<String, usize> = HashMap::new();
        self.counter = 0;
        self.scan_stmts(stmts, &mut defs, &mut last_use);

        let mut intervals = Vec::new();
        for (var, start) in &defs {
            let end = last_use.get(var).copied().unwrap_or(*start);
            intervals.push(LiveInterval {
                var: VarId(var.clone()),
                start: *start,
                end,
            });
        }
        intervals.sort_by_key(|i| i.start);
        intervals
    }

    fn scan_stmts(
        &mut self,
        stmts: &[Statement],
        defs: &mut HashMap<String, usize>,
        uses: &mut HashMap<String, usize>,
    ) {
        for stmt in stmts {
            self.scan_stmt(stmt, defs, uses);
        }
    }

    fn scan_stmt(
        &mut self,
        stmt: &Statement,
        defs: &mut HashMap<String, usize>,
        uses: &mut HashMap<String, usize>,
    ) {
        let id = self.next_id();
        match stmt {
            Statement::Let(name, expr, _) | Statement::Root(name, expr, _) => {
                self.scan_expr(expr, id, uses);
                defs.entry(name.clone()).or_insert(id);
            }
            Statement::Assignment(name, expr) => {
                self.scan_expr(expr, id, uses);
                uses.insert(name.clone(), id);
            }
            Statement::ArrayAssign(name, idx, val) => {
                self.scan_expr(idx, id, uses);
                self.scan_expr(val, id, uses);
                uses.insert(name.clone(), id);
            }
            Statement::Return(Some(expr)) => {
                self.scan_expr(expr, id, uses);
            }
            Statement::If(cond, then_b, else_b) => {
                self.scan_expr(cond, id, uses);
                self.scan_stmts(then_b, defs, uses);
                if let Some(eb) = else_b {
                    self.scan_stmts(eb, defs, uses);
                }
            }
            Statement::While(cond, body) => {
                self.scan_expr(cond, id, uses);
                self.scan_stmts(body, defs, uses);
            }
            Statement::Loop(body) => {
                self.scan_stmts(body, defs, uses);
            }
            Statement::FunctionDefine(_, params, body, _) => {
                for (pname, _) in params {
                    defs.entry(pname.clone()).or_insert(id);
                }
                self.scan_stmts(body, defs, uses);
            }
            Statement::Call(name, args) => {
                uses.insert(name.clone(), id);
                for a in args { self.scan_expr(a, id, uses); }
            }
            Statement::Outb(port, val) | Statement::Poke(port, val) => {
                self.scan_expr(port, id, uses);
                self.scan_expr(val, id, uses);
            }
            Statement::CallPtr(expr) => self.scan_expr(expr, id, uses),
            Statement::IntHandler(_, body) => {
    self.scan_stmts(body, defs, uses);
}
Statement::IntEnable(expr, _) => {
    self.scan_expr(expr, id, uses);
}
_ => {}
        }
    }

    fn scan_expr(&self, expr: &Expression, id: usize, uses: &mut HashMap<String, usize>) {
        match expr {
            Expression::Variable(name) => { uses.insert(name.clone(), id); }
            Expression::BinaryOp(l, _, r) => {
                self.scan_expr(l, id, uses);
                self.scan_expr(r, id, uses);
            }
            Expression::ArrayAccess(name, idx) => {
                uses.insert(name.clone(), id);
                self.scan_expr(idx, id, uses);
            }
            Expression::Call(name, args) => {
                uses.insert(name.clone(), id);
                for a in args { self.scan_expr(a, id, uses); }
            }
            Expression::Peek(addr) | Expression::Inb(addr) => {
                self.scan_expr(addr, id, uses);
            }
            Expression::FieldAccess(var, _) => { uses.insert(var.clone(), id); }
            Expression::FieldAssign(var, _, val) => {
                uses.insert(var.clone(), id);
                self.scan_expr(val, id, uses);
            }
            _ => {}
        }
    }

    fn allocate_registers(&self, intervals: Vec<LiveInterval>) -> RegisterMap {
        let mut reg_map = RegisterMap::new();
        let mut active: Vec<LiveInterval> = Vec::new();
        let mut free_regs: Vec<usize> = (0..MIPS_REGISTERS).collect();

        for interval in intervals {
            self.expire_old_intervals(&interval, &mut active, &mut free_regs);

            if free_regs.is_empty() {
                let spill = self.choose_spill(&mut active, &interval);
                if let Some(spilled_interval) = spill {
                    let reg = reg_map.assignments[&spilled_interval.var.0];
                    reg_map.spilled.insert(spilled_interval.var.0.clone());
                    reg_map.assignments.remove(&spilled_interval.var.0);
                    reg_map.assignments.insert(interval.var.0.clone(), reg);
                    active.retain(|a| a.var != spilled_interval.var);
                    active.push(interval);
                } else {
                    reg_map.spilled.insert(interval.var.0.clone());
                }
            } else {
                let reg = free_regs.remove(0);
                reg_map.assignments.insert(interval.var.0.clone(), reg);
                active.push(interval);
                active.sort_by_key(|i| i.end);
            }
        }

        reg_map
    }

    fn expire_old_intervals(
        &self,
        current: &LiveInterval,
        active: &mut Vec<LiveInterval>,
        free_regs: &mut Vec<usize>,
    ) {
        let expired: Vec<LiveInterval> = active
            .iter()
            .filter(|a| a.end < current.start)
            .cloned()
            .collect();

        for e in expired {
            active.retain(|a| a.var != e.var);
        }
    }

    fn choose_spill(
        &self,
        active: &mut Vec<LiveInterval>,
        current: &LiveInterval,
    ) -> Option<LiveInterval> {
        let spill = active.iter().max_by_key(|a| a.end).cloned();
        if let Some(ref s) = spill {
            if s.end > current.end {
                return spill;
            }
        }
        None
    }

    fn schedule(&self, program: Vec<Statement>, reg_map: &RegisterMap) -> Vec<Statement> {
        program
            .into_iter()
            .map(|s| self.schedule_stmt(s, reg_map))
            .collect()
    }

    fn schedule_stmt(&self, stmt: Statement, reg_map: &RegisterMap) -> Statement {
        match stmt {
            Statement::FunctionDefine(name, params, body, ret) => {
                let body = self.reorder_for_pipeline(body, reg_map);
                Statement::FunctionDefine(name, params, body, ret)
            }
            Statement::Loop(body) => {
                Statement::Loop(self.reorder_for_pipeline(body, reg_map))
            }
            Statement::While(cond, body) => {
                Statement::While(cond, self.reorder_for_pipeline(body, reg_map))
            }
            Statement::If(cond, then_b, else_b) => {
                let then_b = self.reorder_for_pipeline(then_b, reg_map);
                let else_b = else_b.map(|eb| self.reorder_for_pipeline(eb, reg_map));
                Statement::If(cond, then_b, else_b)
            }
            Statement::IntHandler(name, body) => {
    Statement::IntHandler(name, self.reorder_for_pipeline(body, reg_map))
}
other => other,
        }
    }

    fn reorder_for_pipeline(
        &self,
        stmts: Vec<Statement>,
        reg_map: &RegisterMap,
    ) -> Vec<Statement> {
        let mut independent: Vec<Statement> = Vec::new();
        let mut dependent: Vec<Statement> = Vec::new();
        let mut defined_so_far: HashSet<String> = HashSet::new();

        for stmt in stmts {
            let reads = self.stmt_reads(&stmt);
            let writes = self.stmt_writes(&stmt);

            let has_dep = reads.iter().any(|r| {
                reg_map.is_spilled(r)
            });

            let writes_spilled = writes.iter().any(|w| reg_map.is_spilled(w));
            let _ = defined_so_far;

            if has_dep || writes_spilled {
                dependent.push(stmt);
            } else {
                independent.push(stmt);
                for w in writes {
                    defined_so_far.insert(w);
                }
            }
        }

        let mut result = Vec::new();
        let mut dep_iter = dependent.into_iter().peekable();
        let mut ind_iter = independent.into_iter().peekable();

        loop {
            match (ind_iter.peek().is_some(), dep_iter.peek().is_some()) {
                (true, _) => { if let Some(s) = ind_iter.next() { result.push(s); } }
                (false, true) => { if let Some(s) = dep_iter.next() { result.push(s); } }
                (false, false) => break,
            }
        }

        result
    }

    fn stmt_reads(&self, stmt: &Statement) -> HashSet<String> {
        let mut set = HashSet::new();
        match stmt {
            Statement::Let(_, expr, _) | Statement::Root(_, expr, _) => {
                self.expr_vars(expr, &mut set);
            }
            Statement::Assignment(_, expr) => { self.expr_vars(expr, &mut set); }
            Statement::Return(Some(expr)) => { self.expr_vars(expr, &mut set); }
            Statement::Outb(p, v) | Statement::Poke(p, v) => {
                self.expr_vars(p, &mut set);
                self.expr_vars(v, &mut set);
            }
            Statement::ArrayAssign(name, idx, val) => {
                set.insert(name.clone());
                self.expr_vars(idx, &mut set);
                self.expr_vars(val, &mut set);
            }
            _ => {}
        }
        set
    }

    fn stmt_writes(&self, stmt: &Statement) -> HashSet<String> {
        let mut set = HashSet::new();
        match stmt {
            Statement::Let(name, _, _) | Statement::Root(name, _, _) => { set.insert(name.clone()); }
            Statement::Assignment(name, _) => { set.insert(name.clone()); }
            _ => {}
        }
        set
    }

    fn expr_vars(&self, expr: &Expression, set: &mut HashSet<String>) {
        match expr {
            Expression::Variable(name) => { set.insert(name.clone()); }
            Expression::BinaryOp(l, _, r) => {
                self.expr_vars(l, set);
                self.expr_vars(r, set);
            }
            Expression::ArrayAccess(name, idx) => {
                set.insert(name.clone());
                self.expr_vars(idx, set);
            }
            Expression::Peek(addr) | Expression::Inb(addr) => { self.expr_vars(addr, set); }
            Expression::Call(name, args) => {
                set.insert(name.clone());
                for a in args { self.expr_vars(a, set); }
            }
            Expression::FieldAccess(var, _) => { set.insert(var.clone()); }
            Expression::FieldAssign(var, _, val) => {
                set.insert(var.clone());
                self.expr_vars(val, set);
            }
            _ => {}
        }
    }

    pub fn interference_graph(&mut self, stmts: &[Statement]) -> BTreeMap<String, HashSet<String>> {
        let intervals = self.compute_intervals(stmts);
        let mut graph: BTreeMap<String, HashSet<String>> = BTreeMap::new();

        for i in 0..intervals.len() {
            for j in (i + 1)..intervals.len() {
                let a = &intervals[i];
                let b = &intervals[j];
                let overlaps = a.start <= b.end && b.start <= a.end;
                if overlaps {
                    graph
                        .entry(a.var.0.clone())
                        .or_default()
                        .insert(b.var.0.clone());
                    graph
                        .entry(b.var.0.clone())
                        .or_default()
                        .insert(a.var.0.clone());
                }
            }
        }

        graph
    }

    pub fn chromatic_bound(&mut self, stmts: &[Statement]) -> usize {
        let graph = self.interference_graph(stmts);
        graph.values().map(|neighbors| neighbors.len()).max().unwrap_or(0) + 1
    }
}
