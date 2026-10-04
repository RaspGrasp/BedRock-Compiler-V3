# BRISC32 RTL (v0.7)

CPU متعدد الـ cycles (من غير pipeline) بيطبّق **كل** الـ ISA الموصوفة في `BRISC32_SPEC.md`،
مع الاستثناءات الدقيقة (precise exceptions) والـ interrupts. اتختبر على المحاكي (Icarus Verilog) بس،
ولسه **مفيش synthesis ولا FPGA** (شوف "اللي لسه مش متأكد منه" تحت).

## الملفات

| ملف | وظيفته |
|---|---|
| `brisc32_pkg.sv` | الـ opcodes والـ conditions والـ vectors والـ ALU ops |
| `regfile.sv` | 16 × 32-bit، 3 قراءة + 1 كتابة |
| `alu.sv` | add/sub/adc/sbc، logic، shifts، mul/mulhu، flags N Z C V |
| `cpu.sv` | الـ state machine (fetch → exec → data phases) |

## الواجهة (`cpu`)

| مجموعة | إشارات | ملاحظات |
|---|---|---|
| ذاكرة | `mem_addr/re/we/wstrb/wdata/rdata/ready` | القراءة: الطلب بيتسجل، والبيانات بتتقري في الـ cycle اللي بعده طول ما `mem_ready=1`. الكتابة posted (بتتقبل دايماً). الـ **CPU بيظبط الـ byte lanes بنفسه** (الـ load بيستخرج ويعمل extend، والـ store بيكرر البيانات ويحدد `wstrb`) فالذاكرة word عادية بـ strobes |
| I/O | `io_addr/re/we/wdata/rdata/ready` | address space منفصل عن الذاكرة. `io_re`/`io_we` **بيفضلوا High** لحد `io_ready` (الجهاز يقدر يعمل stall لـ `in`/`out`، زي waitkey) |
| interrupts | `irq_req` (level)، `irq_vec[7:0]`، `irq_ack` (نبضة) | بيتقبل بس بين instructions وطالما `IE=1`. الجهاز لازم يفضل رافع `irq_req` لحد ما يشوف `irq_ack` |
| حالة | `halted`، `dbg_fault`، `dbg_pc`، `dbg_cycle` | `halted=1` في الـ halt النهائي أو WFI أو الـ fault |

## اللي اتغيّر في v0.7 (مقابل v0.6)

v0.6 كان بيعمل `halt` صامت على أي instruction ملهاش معالجة. دلوقتي:

| الميزة | قبل | بعد |
|---|---|---|
| `pushm/popm/stm/ldm` | المعالج بيقف | منفذين (cycle لكل register) |
| `divu/divs` | المعالج بيقف | divider restoring، 32 cycle |
| `trap` / `setvec` / illegal / div0 / misaligned | المعالج بيقف | منفذين بالكامل مع vector table |
| interrupts خارجية و`halt` كـ WFI | مفيش pin | `irq_req/irq_vec/irq_ack` |
| `ret` من interrupt frame | بيتجاهل الـ flags | بيرجّع الـ flags (و IE) |
| `ldb/ldh/stb/sth` على عنوان مش على lane 0 | بيقرا/بيكتب lane 0 دايماً (غلط) | الـ CPU بيوجّه الـ lane |
| `ldx.bs/.hs` | مفيش sign-extend | منفذ |
| `stx.b/.h` | بيكتب word كامل ويمسح اللي جنبه | بيكتب الحجم المطلوب بس |
| `in`/`out` | نافذة ذاكرة عند `0xFFFF0000` | address space منفصل + stall |
| `mrs`/`msr` على index غلط | بيتجاهل | illegal instruction (زي المحاكي) |
| الـ S bit على shifts | بيحدّث flags | بيتجاهل (زي المحاكي والـ spec) |
| `flags` | 32 bit بالكامل | بتات N Z C V IE بس |

## الفروق عن المحاكي المرجعي `brisc32.py` (v0.1)

الـ RTL بيطبّق شوية قرارات لسه مش في المحاكي. الاختبارات المتأثرة عليها قيم متوقعة مكتوبة يدوي (`v02_*` في `tests/diff_test.py`):

1. **الـ exceptions دقيقة (precise):** `push/pushm/cal/ret` اللي بتعمل fault (عنوان مش aligned) **مبتغيّرش `sp`/`rsp`**. المحاكي v0.1 بيغيّرهم قبل الـ fault.
2. **`pushm`/`popm` ممنوع فيهم `sp` في الـ list** → illegal instruction (vector 0).
3. **double fault:** لو الـ vector entry = 0 أو `vbr`/`rsp` مش aligned وقت الدخول للـ handler، الـ CPU بيقف و`dbg_fault=1`.
4. **`mrs cycle`** في الـ RTL بيعدّ clock cycles، وفي المحاكي بيعدّ instructions.
5. **`halt`** بيزوّد الـ `pc` (عشان الـ WFI يكمل بعده).
6. **الـ reset:** `pc=0`، `sp=0`، `rsp=0x00101000`، `vbr=0`. يعني الـ vector table الافتراضي (عند 0) **بيتداخل مع الكود** → اضبط `vbr` بـ `msr vbr, rX` قبل أي `setvec`.

## الاختبار

```bash
python3 tests/diff_test.py                              # 42 اختبار موجّه: نفس الـ binary على المحاكي وعلى الـ RTL
python3 tests/diff_test.py --rand 7 --io-delay 3        # + ذاكرة بتتأخر عشوائي (بترجّع DEADBEEF وهي مش جاهزة) + جهاز I/O بيعمل stall
python3 tests/diff_test.py --fuzz 300 --seed 1          # 300 برنامج عشوائي
make smoke                                              # الـ smoke test الأصلي (11 instruction)
```
محتاج `iverilog` (≥ 12). `sim/tb_prog.sv` هو الـ testbench العام (شوف التعليقات في أوله للـ plusargs).

### مدى قوة الاختبارات (mutation testing)
كسّرت الـ RTL عمداً بـ 14 طريقة (sign-extend، byte lane، `popm` مبيحرّكش `sp`، الـ IRQ بيتجاهل IE، `ret` مبيرجّعش flags، عنوان الـ exception frame، إشارة القسمة، فحص alignment للـ half، `pushm {sp}`، `cmp` مبيحدّثش V، الـ WFI مبيصحاش، `in/out` مش بيستنوا الجهاز، `setvec` بيضيّع البت 7). **الـ 14 اتكشفوا.** اتنين منهم ماتكشفوش في الأول وده كان ثغرة في الاختبارات نفسها (مفيش اختبار بـ vector ≥ 128، ومفيش I/O stall)، فضفت اختبار `trap_high_vectors` وجولة `--io-delay`.

## الأداء (قياس من الـ RTL)

| برنامج | instructions | cycles | CPI |
|---|---|---|---|
| hello | 126 | 419 | 3.33 |
| bubble_sort | 585 | 1865 | 3.19 |
| u64_fact | 180 | 542 | 3.01 |
| fib(20) | 262,693 | 1,028,881 | 3.92 |

الـ CPI حوالي 3–4 لأن الـ core multi-cycle من غير pipeline (fetch بياخد cycle ونص قبل الـ exec). مش مصمم للسرعة.

## اللي لسه مش متأكد منه

- **الـ synthesis:** ماجربتش أي synthesizer. الـ `regfile` بيقرا combinational، والـ `mul` combinational في الـ ALU (مسار طويل على FPGA صغير)، وفيه `for` loops في دوال (popcount/lowest-bit) المفروض تتحول لـ logic عادية لكن ماتأكدتش.
- **Verilator:** كل الاختبارات على Icarus 12. الـ Makefile القديم بيستخدم Verilator وماجربتوش.
- **التغطية:** الـ fuzz بيغطي 57 من 61 opcode (الناقص `jmp/cal/calr/jr`، ومغطيين في الاختبارات الموجّهة). مفيش تغطية للـ interrupts جوه الـ fuzz (بس في الاختبارات الموجّهة).
- **وقت وصول الـ interrupt** بين instructions متقارن بالمحاكي بس في سيناريوهات محددة (WFI، أو الطلب مرفوع من البداية)، مش في أي لحظة عشوائية.
