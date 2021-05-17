# NumToA

## `#![no_std]` Compatible with Zero Heap Allocations

The standard library provides a convenient method of converting numbers into strings, but these strings are
heap-allocated. If you have an application which needs to convert large volumes of numbers into strings, but don't
want to pay the price of heap allocation, this crate provides an efficient `no_std`-compatible method of heaplessly converting numbers
into their string representations, storing the representation within a reusable byte array.

## Supports Multiple Bases

In addition to supporting the standard base 10 conversion, this implementation allows you to select the base of
your choice. Therefore, if you want a binary representation, set the base to 2. If you want hexadecimal, set the
base to 16.

## No Unsafe

Both the standard library and itoa crate rely on unsafe functions, but this implementation has been able to avoid
the use of unsafe entirely.

## Fast

Performance is roughly identical to that of the `itoa` crate when performing base 10 conversions. Below is a benchmark
of printing 0 through 5,000,000 to `/dev/null`

```
std:   1150615048 ns
itoa:   838556714 ns
numtoa: 825544518 ns
```

## Base 10 Example

```rust
use numtoa::NumToA;
use std::io::{self, Write};

let stdout = io::stdout();
let mut stdout = stdout.lock();
let mut buffer = [0u8; 20];

let number: u32 = 162392;
let mut start_index = number.numtoa(10, &mut buffer);
let _ = stdout.write(&buffer[start_index..]);
let _ = stdout.write(b"\n");
assert_eq!(&buffer[start_index..], b"162392");

let other_number: i32 = -6235;
start_index = other_number.numtoa(10, &mut buffer);
let _ = stdout.write(&buffer[start_index..]);
let _ = stdout.write(b"\n");
assert_eq!(&buffer[start_index..], b"-6235");

let large_num: u64 = 35320842;
start_index = large_num.numtoa(10, &mut buffer);
let _ = stdout.write(&buffer[start_index..]);
let _ = stdout.write(b"\n");
assert_eq!(&buffer[start_index..], b"35320842");

let max_u64: u64 = 18446744073709551615;
start_index = max_u64.numtoa(10, &mut buffer);
let _ = stdout.write(&buffer[start_index..]);
let _ = stdout.write(b"\n");
assert_eq!(&buffer[start_index..], b"18446744073709551615");
```
