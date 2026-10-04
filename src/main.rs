mod type_inference;
mod optimizer;
mod ast;
mod lexer;
mod parser;
mod codegen;
mod verifier;
mod ir;

use std::{env, fs, path::Path};


fn process_includes(
    content: String,
    base_path: &Path,
    included: &mut std::collections::HashSet<String>,
    stack: &mut Vec<String>,
) -> String {
    let mut final_code = String::new();
    for line in content.lines() {
        if line.trim().starts_with("include ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() > 1 {
                let file_name    = parts[1].replace("\"", "").replace(";", "");
                let include_path = base_path.join(&file_name);
                let canonical    = match include_path.canonicalize() {
                    Ok(p)  => p.to_string_lossy().to_string(),
                    Err(_) => include_path.to_string_lossy().to_string(),
                };
                if stack.contains(&canonical) {
                    let chain: Vec<String> = stack.iter()
                        .map(|p| Path::new(p).file_name()
                            .unwrap_or_default().to_string_lossy().to_string())
                        .collect();
                    eprintln!(
                        "[INCLUDE ERROR] Circular include!\n  chain: {} -> {}",
                        chain.join(" -> "), file_name
                    );
                    std::process::exit(1);
                }
                if included.contains(&canonical) { continue; }
                included.insert(canonical.clone());
                stack.push(canonical.clone());
                let include_content = match fs::read_to_string(&include_path) {
                    Ok(c)  => c,
                    Err(_) => {
                        eprintln!(
                            "[INCLUDE ERROR] File not found: '{}'\n  looked in: {}",
                            file_name, include_path.display()
                        );
                        std::process::exit(1);
                    }
                };
                final_code.push_str(&process_includes(
                    include_content,
                    include_path.parent().unwrap_or(base_path),
                    included, stack,
                ));
                final_code.push('\n');
                stack.pop();
            }
        } else {
            final_code.push_str(line);
            final_code.push('\n');
        }
    }
    final_code
}


fn print_help() {
    eprintln!(
r#"BedRock Compiler

USAGE:
    bedrock <file.br> [OPTIONS]

OPTIONS:
    --target <arch>     Output target: mips (default), riscv, x86, arm, ir
    --optimize <level>  Optimization level: 1, 2, 3
    --emit-ir           Print IR to stdout and exit (no binary)
    --bridge            Use legacy AST→MIPS path instead of IR→MIPS
    --help              Show this message

EXAMPLES:
    bedrock os.br                          # compile → MIPS binary (IR pipeline)
    bedrock os.br --target ir              # emit IR text file
    bedrock os.br --target mips            # explicit MIPS (same as default)
    bedrock os.br --target riscv           # RISC-V (RV32I+M) binary
    bedrock os.br --target x86             # x86-64 Linux ELF executable
    bedrock os.br --bridge --target mips   # legacy path (AST direct)
    bedrock os.br --emit-ir                # print IR to stdout
    bedrock os.br --optimize 3 --target mips
"#
    );
}


fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 || args.contains(&"--help".to_string()) {
        print_help();
        return;
    }

    //Flags
    let source_path = Path::new(&args[1]);

    let target_str = args.iter()
        .position(|a| a == "--target")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("mips");

    let target = codegen::Target::from_str(target_str);


    let use_bridge = args.contains(&"--bridge".to_string());


    let emit_ir = args.contains(&"--emit-ir".to_string());

    let opt_level: u8 = args.iter()
        .position(|a| a == "--optimize")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    if use_bridge && target != codegen::Target::Mips && target != codegen::Target::MipsLe {
        eprintln!("[ERROR] --bridge is only available with --target mips");
        std::process::exit(1);
    }


    let source_code = match fs::read_to_string(source_path) {
        Ok(c) => c,
        Err(_) => {
            eprintln!(
                "[ERROR] Source file not found: '{}'\n  looked in: {}",
                args[1], source_path.display()
            );
            std::process::exit(1);
        }
    };


    let base_path = source_path.parent().unwrap_or(Path::new("."));
    let mut included = std::collections::HashSet::new();
    let mut stack    = Vec::new();
    let processed    = process_includes(source_code, base_path, &mut included, &mut stack);

    let mut lexer  = lexer::Lexer::new(&processed);
    let mut tokens = Vec::new();
    let mut locs: Vec<(usize, usize)> = Vec::new();
    loop {
        let line = lexer.line();
        let col  = lexer.col();
        let tok = lexer.next_token();
        if tok == lexer::Token::EOF { break; }
        tokens.push(tok);
        locs.push((line, col));
    }

    let mut parser  = parser::Parser::new_with_locs(tokens, locs);
    let program     = parser.parse_program();


    let mut inferencer = type_inference::TypeInferencer::new();
    let program        = inferencer.run(program);
    let mut ver        = verifier::Verifier::new();
    ver.run(&program);


    let program = match opt_level {
        1 => optimizer::pruner::Pruner::new().run(program),
        2 => {
            let p = optimizer::pruner::Pruner::new().run(program);
            optimizer::transformer::Transformer::new().run(p)
        }
        3 => optimizer::Optimizer::new().run(program),
        _ => program,
    };


    if use_bridge {
        eprintln!("[INFO] Mode: BRIDGE (AST → MIPS direct)");
        let mut cg     = codegen::mips::LegacyCodegen::new();
        let binary     = cg.compile(&program);
        let out_path   = source_path.with_extension("bin");
        fs::write(&out_path, &binary).expect("[ERROR] Write failed");
        eprintln!("[OK] Written: {}", out_path.display());

        return;
    }


    eprintln!("[INFO] Mode: IR pipeline  target={}", target.name());

    let mut ir_builder = ir::builder::IrBuilder::new();
    let ir_module      = ir_builder.build(program);


    if emit_ir {
        ir_module.dump();
        return;
    }

 
    let mut backend = codegen::select_backend(&target);
    let binary      = backend.compile(&ir_module);

    let out_ext  = target.output_extension();
    let out_path = source_path.with_extension(out_ext);
    fs::write(&out_path, &binary).expect("[ERROR] Write failed");
    eprintln!("[OK] Written: {}", out_path.display());


    let _map = backend.get_source_map();
}
