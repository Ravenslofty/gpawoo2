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

/// q = (a * b) + c

#[derive(Default)]
struct FmaUnit {
    a: u32,
    b: u32,
    c: u32,
    q: u32
}

struct UnpackedFloat {
    sign: bool,
    exponent: i32,
    mantissa: u32 // Q30
}

impl UnpackedFloat {
    const EXPONENT_WIDTH: u32 = 8;
    const EXPLICIT_MANTISSA_WIDTH: u32 = 23;
    const IMPLICIT_MANTISSA_POSITION: u32 = Self::EXPLICIT_MANTISSA_WIDTH + 1;
    const NORMALIZED_POINT_POSITION: u32 = 30; 
    const EXPONENT_BIAS: i32 = 127;

    pub fn unpack(x: u32) -> UnpackedFloat {
        let sign = (x >> 31) != 0;
        let exp_raw = (x >> Self::EXPLICIT_MANTISSA_WIDTH) & ((1 << Self::EXPONENT_WIDTH) - 1);
        let frac_raw = x & ((1 << Self::EXPLICIT_MANTISSA_WIDTH) - 1);

        if exp_raw == 0 {
            UnpackedFloat{sign, exponent: -Self::EXPONENT_BIAS, mantissa: 0}
        } else {
            let exponent = (exp_raw as i32) - Self::EXPONENT_BIAS;
            let mantissa = (frac_raw | (1 << (Self::IMPLICIT_MANTISSA_POSITION - 1))) << (Self::NORMALIZED_POINT_POSITION - Self::EXPLICIT_MANTISSA_WIDTH);

            UnpackedFloat{sign, exponent, mantissa}
        }
    }

    pub fn pack(&self) -> u32 {
        let mut biased_exp = self.exponent + Self::EXPONENT_BIAS;

        const ERROR_WIDTH: u32 = UnpackedFloat::NORMALIZED_POINT_POSITION - UnpackedFloat::EXPLICIT_MANTISSA_WIDTH;
        const HALF_ERROR: u32 = 1 << (ERROR_WIDTH - 1);

        // self.mantissa -> 01mmmmmmmmmmmmmmmmmmmmmmxxxxxxxx

        let mut rounded_mantissa = self.mantissa >> ERROR_WIDTH;
        let error = self.mantissa & ((1 << ERROR_WIDTH) - 1);
        let round_up = error > HALF_ERROR || (error == HALF_ERROR && (rounded_mantissa & 1) == 1);
        if round_up {
            rounded_mantissa += 1;
            if (rounded_mantissa >> Self::IMPLICIT_MANTISSA_POSITION) != 0 {
                rounded_mantissa >>= 1;
                biased_exp += 1;
            }
        }

        if biased_exp <= 0 {
            if self.sign { 0x80000000 } else { 0 }
        } else if biased_exp > ((1 << Self::EXPONENT_WIDTH) - 1) {
            if self.sign { 0xffffffff } else { 0x7fffffff }
        } else {
            (if self.sign { 0x80000000 } else { 0 }) | (biased_exp as u32) << Self::EXPLICIT_MANTISSA_WIDTH | (rounded_mantissa & ((1 << Self::EXPLICIT_MANTISSA_WIDTH) - 1))
        }
    }

    fn reduce_mantissa(sign: bool, exponent: i32, mut mantissa_long: u64) -> UnpackedFloat {
        if mantissa_long == 0 {
            UnpackedFloat{sign: false, exponent: -Self::EXPONENT_BIAS, mantissa: 0}
        } else {
            const EXPECTED_SHIFT: i32 = 64 - ((UnpackedFloat::NORMALIZED_POINT_POSITION as i32) * 2 + 1);
            let mantissa_shift = mantissa_long.leading_zeros() as i32;
            mantissa_long <<= mantissa_shift; // Q63
            let mantissa: u32 = Self::sticky_lsr(mantissa_long, 63 - Self::NORMALIZED_POINT_POSITION as i32) as u32;
            UnpackedFloat{sign, exponent: exponent + (EXPECTED_SHIFT - mantissa_shift), mantissa}
        }
    }

    fn sticky_lsr(x: u64, shift_amount: i32) -> u64 {
        if shift_amount == 0 {
            x
        } else if shift_amount >= 64 {
            (x != 0) as u64
        } else {
            (x >> shift_amount) | ((x & ((1 << shift_amount) - 1) != 0) as u64)
        }
    }

    pub fn fmadd(op1: UnpackedFloat, op2: UnpackedFloat, addend: UnpackedFloat) -> UnpackedFloat {
        let product_sign = op1.sign != op2.sign;
        let mut product_exponent = op1.exponent + op2.exponent;
        let mut product_mantissa_long = (op1.mantissa as u64) * (op2.mantissa as u64); // Q60

        if (product_mantissa_long >> (Self::NORMALIZED_POINT_POSITION * 2 + 1)) != 0 {
            product_mantissa_long >>= 1;
            product_exponent += 1;
        }

        if product_mantissa_long == 0 {
            return addend;
        }

        if addend.mantissa == 0 {
            return Self::reduce_mantissa(product_sign, product_exponent, product_mantissa_long);
        }

        let exp_diff = product_exponent - addend.exponent;
        let addend_mantissa_long = (addend.mantissa as u64) << (Self::NORMALIZED_POINT_POSITION as u64); // Q60

        if addend.sign == product_sign {
            // Add
            if exp_diff > 0 {
                // product > addend
                let result_mantissa_long = product_mantissa_long + Self::sticky_lsr(addend_mantissa_long, exp_diff);
                Self::reduce_mantissa(product_sign, product_exponent, result_mantissa_long)
            } else {
                let result_mantissa_long = addend_mantissa_long + Self::sticky_lsr(product_mantissa_long, -exp_diff);
                Self::reduce_mantissa(addend.sign, addend.exponent, result_mantissa_long)
            }
        } else {
            // Sub
            if exp_diff > 0 || (exp_diff == 0 && product_mantissa_long > addend_mantissa_long) {
                let result_mantissa_long = product_mantissa_long - Self::sticky_lsr(addend_mantissa_long, exp_diff);
                Self::reduce_mantissa(product_sign, product_exponent, result_mantissa_long)
            } else {
                let result_mantissa_long = addend_mantissa_long - Self::sticky_lsr(product_mantissa_long, -exp_diff);
                Self::reduce_mantissa(addend.sign, addend.exponent, result_mantissa_long)
            }
        }
    }
}

impl FmaUnit {
    /// Returns last cycle's Q
    fn step(&mut self, a: u32, b: u32, c: u32) -> u32 {
        let unpacked_a = UnpackedFloat::unpack(self.a);
        let unpacked_b = UnpackedFloat::unpack(self.b);
        let unpacked_c = UnpackedFloat::unpack(self.c);

        self.a = a;
        self.b = b;
        self.c = c;
        UnpackedFloat::fmadd(unpacked_a, unpacked_b, unpacked_c).pack()
    }
}

#[cfg(test)]
mod tests {
    use crate::FmaUnit;

    #[test]
    fn fmadd_one_plus_one() {
        let mut fma = FmaUnit{
            a: 0x3f800000,
            b: 0x3f800000,
            c: 0x3f800000,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0x40000000);
    }

    #[test]
    fn fmadd_two_plus_one() {
        let mut fma = FmaUnit{
            a: 0x3f800000,
            b: 0x40000000,
            c: 0x3f800000,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0x40400000);
    }

    #[test]
    fn fmadd_one_plus_two() {
        let mut fma = FmaUnit{
            a: 0x3f800000,
            b: 0x3f800000,
            c: 0x40000000,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0x40400000);
    }

    #[test]
    fn fmadd_one_minus_one() {
        let mut fma = FmaUnit{
            a: 0x3f800000,
            b: 0x3f800000,
            c: 0xbf800000,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0x00000000);
    }

    #[test]
    fn fmadd_two_minus_one() {
        let mut fma = FmaUnit{
            a: 0x3f800000,
            b: 0x40000000,
            c: 0xbf800000,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0x3f800000);
    }

    #[test]
    fn fmadd_one_minus_two() {
        let mut fma = FmaUnit{
            a: 0x3f800000,
            b: 0x3f800000,
            c: 0xc0000000,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0xbf800000);
    }

    #[test]
    fn fmadd_two_times_two() {
        let mut fma = FmaUnit{
            a: 0x40000000,
            b: 0x40000000,
            c: 0,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0x40800000);
    }

    #[test]
    fn fmadd_two_times_three_plus_one() {
        let mut fma = FmaUnit{
            a: 0x40000000,
            b: 0x40400000,
            c: 0x3f800000,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0x40e00000);
    }

    #[test]
    fn rand1() {
        let mut fma = FmaUnit{
            a: 0xF03479D1,
            b: 0x8AEE42BF,
            c: 0xBBA48E86,
            q: 0,
        };
        assert_eq!(fma.step(0, 0, 0), 0x38DA7217);
    }

    #[test]
    fn addition_commutes() {
        for _ in 0..100 { 
            let x = rand::random::<u32>();
            let y = rand::random::<u32>();

            let q1 = FmaUnit{
                a: x,
                b: 0x3f800000,
                c: y,
                q: 0,
            }.step(0, 0, 0);

            let q2 = FmaUnit{
                a: y,
                b: 0x3f800000,
                c: x,
                q: 0,
            }.step(0, 0, 0);

            let q3 = FmaUnit{
                a: 0x3f800000,
                b: x,
                c: y,
                q: 0,
            }.step(0, 0, 0);

            let q4 = FmaUnit{
                a: 0x3f800000,
                b: y,
                c: x,
                q: 0,
            }.step(0, 0, 0);

            assert_eq!(q1, q2);
            assert_eq!(q1, q3);
            assert_eq!(q1, q4);
        }
    }

    #[test]
    fn random_tests() {
        for _ in 0..10000000 {
            let x = rand::random::<u32>();
            let y = rand::random::<u32>();
            let z = rand::random::<u32>();

            let fx = f32::from_bits(x);
            let fy = f32::from_bits(y);
            let fz = f32::from_bits(z);

            if fx.is_nan() || fx.is_infinite() || fx.is_subnormal() { continue }
            if fy.is_nan() || fy.is_infinite() || fy.is_subnormal() { continue }
            if fz.is_nan() || fz.is_infinite() || fz.is_subnormal() { continue }

            let result = fx.mul_add(fy, fz);

            if result.is_nan() || result.is_infinite() || result.is_subnormal() { continue }

            let q = FmaUnit{
                a: x,
                b: y,
                c: z,
                q: 0,
            }.step(0, 0, 0);

            assert_eq!(result.to_bits(), q)
        }
    }
}

fn main() {
    
}