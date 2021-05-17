//! An implementation of bigcomp for Rust.
//!
//! Compares the known string to theoretical digits generated on the
//! fly for `b+h`, where a string representation of a float is between
//! `b` and `b+u`, where `b+u` is 1 unit in the least-precision. Therefore,
//! the string must be close to `b+h`.
//!
//! Adapted from:
//!     https://www.exploringbinary.com/bigcomp-deciding-truncated-near-halfway-conversions/

use crate::lib::cmp;
use crate::util::*;
use super::alias::*;
use super::bignum::*;
use super::format::*;
use super::math::*;

// ROUNDING

/// Custom rounding for the ratio.
#[allow(unused_variables)]
pub(super) fn round_to_native<F>(f: F, order: cmp::Ordering, kind: RoundingKind)
    -> F
    where F: FloatType
{
    #[cfg(not(feature = "rounding"))] {
        match order {
            cmp::Ordering::Greater  => f.next_positive(),
            cmp::Ordering::Less     => f,
            cmp::Ordering::Equal    => f.round_positive_even(),
        }
    }

    // Compare the actual digits to the round-down or halfway point.
    #[cfg(feature = "rounding")] {
        match order {
            cmp::Ordering::Greater  => match kind {
                // Comparison with `b+h`, above. Round-up.
                RoundingKind::NearestTieEven     => f.next_positive(),
                RoundingKind::NearestTieAwayZero => f.next_positive(),
                // Comparison with `b`, above. Truncated digits.
                RoundingKind::Upward             => f.next_positive(),
                RoundingKind::Downward           => f,
                _                                => unimplemented!(),
            },
            // This cannot happen for RoundingKind Upward or Downward.
            // For round-nearest algorithms, we are below `b+h` so round-down.
            cmp::Ordering::Less     => match kind {
                // Comparison with `b+h`, below. Stay put.
                RoundingKind::NearestTieEven     => f,
                RoundingKind::NearestTieAwayZero => f,
                // Comparison with `b`, below. Truncated digits, but below our
                // estimate `b`.
                RoundingKind::Upward             => f,
                RoundingKind::Downward           => f.prev_positive(),
                _                                => unimplemented!(),
            },
            cmp::Ordering::Equal    => match kind {
                // Only round-up if the mantissa is odd.
                RoundingKind::NearestTieEven     => f.round_positive_even(),
                // Always round-up, we want to go away from 0.
                RoundingKind::NearestTieAwayZero => f.next_positive(),
                // Comparison with `b`, equal. No truncated digits.
                RoundingKind::Upward             => f,
                RoundingKind::Downward           => f,
                _                                => unimplemented!(),
            },
        }
    }
}

// SHARED

perftools_inline!{
/// Calculate `b` from a a representation of `b` as a float.
pub(super) fn b<F: FloatType>(f: F) -> F::ExtendedFloat {
    f.into()
}}

perftools_inline!{
/// Calculate `b+h` from a a representation of `b` as a float.
pub(super) fn bh<F: FloatType>(f: F) -> F::ExtendedFloat {
    // None of these can overflow.
    let mut b = b(f);
    let mant = (b.mant() << 1) + as_cast(1);
    let exp = b.exp() - 1;
    b.set_mant(mant);
    b.set_exp(exp);
    b
}}

perftools_inline!{
/// Generate the theoretical float type for the rounding kind.
#[allow(unused_variables)]
pub(super) fn theoretical_float<F>(f: F, kind: RoundingKind)
    -> F::ExtendedFloat
    where F: FloatType
{
    #[cfg(not(feature = "rounding"))] {
        bh(f)
    }

    #[cfg(feature = "rounding")] {
        match is_nearest(kind) {
            // We need to check if we're close to halfway, so use `b+h`.
            true  => bh(f),
            // Just care if there are any truncated digits, use `b`.
            false => b(f),
        }
    }
}}

// BIGCOMP

perftools_inline!{
/// Get the appropriate scaling factor from the digit count.
///
/// * `radix`           - Radix for the number parsing.
/// * `sci_exponent`    - Exponent of basen string in scientific notation.
pub fn scaling_factor(radix: u32, sci_exponent: u32)
    -> Bigfloat
{
    let mut factor = Bigfloat { data: arrvec![1], exp: 0 };
    factor.imul_power(radix, sci_exponent);
    factor
}}

/// Make a ratio for the numerator and denominator.
///
/// * `radix`           - Radix for the number parsing.
/// * `sci_exponent`    - Exponent of basen string in scientific notation.
/// * `f`               - Sub-halfway (`b`) float.
pub(super) fn make_ratio<F: Float>(radix: u32, sci_exponent: i32, f: F, kind: RoundingKind)
    -> (Bigfloat, Bigfloat)
    where F: FloatType
{
    let theor = theoretical_float(f, kind).to_bigfloat();
    let factor = scaling_factor(radix, sci_exponent.abs().as_u32());
    let mut num: Bigfloat;
    let mut den: Bigfloat;

    if sci_exponent < 0 {
        // Need to have the basen factor be the numerator, and the fp
        // be the denominator. Since we assumed that theor was the numerator,
        // if it's the denominator, we need to multiply it into the numerator.
        num = factor;
        num.imul_large(&theor);
        den = Bigfloat { data: arrvec![1], exp: -theor.exp };
    } else {
        num = theor;
        den = factor;
    }

    // Scale the denominator so it has the number of bits
    // in the radix as the number of leading zeros.
    let wlz = integral_binary_factor(radix).as_usize();
    let nlz = den.leading_zeros().wrapping_sub(wlz) & (<u32 as Integer>::BITS - 1);
    small::ishl_bits(den.data_mut(), nlz);
    den.exp -= nlz.as_i32();

    // Need to scale the numerator or denominator to the same value.
    // We don't want to shift the denominator, so...
    let diff = den.exp - num.exp;
    let shift = diff.abs().as_usize();
    if diff < 0 {
        // Need to shift the numerator left.
        small::ishl(num.data_mut(), shift);
        num.exp -= shift.as_i32()
    } else if diff > 0 {
        // Need to shift denominator left, go by a power of <Limb as Integer>::BITS.
        // After this, the numerator will be non-normalized, and the
        // denominator will be normalized.
        // We need to add one to the quotient,since we're calculating the
        // ceiling of the divmod.
        let (q, r) = shift.ceil_divmod(<Limb as Integer>::BITS);
        // Since we're using a power from the denominator to the
        // numerator, we to invert r, not add u32::BITS.
        let r = -r;
        small::ishl_bits(num.data_mut(), r.as_usize());
        num.exp -= r;
        if !q.is_zero() {
            den.pad_zero_digits(q);
            den.exp -= <Limb as Integer>::BITS.as_i32() * q.as_i32();
        }
    }

    (num, den)
}

// Compare digits in BigFloat with a given iterator.
macro_rules! compare_digits {
    ($iter:ident, $radix:ident, $num:ident, $den:ident) => {
        while !$num.data.is_empty() {
            let actual = match $iter.next() {
                Some(&v) => v,
                None    => return cmp::Ordering::Less,
            };
            let expected = digit_to_char($num.quorem(&$den));
            $num.imul_small($radix);
            if actual < expected {
                return cmp::Ordering::Less;
            } else if actual > expected {
                return cmp::Ordering::Greater;
            }
        }
    };
}

/// Compare digits between the generated values the ratio and the actual view.
///
/// * `integer`     - Digits from the integer component of the mantissa.
/// * `fraction`    - Digits from the fraction component of the mantissa.
/// * `radix`       - Radix for the number parsing.
/// * `num`         - Numerator for the fraction.
/// * `denm`        - Denominator for the fraction.
pub(super) fn compare_digits<'a, Iter1, Iter2>(
    integer: Iter1,
    fraction: Iter2,
    radix: u32,
    mut num: Bigfloat,
    den: Bigfloat
)
    -> cmp::Ordering
    where Iter1: Iterator<Item=&'a u8>,
          Iter2: Iterator<Item=&'a u8>
{
    // Iterate until we get a difference in the generated digits.
    // If we run out,return Equal.
    let radix = as_limb(radix);
    let mut iter = integer.chain(fraction);
    compare_digits!(iter, radix, num, den);

    // We cannot have any trailing zeros, so if there any remaining digits,
    // we're >= to the value. We've already exhausted num.data here,
    // so need to check if integer and fraction don't have data.
    let is_none = iter.next().is_none();
    match is_none {
        true  => cmp::Ordering::Equal,
        false => cmp::Ordering::Greater,
    }
}

/// Generate the correct representation from a halfway representation.
///
/// The digits iterator must not have any trailing zeros (true for
/// `SlowDataInterface`).
///
/// * `digits`          - Actual digits from the mantissa.
/// * `radix`           - Radix for the number parsing.
/// * `sci_exponent`    - Exponent of basen string in scientific notation.
/// * `f`               - Sub-halfway (`b`) float.
pub(super) fn atof<'a, F, Data>(data: Data, radix: u32, f: F, kind: RoundingKind)
    -> F
    where F: FloatType,
          Data: SlowDataInterface<'a>
{
    // This works when we're doing, like, round-even.
    let (num, den) = make_ratio(radix, data.scientific_exponent(), f, kind);
    let integer_iter = data.integer_iter();
    let fraction_iter = data.significant_fraction_iter();
    let order = compare_digits(integer_iter, fraction_iter, radix, num, den);
    round_to_native(f, order, kind)
}

// TESTS
// -----

#[cfg(test)]
mod tests {
    use crate::util::test::*;
    use super::*;

    #[test]
    fn b_test() {
        assert_eq!(b(1e-45_f32), (1, -149).into());
        assert_eq!(b(5e-324_f64), (1, -1074).into());
        assert_eq!(b(1e-323_f64), (2, -1074).into());
        assert_eq!(b(2e-323_f64), (4, -1074).into());
        assert_eq!(b(3e-323_f64), (6, -1074).into());
        assert_eq!(b(4e-323_f64), (8, -1074).into());
        assert_eq!(b(5e-323_f64), (10, -1074).into());
        assert_eq!(b(6e-323_f64), (12, -1074).into());
        assert_eq!(b(7e-323_f64), (14, -1074).into());
        assert_eq!(b(8e-323_f64), (16, -1074).into());
        assert_eq!(b(9e-323_f64), (18, -1074).into());
        assert_eq!(b(1_f32), (8388608, -23).into());
        assert_eq!(b(1_f64), (4503599627370496, -52).into());
        assert_eq!(b(1e38_f32), (9860761, 103).into());
        assert_eq!(b(1e308_f64), (5010420900022432, 971).into());
    }

    #[test]
    fn bh_test() {
        assert_eq!(bh(1e-45_f32), (3, -150).into());
        assert_eq!(bh(5e-324_f64), (3, -1075).into());
        assert_eq!(bh(1_f32), (16777217, -24).into());
        assert_eq!(bh(1_f64), (9007199254740993, -53).into());
        assert_eq!(bh(1e38_f32), (19721523, 102).into());
        assert_eq!(bh(1e308_f64), (10020841800044865, 970).into());
    }

    // SLOW PATH

    #[test]
    fn scaling_factor_test() {
        assert_eq!(scaling_factor(10, 0), Bigfloat { data: deduce_from_u32(&[1]), exp: 0 });
        assert_eq!(scaling_factor(10, 20), Bigfloat { data: deduce_from_u32(&[1977800241, 22204]), exp: 20 });
        assert_eq!(scaling_factor(10, 300), Bigfloat { data: deduce_from_u32(&[2502905297, 773182544, 1122691908, 922368819, 2799959258, 2138784391, 2365897751, 2382789932, 3061508751, 1799019667, 3501640837, 269048281, 2748691596, 1866771432, 2228563347, 475471294, 278892994, 2258936920, 3352132269, 1505791508, 2147965370, 25052104]), exp: 300 });
    }

    #[test]
    fn make_ratio_test() {
        let (num1, den1) = make_ratio(10, -324, 0f64, RoundingKind::NearestTieEven);
        let (num2, den2) = make_ratio(10, -324, 5e-324f64, RoundingKind::NearestTieEven);
        let (num3, den3) = make_ratio(10, 307, 8.98846567431158e+307f64, RoundingKind::NearestTieEven);

        #[cfg(limb_width_32)] {
            assert_eq!(num1, Bigfloat { data: arrvec![1725370368, 1252154597, 1017462556, 675087593, 2805901938, 1401824593, 1124332496, 2380663002, 1612846757, 4128923878, 1492915356, 437569744, 2975325085, 3331531962, 3367627909, 730662168, 2699172281, 1440714968, 2778340312, 690527038, 1297115354, 763425880, 1453089653, 331561842], exp: 312 });
            assert_eq!(den1, Bigfloat { data: arrvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 134217728], exp: 312 });

            assert_eq!(num2, Bigfloat { data: arrvec![881143808, 3756463792, 3052387668, 2025262779, 4122738518, 4205473780, 3372997488, 2847021710, 543572976, 3796837043, 183778774, 1312709233, 336040663, 1404661296, 1512949137, 2191986506, 3802549547, 27177609, 4040053641, 2071581115, 3891346062, 2290277640, 64301663, 994685527], exp: 312 });
            assert_eq!(den2, Bigfloat { data: arrvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 134217728], exp: 312 });

            assert_eq!(num3, Bigfloat { data: arrvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1024, 2147483648], exp: 288 });
            assert_eq!(den3, Bigfloat { data: arrvec![1978138624, 2671552565, 2938166866, 3588566204, 1860064291, 2104472219, 2014975858, 2797301608, 462262832, 318515330, 1101517094, 1738264167, 3721375114, 414401884, 1406861075, 3053102637, 387329537, 2051556775, 1867945454, 3717689914, 1434550525, 1446648206, 238915486], exp: 288 });
        }

        #[cfg(limb_width_64)] {
            assert_eq!(num1, Bigfloat { data: arrvec![7410409304047484928, 4369968404176723173, 12051257060168107241, 4828971301551875409, 6927124077155322074, 6412022633845121254, 12778923935480989904, 14463851737583396026, 11592856673895384344, 11932880778639151320, 5571068025259989822, 6240972538554414168, 331561842], exp: 280 });
            assert_eq!(den1, Bigfloat { data: arrvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 134217728], exp: 280 });

            assert_eq!(num2, Bigfloat { data: arrvec![3784483838432903168, 13109905212530169520, 17707027106794770107, 14486913904655626228, 2334628157756414606, 789323827825812147, 1443283659023866481, 6498067065331084848, 16331825947976601418, 17351898262207902345, 16713204075779969467, 276173541953690888, 994685527], exp: 280 });
            assert_eq!(den2, Bigfloat { data: arrvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 134217728], exp: 280 });

            assert_eq!(num3, Bigfloat { data: arrvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4398046511104, 2147483648], exp: 288 });
            assert_eq!(den3, Bigfloat { data: arrvec![11474230898198052864, 15412774488649031250, 9038639357805614115, 12014318925423187826, 1368012926086910512, 7465787750175199526, 1779842542902160778, 13112975978653220627, 8811369254899559937, 15967356599166997998, 6213306735021621501, 238915486], exp: 288 });
        }
    }

    #[test]
    fn compare_digits_test() {
        // 2^-1074
        let num = Bigfloat { data: deduce_from_u32(&[1725370368, 1252154597, 1017462556, 675087593, 2805901938, 1401824593, 1124332496, 2380663002, 1612846757, 4128923878, 1492915356, 437569744, 2975325085, 3331531962, 3367627909, 730662168, 2699172281, 1440714968, 2778340312, 690527038, 1297115354, 763425880, 1453089653, 331561842]), exp: 312 };
        let den = Bigfloat { data: deduce_from_u32(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 134217728]), exp: 312 };

        // Below halfway
        let digits = b"24703282292062327208828439643411068618252990130716238221279284125033775363510437593264991818081799618989828234772285886546332835517796989819938739800539093906315035659515570226392290858392449105184435931802849936536152500319370457678249219365623669863658480757001585769269903706311928279558551332927834338409351978015531246597263579574622766465272827220056374006485499977096599470454020828166226237857393450736339007967761930577506740176324673600968951340535537458516661134223766678604162159680461914467291840300530057530849048765391711386591646239524912623653881879636239373280423891018672348497668235089863388587925628302755995657524455507255189313690836254779186948667994968324049705821028513185451396213837722826145437693412532098591327667236328124999";
        let empty = b"";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Less);

        // Exactly halfway.
        let digits = b"24703282292062327208828439643411068618252990130716238221279284125033775363510437593264991818081799618989828234772285886546332835517796989819938739800539093906315035659515570226392290858392449105184435931802849936536152500319370457678249219365623669863658480757001585769269903706311928279558551332927834338409351978015531246597263579574622766465272827220056374006485499977096599470454020828166226237857393450736339007967761930577506740176324673600968951340535537458516661134223766678604162159680461914467291840300530057530849048765391711386591646239524912623653881879636239373280423891018672348497668235089863388587925628302755995657524455507255189313690836254779186948667994968324049705821028513185451396213837722826145437693412532098591327667236328125";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Equal);

        // Above halfway.
        let digits = b"24703282292062327208828439643411068618252990130716238221279284125033775363510437593264991818081799618989828234772285886546332835517796989819938739800539093906315035659515570226392290858392449105184435931802849936536152500319370457678249219365623669863658480757001585769269903706311928279558551332927834338409351978015531246597263579574622766465272827220056374006485499977096599470454020828166226237857393450736339007967761930577506740176324673600968951340535537458516661134223766678604162159680461914467291840300530057530849048765391711386591646239524912623653881879636239373280423891018672348497668235089863388587925628302755995657524455507255189313690836254779186948667994968324049705821028513185451396213837722826145437693412532098591327667236328125001";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Greater);

        // 2*2^-1074
        let num = Bigfloat { data: deduce_from_u32(&[881143808, 3756463792, 3052387668, 2025262779, 4122738518, 4205473780, 3372997488, 2847021710, 543572976, 3796837043, 183778774, 1312709233, 336040663, 1404661296, 1512949137, 2191986506, 3802549547, 27177609, 4040053641, 2071581115, 3891346062, 2290277640, 64301663, 994685527]), exp: 312 };
        let den = Bigfloat { data: deduce_from_u32(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 134217728]), exp: 312 };

        // Below halfway
        let digits = b"74109846876186981626485318930233205854758970392148714663837852375101326090531312779794975454245398856969484704316857659638998506553390969459816219401617281718945106978546710679176872575177347315553307795408549809608457500958111373034747658096871009590975442271004757307809711118935784838675653998783503015228055934046593739791790738723868299395818481660169122019456499931289798411362062484498678713572180352209017023903285791732520220528974020802906854021606612375549983402671300035812486479041385743401875520901590172592547146296175134159774938718574737870961645638908718119841271673056017045493004705269590165763776884908267986972573366521765567941072508764337560846003984904972149117463085539556354188641513168478436313080237596295773983001708984374999";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Less);

        // Exactly halfway.
        let digits = b"74109846876186981626485318930233205854758970392148714663837852375101326090531312779794975454245398856969484704316857659638998506553390969459816219401617281718945106978546710679176872575177347315553307795408549809608457500958111373034747658096871009590975442271004757307809711118935784838675653998783503015228055934046593739791790738723868299395818481660169122019456499931289798411362062484498678713572180352209017023903285791732520220528974020802906854021606612375549983402671300035812486479041385743401875520901590172592547146296175134159774938718574737870961645638908718119841271673056017045493004705269590165763776884908267986972573366521765567941072508764337560846003984904972149117463085539556354188641513168478436313080237596295773983001708984375";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Equal);

        // Above halfway.
        let digits = b"74109846876186981626485318930233205854758970392148714663837852375101326090531312779794975454245398856969484704316857659638998506553390969459816219401617281718945106978546710679176872575177347315553307795408549809608457500958111373034747658096871009590975442271004757307809711118935784838675653998783503015228055934046593739791790738723868299395818481660169122019456499931289798411362062484498678713572180352209017023903285791732520220528974020802906854021606612375549983402671300035812486479041385743401875520901590172592547146296175134159774938718574737870961645638908718119841271673056017045493004705269590165763776884908267986972573366521765567941072508764337560846003984904972149117463085539556354188641513168478436313080237596295773983001708984375001";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Greater);

        // 4503599627370496*2^971
        let num = Bigfloat { data: deduce_from_u32(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1024, 2147483648]), exp: 288 };
        let den = Bigfloat { data: deduce_from_u32(&[1978138624, 2671552565, 2938166866, 3588566204, 1860064291, 2104472219, 2014975858, 2797301608, 462262832, 318515330, 1101517094, 1738264167, 3721375114, 414401884, 1406861075, 3053102637, 387329537, 2051556775, 1867945454, 3717689914, 1434550525, 1446648206, 238915486]), exp: 288 };

        // Below halfway
        let digits = b"89884656743115805365666807213050294962762414131308158973971342756154045415486693752413698006024096935349884403114202125541629105369684531108613657287705365884742938136589844238179474556051429647415148697857438797685859063890851407391008830874765563025951597582513936655578157348020066364210154316532161708031999";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Less);

        // Exactly halfway.
        let digits = b"89884656743115805365666807213050294962762414131308158973971342756154045415486693752413698006024096935349884403114202125541629105369684531108613657287705365884742938136589844238179474556051429647415148697857438797685859063890851407391008830874765563025951597582513936655578157348020066364210154316532161708032";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Equal);

        // Above halfway.
        let digits = b"89884656743115805365666807213050294962762414131308158973971342756154045415486693752413698006024096935349884403114202125541629105369684531108613657287705365884742938136589844238179474556051429648741514697857438797685859063890851407391008830874765563025951597582513936655578157348020066364210154316532161708032001";
        assert_eq!(compare_digits(digits.iter(), empty.iter(), 10, num.clone(), den.clone()), cmp::Ordering::Greater);
    }
}
