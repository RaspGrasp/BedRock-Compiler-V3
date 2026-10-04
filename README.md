# BedRock Compiler V3 (brc3)

BedRock (`.br`) is a low-level systems programming language designed for bare-metal software development and operating system design. The BedRock Compiler V3 (`brc3`) compiles BedRock source code directly into optimized machine executables with zero runtime dependencies.

## Key Features

- Zero C-Runtime Dependency: Produces pure bare-metal machine code without standard library bloat (`crt0`).
- Multi-Target Backend: Supports x86_64, MIPS, RISC-V, ARM, and Intermediate Representation (IR).
- High Performance: Lightweight compilation pipeline delivering fast build times and ultra-compact binaries.
- Intermediate Representation: Built-in IR pipeline for optimization passes and cross-target architecture code generation.

## Compiler Usage

```text
USAGE:
    brc3 <file.br> [OPTIONS]

OPTIONS:
    --target <arch>     Output target: mips (default), riscv, x86, arm, ir
    --optimize <level>  Optimization level: 1, 2, 3
    --emit-ir           Print IR to stdout and exit (no binary)
    --bridge            Use legacy AST->MIPS path instead of IR->MIPS
    --help              Show help message

EXAMPLES:
    brc3 main.br --target x86
    brc3 kernel.br --target mips --optimize 3
    brc3 main.br --emit-ir
```

## Benchmarks (BRC3 vs GCC)

Benchmarked on Ubuntu x86_64 against GCC (`-Os` optimization flag). All tests compiled and passed natively.

### Compilation Speed (Avg Time in ms) - Lower is better

```text
BRC3  [████░░░░░░░░░░░░░░░░] 22.73 ms  (~5.2x faster)
GCC   [████████████████████] 120.39 ms
```

### Binary Size Footprint (Avg Size in Bytes) - Lower is better

```text
BRC3  [█                   ] 118 B     (~133x smaller)
GCC   [████████████████████] 15,782 B
```

### Detailed Breakdown

| Benchmark | BRC3 Time | GCC (-Os) Time | BRC3 Binary Size | GCC Binary Size | Status |
| :--- | :--- | :--- | :--- | :--- | :---: |
| `arrays.br` | **99.20 ms** | 328.36 ms | **80 B** | 15,776 B | PASS |
| `bitwise.br` | **1.74 ms** | 65.52 ms | **64 B** | 15,776 B | PASS |
| `fibonacci.br` | **7.51 ms** | 73.31 ms | **244 B** | 15,808 B | PASS |
| `loops.br` | **3.48 ms** | 70.75 ms | **116 B** | 15,776 B | PASS |
| `math.br` | **1.74 ms** | 64.01 ms | **88 B** | 15,776 B | PASS |
| **Average** | **22.73 ms** | **120.39 ms** | **118.4 B** | **15,782.4 B** | **100%** |

> BRC3 generates zero-runtime, bare-metal machine code with no C-runtime (`crt0`) bloat or external dependency linking, resulting in instant compilation passes and minimal binary footprints.
