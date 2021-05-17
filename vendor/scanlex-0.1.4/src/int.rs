pub trait Int {
    type Type;
    fn min_value() -> i64;
    fn max_value() -> i64;
    fn name() -> &'static str;
    fn cast(n: i64) -> Self::Type;
}

macro_rules! impl_int {
    ($t:ident) => {
        impl Int for $t {
            type Type = $t;

            fn min_value() -> i64 {
                $t::min_value() as i64
            }

            fn max_value() -> i64 {
                $t::max_value() as i64
            }

            fn name() -> &'static str {
                stringify!($t)
            }

            fn cast(n: i64) -> Self::Type {
                n as Self::Type
            }
        }
    }

}
impl_int!(i8);
impl_int!(i16);
impl_int!(i32);
impl_int!(i64);

impl_int!(u8);
impl_int!(u16);
impl_int!(u32);
impl_int!(u64);

