//! High-level data interface utilities.

/// Convert format to interface, and call function with new item as first argument.
#[cfg(not(feature = "format"))]
macro_rules! apply_interface {
    ($fn:ident, $format:expr $(,$args:ident)*) => {
        $fn(StandardFastDataInterface::new($format) $(,$args)*)
    };
}

/// Convert format to interface, and call function with new item as first argument.
#[cfg(feature = "format")]
macro_rules! apply_interface {
    ($fn:ident, $format:expr $(,$args:ident)*) => {
        match $format.interface_flags() {
            NumberFormat::PERMISSIVE_INTERFACE  => $fn(PermissiveFastDataInterface::new($format) $(,$args)*),
            NumberFormat::STANDARD_INTERFACE    => $fn(StandardFastDataInterface::new($format) $(,$args)*),
            NumberFormat::IGNORE_INTERFACE      => $fn(IgnoreFastDataInterface::new($format) $(,$args)*),
            flags                               => {
                let integer = flags.intersects(NumberFormat::INTEGER_DIGIT_SEPARATOR_FLAG_MASK);
                let fraction = flags.intersects(NumberFormat::FRACTION_DIGIT_SEPARATOR_FLAG_MASK);
                let exponent = flags.intersects(NumberFormat::EXPONENT_DIGIT_SEPARATOR_FLAG_MASK);
                match (integer, fraction, exponent) {
                    (true, true, true)      => $fn(GenericIFEFastDataInterface::new($format) $(,$args)*),
                    (false, true, true)     => $fn(GenericFEFastDataInterface::new($format) $(,$args)*),
                    (true, false, true)     => $fn(GenericIEFastDataInterface::new($format) $(,$args)*),
                    (true, true, false)     => $fn(GenericIFFastDataInterface::new($format) $(,$args)*),
                    (false, false, true)    => $fn(GenericEFastDataInterface::new($format) $(,$args)*),
                    (false, true, false)    => $fn(GenericFFastDataInterface::new($format) $(,$args)*),
                    (true, false, false)    => $fn(GenericIFastDataInterface::new($format) $(,$args)*),
                    (false, false, false)   => $fn(GenericFastDataInterface::new($format) $(,$args)*)
                }
            }
        }
    };
}
