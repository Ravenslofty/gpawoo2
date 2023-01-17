mod fma;

struct Shader {
    regs: [u32; 16],
}

// SIMD4
// SIMD lane require a FMA unit
// lanewise-disable
// lanewise-broadcast
// 32-bit address space
// crossbar to reduce registers

// VLIW 64-bit insn:
// SIMD4 floating point section for math
//   lanewise-disable: 4 bits
//   lane crossbar: 4x4 bits
// integer section for branches/memory
//   immediates/offsets: 16 bits
//   loads/stores into ABCD or EFGH
//   reciprocal estimation
//   inverse square root estimation
//   constant handling?

// opcode space:
// floating point unit:
//   - FMA; bit for negate intermediate; bit for negate output
//   - mov
//   - convert to int/float?

// integer unit:

// crossbar:
// OX OY OZ OW AX AY AZ AW BX BY BZ BW IA IB IC ID
// O[XYZW] are SIMD outputs
// [AB][XYZW] is accumulator
// I[ABCD] is integer unit
// each lane selects its input

fn main() {
    
}