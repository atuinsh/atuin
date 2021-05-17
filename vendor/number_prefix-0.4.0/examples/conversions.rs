/// This example prints out the conversions for increasingly-large numbers, to
/// showcase how the numbers change as the input gets bigger.
/// It results in this:
///
/// ```text
///                       1000 bytes is 1.000 kB and 1000 bytes
///                    1000000 bytes is 1.000 MB and 976.562 KiB
///                 1000000000 bytes is 1.000 GB and 953.674 MiB
///              1000000000000 bytes is 1.000 TB and 931.323 GiB
///           1000000000000000 bytes is 1.000 PB and 909.495 TiB
///        1000000000000000000 bytes is 1.000 EB and 888.178 PiB
///     1000000000000000000000 bytes is 1.000 ZB and 867.362 EiB
///  1000000000000000000000000 bytes is 1.000 YB and 847.033 ZiB
///
///                       1024 bytes is 1.000 KiB and 1.024 kB
///                    1048576 bytes is 1.000 MiB and 1.049 MB
///                 1073741824 bytes is 1.000 GiB and 1.074 GB
///              1099511627776 bytes is 1.000 TiB and 1.100 TB
///           1125899906842624 bytes is 1.000 PiB and 1.126 PB
///        1152921504606847000 bytes is 1.000 EiB and 1.153 EB
///     1180591620717411300000 bytes is 1.000 ZiB and 1.181 ZB
///  1208925819614629200000000 bytes is 1.000 YiB and 1.209 YB
/// ```

extern crate number_prefix;
use number_prefix::NumberPrefix;
use std::fmt::Display;


fn main() {

    // part one, decimal prefixes
    let mut n = 1_f64;
    for _ in 0 .. 8 {
        n *= 1000_f64;

        let decimal = format_prefix(NumberPrefix::decimal(n));
        let binary  = format_prefix(NumberPrefix::binary(n));
        println!("{:26} bytes is {} and {:10}", n, decimal, binary);
    }

    println!();

    // part two, binary prefixes
    let mut n = 1_f64;
    for _ in 0 .. 8 {
        n *= 1024_f64;

        let decimal = format_prefix(NumberPrefix::decimal(n));
        let binary  = format_prefix(NumberPrefix::binary(n));
        println!("{:26} bytes is {} and {:10}", n, binary, decimal);
    }
}


fn format_prefix<T: Display>(np: NumberPrefix<T>) -> String {
    match np {
        NumberPrefix::Prefixed(prefix, n)  => format!("{:.3} {}B", n, prefix),
        NumberPrefix::Standalone(bytes)    => format!("{} bytes", bytes),
    }
}
