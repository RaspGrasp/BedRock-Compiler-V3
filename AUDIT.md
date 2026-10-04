# BedRock Compiler — Self-Hosting Ready Audit

**Package:** compiler only (no libraries, no apps)  
**Date:** post nested-call fix

## Fixes applied for self-hosting readiness
1. **Eager argument evaluation** — all call args lowered before any `Psh` (fixes nested `add(add(a,b),c)`)
2. **Call return preservation** — `RAX` saved to `[rbp-0x48]` around live-reg restore after `call`
3. **Psh scratch** — uses `R11` instead of `RAX` so pushes do not clobber return values

## Regression (x86 execute)
23/23 PASS including:
- nested multi-arg calls, expression-nested calls
- recursion-like chains, syscalls, arithmetic, control flow

## Still out of scope (not blockers for subset bootstrap)
- ARM backend stub
- Full optimizer maturity
- Dynamic allocation / GC
- Interactive stdin edge cases

## Verdict
**Ready for subset self-hosting phase** on Linux x86-64 host.
