from amaranth import *

# is this how you do constants??
EXPONENT_WIDTH = 8
EXPLICIT_MANTISSA_WIDTH = 23
IMPLICIT_MANTISSA_POSITION = EXPLICIT_MANTISSA_WIDTH + 1
NORMALIZED_POINT_POSITION = 30
EXPONENT_BIAS = 127

# i wonder if this is proper class naming convention
class FloatUnpacker(Elaboratable):
    def __init__(self):
        self.i = Signal(32)

        # can probably reduce exponent width to 8... i think, maybe
        self.o_sign = Signal(1)
        self.o_exponent = Signal(signed(10))
        self.o_mantissa = Signal(31)
    
    def elaborate(self, platform):
        m = Module()

        exp_raw = self.i.bit_select(EXPLICIT_MANTISSA_WIDTH, EXPONENT_WIDTH)
        m.d.comb += [
            self.o_sign.eq(self.i[-1]),
            self.o_exponent.eq(exp_raw - C(EXPONENT_BIAS, signed(8)))
        ]

        with m.If(exp_raw == 0):
            m.d.comb += self.o_mantissa.eq(0)
        with m.Else():
            m.d.comb += self.o_mantissa.eq((self.i[:EXPLICIT_MANTISSA_WIDTH] | (1 << IMPLICIT_MANTISSA_POSITION - 1)) << (NORMALIZED_POINT_POSITION - EXPLICIT_MANTISSA_WIDTH))

        return m            

class FloatPacker(Elaboratable):
    def __init__(self):
        self.i_sign = Signal(1)
        self.i_exponent = Signal(signed(10))
        self.i_mantissa = Signal(31)

        self.o = Signal(32)
    
    def elaborate(self, platform):
        m = Module()

        biased_exp = self.i_exponent + EXPONENT_BIAS

        ERROR_WIDTH = NORMALIZED_POINT_POSITION - EXPLICIT_MANTISSA_WIDTH
        HALF_ERROR = 1 << (ERROR_WIDTH - 1)

        rounded_mantissa = self.i_mantissa >> ERROR_WIDTH
        error = self.i_mantissa[:ERROR_WIDTH]

        round_up = (error > HALF_ERROR) | (
            (error == HALF_ERROR) & rounded_mantissa[0])
        rounded_mantissa = Mux(round_up, rounded_mantissa + 1, rounded_mantissa)
        
        # is this actually better than just using muxes? i'm not sure
        final_mantissa = Signal(30)
        final_exponent = Signal(signed(11))
        with m.If(rounded_mantissa[IMPLICIT_MANTISSA_POSITION] != 0):
            m.d.comb += [
                final_mantissa.eq(rounded_mantissa >> 1),
                final_exponent.eq(biased_exp + 1)
            ]
        with m.Else():
            m.d.comb += [
                final_mantissa.eq(rounded_mantissa),
                final_exponent.eq(biased_exp)
            ]

        with m.If(final_exponent < 0):
            m.d.comb += self.o.eq(self.i_sign << 31)
        with m.Elif(final_exponent > ((1 << EXPONENT_WIDTH) - 1)):
            m.d.comb += self.o.eq(~(self.i_sign << 31))
        with m.Else():
            m.d.comb += self.o.eq(self.i_sign << 31
                       | final_exponent << EXPLICIT_MANTISSA_WIDTH
                       | final_mantissa[:EXPLICIT_MANTISSA_WIDTH])

        return m

class FloatReducer(Elaboratable):
    def __init__(self):
        self.i_sign = Signal(1)
        self.i_exponent = Signal(signed(10))
        self.i_mantissa_long = Signal(63)

        self.o_sign = Signal(1)
        self.o_exponent = Signal(signed(10))
        self.o_mantissa = Signal(31)

    def elaborate(self, platform):
        m = Module()

        with m.If(self.i_mantissa_long == 0):
            m.d.comb += [
                self.o_sign.eq(0),
                self.o_exponent.eq(-EXPONENT_BIAS),
                self.o_mantissa.eq(0)
            ]
        with m.Else():
            clz = Signal(range(63))
            for j in range(63):
                with m.If(self.i_mantissa_long[j]):
                    m.d.comb += clz.eq(63 - j)
            
            m.d.comb += [
                self.o_sign.eq(self.i_sign),
                self.o_exponent.eq(self.i_exponent - clz + 3),
                self.o_mantissa.eq(sticky_lsr(self.i_mantissa_long << clz, 63 - NORMALIZED_POINT_POSITION))
            ]

        return m

# i'll keep this one like this for now...
def sticky_lsr(x, shift_amount):
    return Mux(shift_amount == 0,
        x,
        Mux(shift_amount >= 63,
            x != 0,
            x >> shift_amount | ((x & ((1 << shift_amount) - 1)) != 0)
            )
        )


class FmaUnit(Elaboratable):
    def __init__(self):
        self.a = Signal(32)
        self.b = Signal(32)
        self.c = Signal(32)

        self.q = Signal(32)

    def elaborate(self, platform):
        m = Module()

        m.submodules.unpack_a = unpack_a = FloatUnpacker()
        m.submodules.unpack_b = unpack_b = FloatUnpacker()
        m.submodules.unpack_c = unpack_c = FloatUnpacker()
        m.submodules.pack = pack = FloatPacker()
        m.submodules.reduce = reduce = FloatReducer()

        m.d.comb += [
            unpack_a.i.eq(self.a),
            unpack_b.i.eq(self.b),
            unpack_c.i.eq(self.c),
        ]

        product_sign = unpack_a.o_sign != unpack_b.o_sign
        product_exponent = unpack_a.o_exponent + unpack_b.o_exponent
        product_mantissa_long = unpack_a.o_mantissa * unpack_b.o_mantissa

        product_overflowed = (product_mantissa_long >> (
            NORMALIZED_POINT_POSITION * 2 + 1)) != 0

        product_exponent = product_exponent + product_overflowed
        product_mantissa_long = product_mantissa_long >> product_overflowed

        m.d.comb += [
            pack.i_sign.eq(reduce.o_sign),
            pack.i_exponent.eq(reduce.o_exponent),
            pack.i_mantissa.eq(reduce.o_mantissa)
        ]

        with m.If(product_mantissa_long == 0):
            m.d.comb += [
                pack.i_sign.eq(unpack_c.o_sign),
                pack.i_exponent.eq(unpack_c.o_exponent),
                pack.i_mantissa.eq(unpack_c.o_mantissa),
            ]
        with m.Elif(unpack_c.o_mantissa == 0):
            m.d.comb += [
                reduce.i_sign.eq(product_sign),
                reduce.i_exponent.eq(product_exponent),
                reduce.i_mantissa_long.eq(product_mantissa_long)
            ]
        with m.Else():
            exp_diff = product_exponent - unpack_c.o_exponent
            addend_mantissa_long = unpack_c.o_mantissa << NORMALIZED_POINT_POSITION  # Q60
            with m.If(product_sign == unpack_c.o_sign):
                with m.If(exp_diff > 0):
                    result_mantissa_long = product_mantissa_long + sticky_lsr(addend_mantissa_long, (exp_diff).as_unsigned())
                    m.d.comb += [
                        reduce.i_sign.eq(product_sign),
                        reduce.i_exponent.eq(product_exponent),
                        reduce.i_mantissa_long.eq(result_mantissa_long)
                    ]
                with m.Else():
                    result_mantissa_long = addend_mantissa_long + sticky_lsr(product_mantissa_long, (-exp_diff).as_unsigned())
                    m.d.comb += [
                        reduce.i_sign.eq(unpack_c.o_sign),
                        reduce.i_exponent.eq(unpack_c.o_exponent),
                        reduce.i_mantissa_long.eq(result_mantissa_long)
                    ]
            with m.Else():
                with m.If((exp_diff > 0) | ((exp_diff == 0) & (product_mantissa_long > addend_mantissa_long))):
                    result_mantissa_long = product_mantissa_long - sticky_lsr(addend_mantissa_long, (exp_diff).as_unsigned())
                    m.d.comb += [
                        reduce.i_sign.eq(product_sign),
                        reduce.i_exponent.eq(product_exponent),
                        reduce.i_mantissa_long.eq(result_mantissa_long)
                    ]
                with m.Else():
                    result_mantissa_long = addend_mantissa_long - sticky_lsr(product_mantissa_long, (-exp_diff).as_unsigned())
                    m.d.comb += [
                        reduce.i_sign.eq(unpack_c.o_sign),
                        reduce.i_exponent.eq(unpack_c.o_exponent),
                        reduce.i_mantissa_long.eq(result_mantissa_long)
                    ]

        m.d.sync += self.q.eq(pack.o)

        return m


if __name__ == "__main__":
    unit = FmaUnit()
    ports = [
        unit.a, unit.b, unit.c, unit.q
    ]

# stolen from gpawoo 1, probably only used for synthesis, which i don't know how to do anyways
# may as well have it in case it becomes important
#   from amaranth.back import rtlil
#
#   with open("fma.il", "w") as f:
#       f.write(rtlil.convert(unit, ports=ports))

    from amaranth.sim import *

    def test():
        # one plus one
        yield unit.a.eq(0x3f800000)
        yield unit.b.eq(0x3f800000)
        yield unit.c.eq(0x3f800000)
        yield
        yield
        assert(yield unit.q == 0x40000000)

        # two plus one
        yield unit.a.eq(0x3f800000)
        yield unit.b.eq(0x40000000)
        yield unit.c.eq(0x3f800000)
        yield
        yield
        assert(yield unit.q == 0x40400000)

        # one plus two
        yield unit.a.eq(0x3f800000)
        yield unit.b.eq(0x3f800000)
        yield unit.c.eq(0x40000000)
        yield
        yield
        assert(yield unit.q == 0x40400000)

        # one minus one
        yield unit.a.eq(0x3f800000)
        yield unit.b.eq(0x3f800000)
        yield unit.c.eq(0xbf800000)
        yield
        yield
        assert(yield unit.q == 0x00000000)

        # two minus one
        yield unit.a.eq(0x3f800000)
        yield unit.b.eq(0x40000000)
        yield unit.c.eq(0xbf800000)
        yield
        yield
        assert(yield unit.q == 0x3f800000)

        # one minus two
        yield unit.a.eq(0x3f800000)
        yield unit.b.eq(0x3f800000)
        yield unit.c.eq(0xc0000000)
        yield
        yield
        assert(yield unit.q == 0xbf800000)

        # two times two
        yield unit.a.eq(0x40000000)
        yield unit.b.eq(0x40000000)
        yield unit.c.eq(0x00000000)
        yield
        yield
        assert(yield unit.q == 0x40800000)

        # two times three plus one
        yield unit.a.eq(0x40000000)
        yield unit.b.eq(0x40400000)
        yield unit.c.eq(0x3f800000)
        yield
        yield
        assert(yield unit.q == 0x40e00000)

        # RAND1
        yield unit.a.eq(0xF03479D1)
        yield unit.b.eq(0x8AEE42BF)
        yield unit.c.eq(0xBBA48E86)
        yield
        yield
        assert(yield unit.q == 0x38DA7217)

        # addition commutes
        for _ in range(100):
            from random import getrandbits
            x = getrandbits(32)
            y = getrandbits(32)

            yield unit.a.eq(x)
            yield unit.b.eq(0x3f800000)
            yield unit.c.eq(y)
            yield
            yield
            q1 = yield unit.q

            yield unit.a.eq(y)
            yield unit.b.eq(0x3f800000)
            yield unit.c.eq(x)
            yield
            yield
            q2 = yield unit.q

            yield unit.a.eq(0x3f800000)
            yield unit.b.eq(x)
            yield unit.c.eq(y)
            yield
            yield
            q3 = yield unit.q

            yield unit.a.eq(0x3f800000)
            yield unit.b.eq(y)
            yield unit.c.eq(x)
            yield
            yield
            q4 = yield unit.q

            assert(q1 == q2 == q3 == q4)

        with open('test_cases.txt', newline='') as test_cases:
            import csv
            reader = csv.reader(test_cases)
            for case in reader:
                yield unit.a.eq(int(case[0]))
                yield unit.b.eq(int(case[1]))
                yield unit.c.eq(int(case[2]))
                yield
                yield
                assert(yield unit.q == int(case[3]))

    sim = Simulator(unit)
    sim.add_clock(1e-9)
    sim.add_sync_process(test)
    with sim.write_vcd("test.vcd", "test.gtkw"):
        sim.run()