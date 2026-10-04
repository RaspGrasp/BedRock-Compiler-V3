pub mod pruner;
pub mod transformer;
pub mod quantum;

use crate::ast::Statement;

pub struct Optimizer {
    pub passes: u8,
}

impl Optimizer {
    pub fn new() -> Self {
        Optimizer { passes: 3 }
    }

    pub fn run(&self, program: Vec<Statement>) -> Vec<Statement> {
        let program = pruner::Pruner::new().run(program);
        let program = transformer::Transformer::new().run(program);
        let program = quantum::Quantum::new().run(program);
        program
    }
}
