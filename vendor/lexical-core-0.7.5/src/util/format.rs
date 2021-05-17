//! Number format enumerations and bit masks.

// Sample test code for each language used:
//
//  Rust
//  ----
//
//  Setup:
//      Save to `main.rs` and run `rustc main.rs -o main`.
//
//  Code:
//      ```text
//      pub fn main() {
//          println!("{:?}", 3_.0f32);
//          println!("{:?}", "3_.0".parse::<f32>());
//      }
//      ```
//
// Python
// ------
//
//  Setup:
//      Run `python` to enter the interpreter.
//
//  Code:
//      ```text
//      print(3_.0)
//      print(float("3_.0"))
//      ```
//
//  C++
//  ---
//
//  Setup:
//      Save to `main.cc` and run `g++ main.cc -o main -std=c++XX`,
//      where XX is one of the following values:
//          - 98
//          - 03
//          - 11
//          - 14
//          - 17
//
//  Code:
//      ```text
//      #include <cstdlib>
//      #include <cstring>
//      #include <iostream>
//      #include <iterator>
//      #include <stdexcept>
//
//      double parse(const char* string) {
//          char* end;
//          double result = strtod(string, &end);
//          if (std::distance(string, reinterpret_cast<const char*>(end)) != strlen(string)) {
//              throw std::invalid_argument("did not consume entire string.");
//          }
//          return result;
//      }
//
//      int main() {
//          std::cout << 3'.0 << std::endl;
//          std::cout << parse("3'.0") << std::endl;
//      }
//      ```
//
//  C
//  -
//
//  Setup:
//      Save to `main.c` and run `gcc main.c -o main -std=cXX`,
//      where XX is one of the following values:
//          - 89
//          - 90
//          - 99
//          - 11
//          - 18
//
//  Code:
//      ```text
//      #include <stdint.h>
//      #include <stdlib.h>
//      #include <string.h>
//      #include <stdio.h>
//
//      size_t distance(const char* first, const char* last) {
//          uintptr_t x = (uintptr_t) first;
//          uintptr_t y = (uintptr_t) last;
//          return (size_t) (y - x);
//      }
//
//      double parse(const char* string) {
//          char* end;
//          double result = strtod(string, &end);
//          if (distance(string, (const char*) end) != strlen(string)) {
//              abort();
//          }
//          return result;
//      }
//
//      int main() {
//          printf("%f\n", 3'.);
//          printf("%f\n", parse("3'."));
//      }
//      ```
//
// Ruby
// ----
//
//  Setup:
//      Run `irb` to enter the interpreter.
//
//  Code:
//      ```text
//      puts 3.0_1;
//      puts "3.0_1".to_f;
//      ```
// Swift
// -----
//
//  Setup:
//      Run `swift` to enter the interpreter.
//
//  Code:
//      ```text
//      print(3.0);
//      print(Float("3.0"));
//      ```
// Golang
// ------
//
// Setup:
//      Save to `main.go` and run `go run main.go`
//
// Code:
//      ```text
//      package main
//
//      import (
//          "fmt"
//          "strconv"
//      )
//
//      func main() {
//          fmt.Println(3.0)
//          fmt.Println(strconv.ParseFloat("3.0", 64))
//      }
//      ```
//
// Haskell
// -------
//
// Setup:
//      Run `ghci` to enter the interpreter.
//
// Code:
//      ```text
//      :m Numeric
//      showFloat 3.0 ""
//      let x = "3.0"
//      read x :: Float
//      ```
//
// Javascript
// ----------
//
// Setup:
//      Run `nodejs` (or `node`) to enter the interpreter.
//
// Code:
//      ```text
//          console.log(3.0)
//          console.log(parseFloat("3.0"))
//      ```
//
// Perl
// ----
//
// Setup:
//      Run `perl -de1` to enter the interpret.
//
// Code:
//      ```text
//      print 3.01;
//      print '3.01' * 1;
//      ```
//
// PHP
// ---
//
// Setup:
//      Run `php -a` to enter the interpret.
//
// Code:
//      ```text
//      printf("%f\n", 3.0);
//      printf("%f\n", floatval("3.0"));
//      ```
//
// Java
// ----
//
// Setup:
//      Save to `main.java` and run `javac main.java`, then run `java Main`.
//
// Code:
//      ```text
//      class Main {
//          public static void main(String args[]) {
//              System.out.println(3.0);
//              System.out.println(Float.parseFloat("3.0"));
//          }
//      }
//      ```
//
// R
// -
//
// Setup:
//      Run `R` to enter the interpret.
//
// Code:
//      ```text
//      print(3.0);
//      print(as.numeric("3.0"));
//      ```
//
// Kotlin
// ------
//
// Setup:
//      Save file to `main.kt` and run `kotlinc main.kt -d main.jar`,
//      then run `java -jar main.jar`.
//
// Code:
//      ```text
//      fun main() {
//          println(3.0)
//          println("3.0".toDouble())
//      }
//      ```
//
// Julia
// -----
//
// Setup:
//      Run `julia` to enter the interpret.
//
// Code:
//      ```text
//      print(3.0);
//      print(parse(Float64, "3.0"));
//      ```
//
// C#
// --
//
// Note:
//      Mono accepts both integer and fraction decimal separators, Mono is
//      just buggy, see https://github.com/dotnet/csharplang/issues/55#issuecomment-574902516.
//
// Setup:
//      Run `csharp -langversion:X` to enter the interpret,
//      where XX is one of the following values:
//          - ISO-1
//          - ISO-2
//          - 3
//          - 4
//          - 5
//          - 6
//          - 7
//
// Code:
//      ```text
//      Console.WriteLine("{0}", 3.0);
//      Console.WriteLine("{0}", float.Parse("3.0"));
//      ```
//
// Kawa
// ----
//
// Setup:
//      Run `kawa` to enter the interpreter.
//
// Code:
//      ```text
//      3.0
//      (string->number "3.0")
//      ```
//
// Gambit-C
// --------
//
// Setup:
//      Run `gsc` to enter the interpreter.
//
// Code:
//      ```text
//      3.0
//      (string->number "3.0")
//      ```
//
// Guile
// -----
//
// Setup:
//      Run `guile` to enter the interpreter.
//
// Code:
//      ```text
//      3.0
//      (string->number "3.0")
//      ```
//
// Clojure
// -------
//
// Setup:
//      Run `clojure` to enter the interpreter.
//
// Code:
//      ```text
//      3.0
//      (Float/parseFloat "3.0")
//      ```
//
// Erlang
// ------
//
// Setup:
//      Run `erl` to enter the interpreter.
//
// Code:
//      ```text
//      io:format("~p~n", [3.0]).
//      string:to_float("3.0").
//      ```
//
// Elm
// ---
//
// Setup:
//      Run `elm repl` to enter the interpreter.
//
// Code:
//      ```text
//      3.0
//      String.toFloat "3.0"
//      ```
//
// Scala
// -----
//
// Setup:
//      Run `scala` to enter the interpreter.
//
// Code:
//      ```text
//      3.0
//      "3.0".toFloat
//      ```
//
// Elixir
// ------
//
// Setup:
//      Run `iex` to enter the interpreter.
//
// Code:
//      ```text
//      3.0;
//      String.to_float("3.0");
//      ```
//
// FORTRAN
// -------
//
// Setup:
//      Save to `main.f90` and run `gfortran -o main main.f90`
//
// Code:
//      ```text
//      program main
//        real :: x
//        character (len=30) :: word
//        word = "3."
//        read(word, *) x
//        print *, 3.
//        print *, x
//      end program main
//      ```
//
// D
// -
//
// Setup:
//      Save to `main.d` and run `dmd -run main.d`
//
// Code:
//      ```text
//      import std.conv;
//      import std.stdio;
//
//      void main()
//      {
//          writeln(3.0);
//          writeln(to!double("3.0"));
//      }
//      ```
//
// Coffeescript
// ------------
//
// Setup:
//      Run `coffee` to enter the interpreter.
//
// Code:
//      ```text
//      3.0;
//      parseFloat("3.0");
//      ```
//
// Cobol
// -----
//
// Setup:
//      Save to `main.cbl` and run `cobc main.cbl` then `cobcrun main`.
//
// Code:
//      ```text
//                IDENTIFICATION DIVISION.
//                PROGRAM-ID. main.
//
//                DATA DIVISION.
//                   WORKING-STORAGE SECTION.
//                   01 R PIC X(20)   VALUE "3.0".
//                   01 TOTAL        USAGE IS COMP-2.
//
//                PROCEDURE DIVISION.
//                   COMPUTE TOTAL = FUNCTION NUMVAL(R).
//                   Display 3.0.
//                   Display TOTAL.
//                   STOP RUN.
//      ```
//
// F#
// --
//
// Setup:
//      Run `fsharpi` to enter the interpreter.
//
// Code:
//      ```text
//      printfn "%f" 3.0;;
//      let f = float "3.0";;
//      printfn "%f" f;;
//      ```
//
// Visual Basic
// ------------
//
// Setup:
//      Save to `main.vb` and run `vbnc main.vb`.
//
// Code:
//      ```text
//      Imports System
//
//      Module Module1
//          Sub Main()
//              Console.WriteLine(Format$(3.0, "0.0000000000000"))
//              Console.WriteLine(Format$(CDbl("3.0"), "0.0000000000000"))
//          End Sub
//      End Module
//      ```
//
// OCaml
// -----
//
// Setup:
//      Save to `main.ml` and run `ocamlc -o main main.ml`.
//
// Code:
//      ```text
//      Printf.printf "%f\n" 3.0
//      let () =
//          let f = float_of_string "3.0" in
//          Printf.printf "%f\n" f
//      ```
//
// Objective-C
// -----------
//
// Setup:
//      Save to `main.m` and run `gcc -o main -lobjc -lgnustep-base main.m -fconstant-string-class=NSConstantString`.
//
// Code:
//      ```text
//      #import <Foundation/Foundation.h>
//      #import <stdio.h>
//
//      int main(int argv, char* argc[])
//      {
//          printf("%f\n", 3.0);
//          NSString *s = @"3.0";
//          double f = [s doubleValue];
//          printf("%f\n", f);
//      }
//      ```
//
// ReasonML
// --------
//
// Setup:
//      Run `rtop` to enter the interpreter.
//
// Code:
//      ```text
//      Printf.printf("%f\n", 3.0);
//      Printf.printf("%f\n", float_of_string("3.0"));
//      ```
//
// Zig
// ---
//
// Setup:
//      Save to `main.zig` and run `zig build-exe main.zig`
//
// Code:
//      ```text
//      const std = @import("std");
//
//      pub fn main() void {
//          const f: f64 = 3.0;
//          std.debug.warn("{}\n", f);
//          const x: f64 = std.fmt.parseFloat(f64, "3.0") catch unreachable;
//          std.debug.warn("{}\n", x);
//      }
//      ```
//
//
// Octave (and Matlab)
// -------------------
//
// Setup:
//      Run `octave` to enter the interpreter, or
//      run `octave --traditional` to enter the Matlab interpret.
//
// Code:
//      ```text
//      3.0
//      str2double("3.0")
//      ```
//
// Sage
// ----
//
// Setup:
//      Run `sage` to enter the interpreter.
//
// Code:
//      ```text
//      3.0
//      float("3.0")
//      ```
//
// JSON
// ----
//
// Setup:
//      Run `node` (or `nodejs`) to enter the JS interpreter.
//
// Code:
//      ```text
//      JSON.parse("3.0")
//      ```
//
// TOML
// ----
//
// Setup:
//      Run `python` to enter the Python interpreter.
//
// Code:
//      ```text
//      import tomlkit
//      tomlkit.parse("a = 3.0")
//      ```
//
// XML
// ---
//
// Setup:
//      Run `python` to enter the Python interpreter.
//
// Code:
//      ```text
//      from lxml import etree
//
//      def validate_xml(xsd, xml):
//          '''Validate XML file against schema'''
//
//          schema = etree.fromstring(xsd)
//          doc = etree.fromstring(xml)
//          xmlschema = etree.XMLSchema(schema)
//
//          return xmlschema.validate(doc)
//
//
//      xsd = b'''<?xml version="1.0" encoding="UTF-8"?>
//      <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
//          <xs:element name="prize" type="xs:float"/>
//      </xs:schema>'''
//
//      xml = b'''<?xml version="1.0" encoding="UTF-8"?>
//      <prize>3.0</prize>
//      '''
//
//      validate_xml(xsd, xml)
//      ```
//
// SQLite
// ------
//
// Setup:
//      Run `sqlite3 :memory:` to enter the sqlite3 interpreter
//      with an in-memory database.
//
// Code:
//      ```text
//      CREATE TABLE stocks (price real);
//      INSERT INTO stocks VALUES (3.0);
//      SELECT * FROM stocks;
//      ```
//
// PostgreSQL
// ----------
//
// Setup:
//      Run `initdb -D db` to create a database data direction,
//      then run `pg_ctl -D db start` to start the server, then run
//      `createdb` to create a user database and `psql` to start the
//      interpreter.
//
// Code:
//      ```text
//      CREATE TABLE stocks (price real);
//      INSERT INTO stocks VALUES (3.0);
//      SELECT * FROM stocks;
//      ```
//
// MySQL
// -----
//
// Setup:
//      Run `mysqld` to start the server, then run `mysql` to start the
//      interpreter.
//
// Code:
//      ```text
//      USE mysql;
//      CREATE TABLE stocks (price real);
//      INSERT INTO stocks VALUES (3.0);
//      SELECT * FROM stocks;
//      ```
//
// MongoDB
// -------
//
// Setup:
//      Run `mongod --dbpath data/db` to start the server, then run
//      `mongo` to start the interpreter.
//
// Code:
//      ```text
//      use mydb
//      db.movie.insert({"name": 3.0})
//      db.movie.find()
//      ```

use super::config;

cfg_if! {
if #[cfg(not(feature = "format"))] {
    bitflags! {
        /// Dummy bitflags for the float format.
        #[doc(hidden)]
        #[derive(Default)]
        pub struct NumberFormat: u64 {
            const __NONEXHAUSTIVE = 0;
        }
    }

    impl NumberFormat {
        #[inline]
        pub fn standard() -> Option<NumberFormat> {
            Some(NumberFormat::default())
        }

        #[inline]
        pub fn digit_separator(&self) -> u8 {
            0
        }
    }
} else {
    // HELPERS

    // Determine if character is valid ASCII.
    #[inline]
    fn is_ascii(ch: u8) -> bool {
        ch.is_ascii()
    }

    /// Determine if the digit separator is valid.
    #[inline]
    #[cfg(not(feature = "radix"))]
    fn is_valid_separator(ch: u8) -> bool {
        match ch {
            b'0' ..= b'9'       => false,
            b'+' | b'.' | b'-'  => false,
            _                   => (
                is_ascii(ch)
                && ch != config::get_exponent_default_char()
            )
        }
    }

    /// Determine if the digit separator is valid.
    #[inline]
    #[cfg(feature = "radix")]
    fn is_valid_separator(ch: u8) -> bool {
        match ch {
            b'A' ..= b'Z'       => false,
            b'a' ..= b'z'       => false,
            b'0' ..= b'9'       => false,
            b'+' | b'.' | b'-'  => false,
            _                   => (
                is_ascii(ch)
                && ch != config::get_exponent_default_char()
                && ch != config::get_exponent_backup_char()
            )
        }
    }

    /// Convert digit separator to flags.
    #[inline]
    const fn digit_separator_to_flags(ch: u8) -> u64 {
        (ch as u64) << 56
    }

    /// Extract digit separator from flags.
    #[inline]
    const fn digit_separator_from_flags(flag: u64) -> u8 {
        (flag >> 56) as u8
    }

    // BITFLAGS

    bitflags! {
        /// Bitflags for a serialized number format.
        ///
        /// This is used to derive the high-level bitflags.The default
        /// representation has no digit separators, no required integer or
        /// fraction digits, required exponent digits, and no digit separators.
        ///
        /// Bit Flags Layout
        /// ----------------
        ///
        /// The bitflags has the lower bits designated for flags that modify
        /// the parsing behavior of lexical, and the upper 8 bits set for the
        /// digit separator, allowing any valid ASCII character as a
        /// separator. The first 32-bits are reserved for non-digit separator
        /// flags, bits 32-55 are reserved for digit separator flags, and
        /// the last 8 bits for the digit separator.
        //
        /// ```text
        ///  0   1   2   3   4   5   6   7   8   9   0   1   2   3   4   5
        /// +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
        /// |I/R|F/R|E/R|+/M|R/M|e/e|+/E|R/E|e/F|S/S|S/C|     RESERVED      |
        /// +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
        ///
        ///  16  17  18  19  20  21  22  23  24  25  26  27  28  29  30  31
        /// +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
        /// |                           RESERVED                            |
        /// +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
        ///
        ///  32  33  34  35  36  37  38  39  40  41  42  43  44  45  46  47
        /// +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
        /// |I/I|F/I|E/I|I/L|F/L|E/L|I/T|F/T|E/T|I/C|F/C|E/C|S/D| RESERVED  |
        /// +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
        ///
        ///  48  49  50  51  52  53  54  55  56  57  58  59  60  62  62  63
        /// +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
        /// |           RESERVED            |        Digit Separator        |
        /// +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
        ///
        /// Where:
        ///     I/R = Required integer digits.
        ///     F/R = Required fraction digits.
        ///     E/R = Required exponent digits.
        ///     +/M = No mantissa positive sign.
        ///     R/M = Required positive sign.
        ///     e/e = No exponent notation.
        ///     +/E = No exponent positive sign.
        ///     R/E = Required exponent sign.
        ///     e/F = No exponent without fraction.
        ///     S/S = No special (non-finite) values.
        ///     S/C = Case-sensitive special (non-finite) values.
        ///     I/I = Integer internal digit separator.
        ///     F/I = Fraction internal digit separator.
        ///     E/I = Exponent internal digit separator.
        ///     I/L = Integer leading digit separator.
        ///     F/L = Fraction leading digit separator.
        ///     E/L = Exponent leading digit separator.
        ///     I/T = Integer trailing digit separator.
        ///     F/T = Fraction trailing digit separator.
        ///     E/T = Exponent trailing digit separator.
        ///     I/C = Integer consecutive digit separator.
        ///     F/C = Fraction consecutive digit separator.
        ///     E/C = Exponent consecutive digit separator.
        ///     S/D = Special (non-finite) digit separator.
        /// ```
        ///
        /// Note:
        /// -----
        ///
        /// In order to limit the format specification and avoid parsing
        /// non-numerical data, all number formats require some significant
        /// digits. Examples of always invalid numbers include:
        /// - ``
        /// - `.`
        /// - `e`
        /// - `e7`
        ///
        /// Test Cases:
        /// -----------
        ///
        /// The following test-cases are used to define whether a literal or
        /// a string float is valid in a given language, and these tests are
        /// used to denote features in pre-defined formats. Only a few
        /// of these flags may modify the parsing behavior of integers.
        /// Integer parsing is assumed to be derived from float parsing,
        /// so if consecutive digit separators are valid in the integer
        /// component of a float, they are also valid in an integer.
        ///
        /// ```text
        /// 0: '.3'         // Non-required integer.
        /// 1: '3.'         // Non-required fraction.
        /// 2: '3e'         // Non-required exponent.
        /// 3. '+3.0'       // Mantissa positive sign.
        /// 4: '3.0e7'      // Exponent notation.
        /// 5: '3.0e+7'     // Exponent positive sign.
        /// 6. '3e7'        // Exponent notation without fraction.
        /// 7: 'NaN'        // Special (non-finite) values.
        /// 8: 'NAN'        // Case-sensitive special (non-finite) values.
        /// 9: '3_4.01'     // Integer internal digit separator.
        /// A: '3.0_1'      // Fraction internal digit separator.
        /// B: '3.0e7_1'    // Exponent internal digit separator.
        /// C: '_3.01'      // Integer leading digit separator.
        /// D: '3._01'      // Fraction leading digit separator.
        /// E: '3.0e_71'    // Exponent leading digit separator.
        /// F: '3_.01'      // Integer trailing digit separator.
        /// G: '3.01_'      // Fraction trailing digit separator.
        /// H: '3.0e71_'    // Exponent trailing digit separator.
        /// I: '3__4.01'    // Integer consecutive digit separator.
        /// J: '3.0__1'     // Fraction consecutive digit separator.
        /// K: '3.0e7__1'   // Exponent consecutive digit separator.
        /// L: 'In_f'       // Special (non-finite) digit separator.
        /// M: '010'        // No integer leading zeros.
        /// N: '010.0'      // No float leading zeros.
        /// ```
        ///
        /// Currently Supported Programming and Data Languages:
        /// ---------------------------------------------------
        ///
        /// 1. Rust
        /// 2. Python
        /// 3. C++ (98, 03, 11, 14, 17)
        /// 4. C (89, 90, 99, 11, 18)
        /// 5. Ruby
        /// 6. Swift
        /// 7. Go
        /// 8. Haskell
        /// 9. Javascript
        /// 10. Perl
        /// 11. PHP
        /// 12. Java
        /// 13. R
        /// 14. Kotlin
        /// 15. Julia
        /// 16. C# (ISO-1, ISO-2, 3, 4, 5, 6, 7)
        /// 17. Kawa
        /// 18. Gambit-C
        /// 19. Guile
        /// 20. Clojure
        /// 21. Erlang
        /// 22. Elm
        /// 23. Scala
        /// 24. Elixir
        /// 25. FORTRAN
        /// 26. D
        /// 27. Coffeescript
        /// 28. Cobol
        /// 29. F#
        /// 30. Visual Basic
        /// 31. OCaml
        /// 32. Objective-C
        /// 33. ReasonML
        /// 34. Octave
        /// 35. Matlab
        /// 36. Zig
        /// 37. SageMath
        /// 38. JSON
        /// 39. TOML
        /// 40. XML
        /// 41. SQLite
        /// 42. PostgreSQL
        /// 43. MySQL
        /// 44. MongoDB
        #[derive(Default)]
        pub struct NumberFormat: u64 {
            // MASKS & FLAGS

            /// Mask to extract the flag bits.
            #[doc(hidden)]
            const FLAG_MASK                             = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_POSITIVE_MANTISSA_SIGN.bits
                | Self::REQUIRED_MANTISSA_SIGN.bits
                | Self::NO_EXPONENT_NOTATION.bits
                | Self::NO_POSITIVE_EXPONENT_SIGN.bits
                | Self::REQUIRED_EXPONENT_SIGN.bits
                | Self::NO_EXPONENT_WITHOUT_FRACTION.bits
                | Self::NO_SPECIAL.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
                | Self::SPECIAL_DIGIT_SEPARATOR.bits
            );

            /// Mask to extract the flag bits controlling interface parsing.
            ///
            /// This mask controls all the flags handled by the interface,
            /// omitting those that are handled prior. This limits the
            /// number of match paths required to determine the correct
            /// interface.
            const INTERFACE_FLAG_MASK                   = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_EXPONENT_NOTATION.bits
                | Self::NO_POSITIVE_EXPONENT_SIGN.bits
                | Self::REQUIRED_EXPONENT_SIGN.bits
                | Self::NO_EXPONENT_WITHOUT_FRACTION.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            /// Mask to extract digit separator flags.
            #[doc(hidden)]
            const DIGIT_SEPARATOR_FLAG_MASK             = (
                Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
                | Self::SPECIAL_DIGIT_SEPARATOR.bits
            );

            /// Mask to extract integer digit separator flags.
            #[doc(hidden)]
            const INTEGER_DIGIT_SEPARATOR_FLAG_MASK     = (
                Self::INTEGER_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::INTEGER_LEADING_DIGIT_SEPARATOR.bits
                | Self::INTEGER_TRAILING_DIGIT_SEPARATOR.bits
                | Self::INTEGER_CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            /// Mask to extract fraction digit separator flags.
            #[doc(hidden)]
            const FRACTION_DIGIT_SEPARATOR_FLAG_MASK     = (
                Self::FRACTION_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_LEADING_DIGIT_SEPARATOR.bits
                | Self::FRACTION_TRAILING_DIGIT_SEPARATOR.bits
                | Self::FRACTION_CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            /// Mask to extract exponent digit separator flags.
            #[doc(hidden)]
            const EXPONENT_DIGIT_SEPARATOR_FLAG_MASK     = (
                Self::EXPONENT_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_LEADING_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_TRAILING_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            /// Mask to extract exponent flags.
            #[doc(hidden)]
            const EXPONENT_FLAG_MASK                    = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_POSITIVE_EXPONENT_SIGN.bits
                | Self::REQUIRED_EXPONENT_SIGN.bits
                | Self::NO_EXPONENT_WITHOUT_FRACTION.bits
                | Self::EXPONENT_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_LEADING_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_TRAILING_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // NON-DIGIT SEPARATOR FLAGS & MASKS

            /// Digits are required before the decimal point.
            #[doc(hidden)]
            const REQUIRED_INTEGER_DIGITS               = 0b0000000000000000000000000000000000000000000000000000000000000001;

            /// Digits are required after the decimal point.
            /// This check will only occur if the decimal point is present.
            #[doc(hidden)]
            const REQUIRED_FRACTION_DIGITS              = 0b0000000000000000000000000000000000000000000000000000000000000010;

            /// Digits are required after the exponent character.
            /// This check will only occur if the exponent character is present.
            #[doc(hidden)]
            const REQUIRED_EXPONENT_DIGITS              = 0b0000000000000000000000000000000000000000000000000000000000000100;

            /// Digits are required before or after the control characters.
            #[doc(hidden)]
            const REQUIRED_DIGITS                       = (
                Self::REQUIRED_INTEGER_DIGITS.bits
                | Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
            );

            /// Positive sign before the mantissa is not allowed.
            #[doc(hidden)]
            const NO_POSITIVE_MANTISSA_SIGN             = 0b0000000000000000000000000000000000000000000000000000000000001000;

            /// Positive sign before the mantissa is required.
            #[doc(hidden)]
            const REQUIRED_MANTISSA_SIGN                = 0b0000000000000000000000000000000000000000000000000000000000010000;

            /// Exponent notation is not allowed.
            #[doc(hidden)]
            const NO_EXPONENT_NOTATION                  = 0b0000000000000000000000000000000000000000000000000000000000100000;

            /// Positive sign before the exponent is not allowed.
            #[doc(hidden)]
            const NO_POSITIVE_EXPONENT_SIGN             = 0b0000000000000000000000000000000000000000000000000000000001000000;

            /// Positive sign before the exponent is required.
            #[doc(hidden)]
            const REQUIRED_EXPONENT_SIGN                = 0b0000000000000000000000000000000000000000000000000000000010000000;

            /// Exponent without a fraction component is not allowed.
            ///
            /// This only checks if a decimal point precedes the exponent character.
            /// To require fraction digits or exponent digits with this check,
            /// please use the appropriate flags.
            #[doc(hidden)]
            const NO_EXPONENT_WITHOUT_FRACTION          = 0b0000000000000000000000000000000000000000000000000000000100000000;

            /// Special (non-finite) values are not allowed.
            #[doc(hidden)]
            const NO_SPECIAL                            = 0b0000000000000000000000000000000000000000000000000000001000000000;

            /// Special (non-finite) values are case-sensitive.
            #[doc(hidden)]
            const CASE_SENSITIVE_SPECIAL                = 0b0000000000000000000000000000000000000000000000000000010000000000;

            /// Leading zeros before an integer value are not allowed.
            ///
            /// If the value is a literal, then this distinction applies
            /// when the value is treated like an integer literal, typically
            /// when there is no decimal point. If the value is parsed,
            /// then this distinction applies when the value as parsed
            /// as an integer.
            ///
            /// # Warning
            ///
            /// This also does not mean that the value parsed will be correct,
            /// for example, in languages like C, this will not auto-
            /// deduce that the radix is 8 with leading zeros, for an octal
            /// literal.
            #[doc(hidden)]
            const NO_INTEGER_LEADING_ZEROS              = 0b0000000000000000000000000000000000000000000000000000100000000000;

            /// Leading zeros before a float value are not allowed.
            ///
            /// If the value is a literal, then this distinction applies
            /// when the value is treated like an integer float, typically
            /// when there is a decimal point. If the value is parsed,
            /// then this distinction applies when the value as parsed
            /// as a float.
            ///
            /// # Warning
            ///
            /// This also does not mean that the value parsed will be correct,
            /// for example, in languages like C, this will not auto-
            /// deduce that the radix is 8 with leading zeros, for an octal
            /// literal.
            #[doc(hidden)]
            const NO_FLOAT_LEADING_ZEROS                = 0b0000000000000000000000000000000000000000000000000001000000000000;

            // DIGIT SEPARATOR FLAGS & MASKS

            /// Digit separators are allowed between integer digits.
            #[doc(hidden)]
            const INTEGER_INTERNAL_DIGIT_SEPARATOR      = 0b0000000000000000000000000000000100000000000000000000000000000000;

            /// A digit separator is allowed before any integer digits.
            #[doc(hidden)]
            const INTEGER_LEADING_DIGIT_SEPARATOR       = 0b0000000000000000000000000000001000000000000000000000000000000000;

            /// A digit separator is allowed after any integer digits.
            #[doc(hidden)]
            const INTEGER_TRAILING_DIGIT_SEPARATOR      = 0b0000000000000000000000000000010000000000000000000000000000000000;

            /// Multiple consecutive integer digit separators are allowed.
            #[doc(hidden)]
            const INTEGER_CONSECUTIVE_DIGIT_SEPARATOR   = 0b0000000000000000000000000000100000000000000000000000000000000000;

            /// Digit separators are allowed between fraction digits.
            #[doc(hidden)]
            const FRACTION_INTERNAL_DIGIT_SEPARATOR     = 0b0000000000000000000000000001000000000000000000000000000000000000;

            /// A digit separator is allowed before any fraction digits.
            #[doc(hidden)]
            const FRACTION_LEADING_DIGIT_SEPARATOR      = 0b0000000000000000000000000010000000000000000000000000000000000000;

            /// A digit separator is allowed after any fraction digits.
            #[doc(hidden)]
            const FRACTION_TRAILING_DIGIT_SEPARATOR     = 0b0000000000000000000000000100000000000000000000000000000000000000;

            /// Multiple consecutive fraction digit separators are allowed.
            #[doc(hidden)]
            const FRACTION_CONSECUTIVE_DIGIT_SEPARATOR  = 0b0000000000000000000000001000000000000000000000000000000000000000;

            /// Digit separators are allowed between exponent digits.
            #[doc(hidden)]
            const EXPONENT_INTERNAL_DIGIT_SEPARATOR     = 0b0000000000000000000000010000000000000000000000000000000000000000;

            /// A digit separator is allowed before any exponent digits.
            #[doc(hidden)]
            const EXPONENT_LEADING_DIGIT_SEPARATOR      = 0b0000000000000000000000100000000000000000000000000000000000000000;

            /// A digit separator is allowed after any exponent digits.
            #[doc(hidden)]
            const EXPONENT_TRAILING_DIGIT_SEPARATOR     = 0b0000000000000000000001000000000000000000000000000000000000000000;

            /// Multiple consecutive exponent digit separators are allowed.
            #[doc(hidden)]
            const EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR  = 0b0000000000000000000010000000000000000000000000000000000000000000;

            /// Digit separators are allowed between digits.
            #[doc(hidden)]
            const INTERNAL_DIGIT_SEPARATOR              = (
                Self::INTEGER_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_INTERNAL_DIGIT_SEPARATOR.bits
            );

            /// A digit separator is allowed before any digits.
            #[doc(hidden)]
            const LEADING_DIGIT_SEPARATOR               = (
                Self::INTEGER_LEADING_DIGIT_SEPARATOR.bits
                | Self::FRACTION_LEADING_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_LEADING_DIGIT_SEPARATOR.bits
            );

            /// A digit separator is allowed after any digits.
            #[doc(hidden)]
            const TRAILING_DIGIT_SEPARATOR              = (
                Self::INTEGER_TRAILING_DIGIT_SEPARATOR.bits
                | Self::FRACTION_TRAILING_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_TRAILING_DIGIT_SEPARATOR.bits
            );

            /// Multiple consecutive digit separators are allowed.
            #[doc(hidden)]
            const CONSECUTIVE_DIGIT_SEPARATOR           = (
                Self::INTEGER_CONSECUTIVE_DIGIT_SEPARATOR.bits
                | Self::FRACTION_CONSECUTIVE_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            /// Any digit separators are allowed in special (non-finite) values.
            #[doc(hidden)]
            const SPECIAL_DIGIT_SEPARATOR               = 0b0000000000000000000100000000000000000000000000000000000000000000;

            // PRE-DEFINED
            //
            // Sample Format Shorthand:
            // ------------------------
            //
            // The format shorthand lists the test cases, and if applicable,
            // the digit separator character. For example, the shorthand
            // `[134-_]` specifies it passes tests 1, 3, and 4, and uses
            // `'_'` as a digit-separator character. Meanwhile, `[0]` means it
            // passes test 0, and has no digit separator.

            // RUST LITERAL [4569ABFGHIJKMN-_]
            /// Float format for a Rust literal floating-point number.
            const RUST_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_DIGITS.bits
                | Self::NO_POSITIVE_MANTISSA_SIGN.bits
                | Self::NO_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // RUST STRING [0134567MN]
            /// Float format to parse a Rust float from string.
            const RUST_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // RUST STRING STRICT [01345678MN]
            /// `RUST_STRING`, but enforces strict equality for special values.
            const RUST_STRING_STRICT = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            /// Float format for a Python literal floating-point number.
            const PYTHON_LITERAL = Self::PYTHON3_LITERAL.bits;

            /// Float format to parse a Python float from string.
            const PYTHON_STRING = Self::PYTHON3_STRING.bits;

            // PYTHON3 LITERAL [013456N]
            /// Float format for a Python3 literal floating-point number.
            const PYTHON3_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
            );

            // PYTHON3 STRING [0134567MN]
            /// Float format to parse a Python3 float from string.
            const PYTHON3_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // PYTHON2 LITERAL [013456MN]
            /// Float format for a Python2 literal floating-point number.
            const PYTHON2_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // PYTHON2 STRING [0134567MN]
            /// Float format to parse a Python2 float from string.
            const PYTHON2_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            /// Float format for a C++ literal floating-point number.
            const CXX_LITERAL = Self::CXX17_LITERAL.bits;

            /// Float format to parse a C++ float from string.
            const CXX_STRING = Self::CXX17_STRING.bits;

            // C++17 LITERAL [01345689ABMN-']
            /// Float format for a C++17 literal floating-point number.
            const CXX17_LITERAL = (
                digit_separator_to_flags(b'\'')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
            );

            // C++17 STRING [013456MN]
            const CXX17_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // C++14 LITERAL [01345689ABMN-']
            /// Float format for a C++14 literal floating-point number.
            const CXX14_LITERAL = (
                digit_separator_to_flags(b'\'')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
            );

            // C++14 STRING [013456MN]
            /// Float format to parse a C++14 float from string.
            const CXX14_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // C++11 LITERAL [0134568MN]
            /// Float format for a C++11 literal floating-point number.
            const CXX11_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // C++11 STRING [013456MN]
            /// Float format to parse a C++11 float from string.
            const CXX11_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // C++03 LITERAL [0134567MN]
            /// Float format for a C++03 literal floating-point number.
            const CXX03_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // C++03 STRING [013456MN]
            /// Float format to parse a C++03 float from string.
            const CXX03_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // C++98 LITERAL [0134567MN]
            /// Float format for a C++98 literal floating-point number.
            const CXX98_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // C++98 STRING [013456MN]
            /// Float format to parse a C++98 float from string.
            const CXX98_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            /// Float format for a C literal floating-point number.
            const C_LITERAL = Self::C18_LITERAL.bits;

            /// Float format to parse a C float from string.
            const C_STRING = Self::C18_STRING.bits;

            // C18 LITERAL [0134568MN]
            /// Float format for a C18 literal floating-point number.
            const C18_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // C18 STRING [013456MN]
            /// Float format to parse a C18 float from string.
            const C18_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // C11 LITERAL [0134568MN]
            /// Float format for a C11 literal floating-point number.
            const C11_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // C11 STRING [013456MN]
            /// Float format to parse a C11 float from string.
            const C11_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // C99 LITERAL [0134568MN]
            /// Float format for a C99 literal floating-point number.
            const C99_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // C99 STRING [013456MN]
            /// Float format to parse a C99 float from string.
            const C99_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // C90 LITERAL [0134567MN]
            /// Float format for a C90 literal floating-point number.
            const C90_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // C90 STRING [013456MN]
            /// Float format to parse a C90 float from string.
            const C90_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // C89 LITERAL [0134567MN]
            /// Float format for a C89 literal floating-point number.
            const C89_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // C89 STRING [013456MN]
            /// Float format to parse a C89 float from string.
            const C89_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // RUBY LITERAL [345689AM-_]
            /// Float format for a Ruby literal floating-point number.
            const RUBY_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
            );

            // RUBY STRING [01234569ABMN-_]
            /// Float format to parse a Ruby float from string.
            // Note: Amazingly, Ruby 1.8+ do not allow parsing special values.
            const RUBY_STRING = (
                digit_separator_to_flags(b'_')
                | Self::NO_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
            );

            // SWIFT LITERAL [34569ABFGHIJKMN-_]
            /// Float format for a Swift literal floating-point number.
            const SWIFT_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // SWIFT STRING [13456MN]
            /// Float format to parse a Swift float from string.
            const SWIFT_STRING = Self::REQUIRED_FRACTION_DIGITS.bits;

            // GO LITERAL [0134567MN]
            /// Float format for a Golang literal floating-point number.
            const GO_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // GO STRING [013456MN]
            /// Float format to parse a Golang float from string.
            const GO_STRING = Self::REQUIRED_FRACTION_DIGITS.bits;

            // HASKELL LITERAL [456MN]
            /// Float format for a Haskell literal floating-point number.
            const HASKELL_LITERAL = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_POSITIVE_MANTISSA_SIGN.bits
                | Self::NO_SPECIAL.bits
            );

            // HASKELL STRING [45678MN]
            /// Float format to parse a Haskell float from string.
            const HASKELL_STRING = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_POSITIVE_MANTISSA_SIGN.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // JAVASCRIPT LITERAL [01345678M]
            /// Float format for a Javascript literal floating-point number.
            const JAVASCRIPT_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
            );

            // JAVASCRIPT STRING [012345678MN]
            /// Float format to parse a Javascript float from string.
            const JAVASCRIPT_STRING = Self::CASE_SENSITIVE_SPECIAL.bits;

            // PERL LITERAL [0134569ABDEFGHIJKMN-_]
            /// Float format for a Perl literal floating-point number.
            const PERL_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_LEADING_DIGIT_SEPARATOR.bits
                | Self::EXPONENT_LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // PERL STRING [01234567MN]
            /// Float format to parse a Perl float from string.
            const PERL_STRING = 0;

            // PHP LITERAL [01345678MN]
            /// Float format for a PHP literal floating-point number.
            const PHP_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // PHP STRING [0123456MN]
            /// Float format to parse a PHP float from string.
            const PHP_STRING = Self::NO_SPECIAL.bits;

            // JAVA LITERAL [0134569ABIJKMN-_]
            /// Float format for a Java literal floating-point number.
            const JAVA_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // JAVA STRING [01345678MN]
            /// Float format to parse a Java float from string.
            const JAVA_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // R LITERAL [01345678MN]
            /// Float format for a R literal floating-point number.
            const R_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // R STRING [01234567MN]
            /// Float format to parse a R float from string.
            const R_STRING = 0;

            // KOTLIN LITERAL [0134569ABIJKN-_]
            /// Float format for a Kotlin literal floating-point number.
            const KOTLIN_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // KOTLIN STRING [0134568MN]
            /// Float format to parse a Kotlin float from string.
            const KOTLIN_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // JULIA LITERAL [01345689AMN-_]
            /// Float format for a Julia literal floating-point number.
            const JULIA_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::INTEGER_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_INTERNAL_DIGIT_SEPARATOR.bits
            );

            // JULIA STRING [01345678MN]
            /// Float format to parse a Julia float from string.
            const JULIA_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            /// Float format for a C# literal floating-point number.
            const CSHARP_LITERAL = Self::CSHARP7_LITERAL.bits;

            /// Float format to parse a C# float from string.
            const CSHARP_STRING = Self::CSHARP7_STRING.bits;

            // CSHARP7 LITERAL [034569ABIJKMN-_]
            /// Float format for a C#7 literal floating-point number.
            const CSHARP7_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // CSHARP7 STRING [0134568MN]
            /// Float format to parse a C#7 float from string.
            const CSHARP7_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // CSHARP6 LITERAL [03456MN]
            /// Float format for a C#6 literal floating-point number.
            const CSHARP6_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // CSHARP6 STRING [0134568MN]
            /// Float format to parse a C#6 float from string.
            const CSHARP6_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // CSHARP5 LITERAL [03456MN]
            /// Float format for a C#5 literal floating-point number.
            const CSHARP5_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // CSHARP5 STRING [0134568MN]
            /// Float format to parse a C#5 float from string.
            const CSHARP5_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // CSHARP4 LITERAL [03456MN]
            /// Float format for a C#4 literal floating-point number.
            const CSHARP4_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // CSHARP4 STRING [0134568MN]
            /// Float format to parse a C#4 float from string.
            const CSHARP4_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // CSHARP3 LITERAL [03456MN]
            /// Float format for a C#3 literal floating-point number.
            const CSHARP3_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // CSHARP3 STRING [0134568MN]
            /// Float format to parse a C#3 float from string.
            const CSHARP3_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // CSHARP2 LITERAL [03456MN]
            /// Float format for a C#2 literal floating-point number.
            const CSHARP2_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // CSHARP2 STRING [0134568MN]
            /// Float format to parse a C#2 float from string.
            const CSHARP2_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // CSHARP1 LITERAL [03456MN]
            /// Float format for a C#1 literal floating-point number.
            const CSHARP1_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // CSHARP1 STRING [0134568MN]
            /// Float format to parse a C#1 float from string.
            const CSHARP1_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // KAWA LITERAL [013456MN]
            /// Float format for a Kawa literal floating-point number.
            const KAWA_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // KAWA STRING [013456MN]
            /// Float format to parse a Kawa float from string.
            const KAWA_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // GAMBITC LITERAL [013456MN]
            /// Float format for a Gambit-C literal floating-point number.
            const GAMBITC_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // GAMBITC STRING [013456MN]
            /// Float format to parse a Gambit-C float from string.
            const GAMBITC_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // GUILE LITERAL [013456MN]
            /// Float format for a Guile literal floating-point number.
            const GUILE_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // GUILE STRING [013456MN]
            /// Float format to parse a Guile float from string.
            const GUILE_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // CLOJURE LITERAL [13456MN]
            /// Float format for a Clojure literal floating-point number.
            const CLOJURE_LITERAL = (
                Self::REQUIRED_INTEGER_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // CLOJURE STRING [01345678MN]
            /// Float format to parse a Clojure float from string.
            const CLOJURE_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // ERLANG LITERAL [34578MN]
            /// Float format for an Erlang literal floating-point number.
            const ERLANG_LITERAL = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_EXPONENT_WITHOUT_FRACTION.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // ERLANG STRING [345MN]
            /// Float format to parse an Erlang float from string.
            const ERLANG_STRING = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_EXPONENT_WITHOUT_FRACTION.bits
                | Self::NO_SPECIAL.bits
            );

            // ELM LITERAL [456]
            /// Float format for an Elm literal floating-point number.
            const ELM_LITERAL = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_POSITIVE_MANTISSA_SIGN.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
            );

            // ELM STRING [01345678MN]
            /// Float format to parse an Elm float from string.
            // Note: There is no valid representation of NaN, just Infinity.
            const ELM_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // SCALA LITERAL [3456]
            /// Float format for a Scala literal floating-point number.
            const SCALA_LITERAL = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
            );

            // SCALA STRING [01345678MN]
            /// Float format to parse a Scala float from string.
            const SCALA_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // ELIXIR LITERAL [3459ABMN-_]
            /// Float format for an Elixir literal floating-point number.
            const ELIXIR_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_DIGITS.bits
                | Self::NO_EXPONENT_WITHOUT_FRACTION.bits
                | Self::NO_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
            );

            // ELIXIR STRING [345MN]
            /// Float format to parse an Elixir float from string.
            const ELIXIR_STRING = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_EXPONENT_WITHOUT_FRACTION.bits
                | Self::NO_SPECIAL.bits
            );

            // FORTRAN LITERAL [013456MN]
            /// Float format for a FORTRAN literal floating-point number.
            const FORTRAN_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // FORTRAN STRING [0134567MN]
            /// Float format to parse a FORTRAN float from string.
            const FORTRAN_STRING = Self::REQUIRED_EXPONENT_DIGITS.bits;

            // D LITERAL [0134569ABFGHIJKN-_]
            /// Float format for a D literal floating-point number.
            const D_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // D STRING [01345679AFGMN-_]
            /// Float format to parse a D float from string.
            const D_STRING = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::INTEGER_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_INTERNAL_DIGIT_SEPARATOR.bits
                | Self::INTEGER_TRAILING_DIGIT_SEPARATOR.bits
                | Self::FRACTION_TRAILING_DIGIT_SEPARATOR.bits
            );

            // COFFEESCRIPT LITERAL [01345678]
            /// Float format for a Coffeescript literal floating-point number.
            const COFFEESCRIPT_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
            );

            // COFFEESCRIPT STRING [012345678MN]
            /// Float format to parse a Coffeescript float from string.
            const COFFEESCRIPT_STRING = Self::CASE_SENSITIVE_SPECIAL.bits;

            // COBOL LITERAL [0345MN]
            /// Float format for a Cobol literal floating-point number.
            const COBOL_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_EXPONENT_WITHOUT_FRACTION.bits
                | Self::NO_SPECIAL.bits
            );

            // COBOL STRING [012356MN]
            /// Float format to parse a Cobol float from string.
            const COBOL_STRING = (
                Self::REQUIRED_EXPONENT_SIGN.bits
                | Self::NO_SPECIAL.bits
            );

            // FSHARP LITERAL [13456789ABIJKMN-_]
            /// Float format for a F# literal floating-point number.
            const FSHARP_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_INTEGER_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // FSHARP STRING [013456789ABCDEFGHIJKLMN-_]
            /// Float format to parse a F# float from string.
            const FSHARP_STRING = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
                | Self::SPECIAL_DIGIT_SEPARATOR.bits
            );

            // VB LITERAL [03456MN]
            /// Float format for a Visual Basic literal floating-point number.
            const VB_LITERAL = (
                Self::REQUIRED_FRACTION_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // VB STRING [01345678MN]
            /// Float format to parse a Visual Basic float from string.
            // Note: To my knowledge, Visual Basic cannot parse infinity.
            const VB_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // OCAML LITERAL [1456789ABDFGHIJKMN-_]
            /// Float format for an OCaml literal floating-point number.
            const OCAML_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_INTEGER_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_POSITIVE_MANTISSA_SIGN.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // OCAML STRING [01345679ABCDEFGHIJKLMN-_]
            /// Float format to parse an OCaml float from string.
            const OCAML_STRING = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
                | Self::SPECIAL_DIGIT_SEPARATOR.bits
            );

            // OBJECTIVEC LITERAL [013456MN]
            /// Float format for an Objective-C literal floating-point number.
            const OBJECTIVEC_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // OBJECTIVEC STRING [013456MN]
            /// Float format to parse an Objective-C float from string.
            const OBJECTIVEC_STRING = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // REASONML LITERAL [13456789ABDFGHIJKMN-_]
            /// Float format for a ReasonML literal floating-point number.
            const REASONML_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_INTEGER_DIGITS.bits
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // REASONML STRING [01345679ABCDEFGHIJKLMN-_]
            /// Float format to parse a ReasonML float from string.
            const REASONML_STRING = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
                | Self::SPECIAL_DIGIT_SEPARATOR.bits
            );

            // OCTAVE LITERAL [013456789ABDFGHIJKMN-_]
            /// Float format for an Octave literal floating-point number.
            // Note: Octave accepts both NaN and nan, Inf and inf.
            const OCTAVE_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // OCTAVE STRING [01345679ABCDEFGHIJKMN-,]
            /// Float format to parse an Octave float from string.
            const OCTAVE_STRING = (
                digit_separator_to_flags(b',')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // MATLAB LITERAL [013456789ABDFGHIJKMN-_]
            /// Float format for an Matlab literal floating-point number.
            // Note: Matlab accepts both NaN and nan, Inf and inf.
            const MATLAB_LITERAL = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::FRACTION_LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // MATLAB STRING [01345679ABCDEFGHIJKMN-,]
            /// Float format to parse an Matlab float from string.
            const MATLAB_STRING = (
                digit_separator_to_flags(b',')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::LEADING_DIGIT_SEPARATOR.bits
                | Self::TRAILING_DIGIT_SEPARATOR.bits
                | Self::CONSECUTIVE_DIGIT_SEPARATOR.bits
            );

            // ZIG LITERAL [1456MN]
            /// Float format for a Zig literal floating-point number.
            const ZIG_LITERAL = (
                Self::REQUIRED_INTEGER_DIGITS.bits
                | Self::NO_POSITIVE_MANTISSA_SIGN.bits
                | Self::NO_SPECIAL.bits
            );

            // ZIG STRING [01234567MN]
            /// Float format to parse a Zig float from string.
            const ZIG_STRING = 0;

            // SAGE LITERAL [012345678MN]
            /// Float format for a Sage literal floating-point number.
            // Note: Both Infinity and infinity are accepted.
            const SAGE_LITERAL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
            );

            // SAGE STRING [01345679ABMN-_]
            /// Float format to parse a Sage float from string.
            const SAGE_STRING = (
                digit_separator_to_flags(b'_')
                | Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
            );

            // JSON [456]
            /// Float format for a JSON literal floating-point number.
            const JSON = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_POSITIVE_MANTISSA_SIGN.bits
                | Self::NO_SPECIAL.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
            );

            // TOML [34569AB]
            /// Float format for a TOML literal floating-point number.
            const TOML = (
                Self::REQUIRED_DIGITS.bits
                | Self::NO_SPECIAL.bits
                | Self::INTERNAL_DIGIT_SEPARATOR.bits
                | Self::NO_INTEGER_LEADING_ZEROS.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
            );

            // YAML (defined in-terms of JSON schema).
            /// Float format for a YAML literal floating-point number.
            const YAML = Self::JSON.bits;

            // XML [01234578MN]
            /// Float format for a XML literal floating-point number.
            const XML = Self::CASE_SENSITIVE_SPECIAL.bits;

            // SQLITE [013456MN]
            /// Float format for a SQLite literal floating-point number.
            const SQLITE = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // POSTGRESQL [013456MN]
            /// Float format for a PostgreSQL literal floating-point number.
            const POSTGRESQL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // MYSQL [013456MN]
            /// Float format for a MySQL literal floating-point number.
            const MYSQL = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::NO_SPECIAL.bits
            );

            // MONGODB [01345678M]
            /// Float format for a MongoDB literal floating-point number.
            const MONGODB = (
                Self::REQUIRED_EXPONENT_DIGITS.bits
                | Self::CASE_SENSITIVE_SPECIAL.bits
                | Self::NO_FLOAT_LEADING_ZEROS.bits
            );

            // HIDDEN DEFAULTS

            /// Float format when no flags are set.
            #[doc(hidden)]
            const PERMISSIVE = 0;

            /// Permissive interface float format flags.
            #[doc(hidden)]
            const PERMISSIVE_INTERFACE = Self::PERMISSIVE.bits & Self::INTERFACE_FLAG_MASK.bits;

            /// Standard float format.
            #[doc(hidden)]
            const STANDARD = Self::RUST_STRING.bits;

            /// Standard interface float format flags.
            #[doc(hidden)]
            const STANDARD_INTERFACE = Self::STANDARD.bits & Self::INTERFACE_FLAG_MASK.bits;

            /// Float format when all digit separator flags are set.
            #[doc(hidden)]
            const IGNORE = Self::DIGIT_SEPARATOR_FLAG_MASK.bits;

            /// Ignore interface float format flags.
            #[doc(hidden)]
            const IGNORE_INTERFACE = Self::IGNORE.bits & Self::INTERFACE_FLAG_MASK.bits;
        }
    }

    // Ensure all our bit flags are valid.
    macro_rules! check_subsequent_flags {
        ($x:ident, $y:ident) => (
            const_assert!(NumberFormat::$x.bits << 1 == NumberFormat::$y.bits);
        );
    }

    // Non-digit separator flags.
    const_assert!(NumberFormat::REQUIRED_INTEGER_DIGITS.bits == 1);
    check_subsequent_flags!(REQUIRED_INTEGER_DIGITS, REQUIRED_FRACTION_DIGITS);
    check_subsequent_flags!(REQUIRED_FRACTION_DIGITS, REQUIRED_EXPONENT_DIGITS);
    check_subsequent_flags!(REQUIRED_EXPONENT_DIGITS, NO_POSITIVE_MANTISSA_SIGN);
    check_subsequent_flags!(NO_POSITIVE_MANTISSA_SIGN, REQUIRED_MANTISSA_SIGN);
    check_subsequent_flags!(REQUIRED_MANTISSA_SIGN, NO_EXPONENT_NOTATION);
    check_subsequent_flags!(NO_EXPONENT_NOTATION, NO_POSITIVE_EXPONENT_SIGN);
    check_subsequent_flags!(NO_POSITIVE_EXPONENT_SIGN, REQUIRED_EXPONENT_SIGN);
    check_subsequent_flags!(REQUIRED_EXPONENT_SIGN, NO_EXPONENT_WITHOUT_FRACTION);
    check_subsequent_flags!(NO_EXPONENT_WITHOUT_FRACTION, NO_SPECIAL);
    check_subsequent_flags!(NO_SPECIAL, CASE_SENSITIVE_SPECIAL);
    check_subsequent_flags!(CASE_SENSITIVE_SPECIAL, NO_INTEGER_LEADING_ZEROS);
    check_subsequent_flags!(NO_INTEGER_LEADING_ZEROS, NO_FLOAT_LEADING_ZEROS);

    // Digit separator flags.
    const_assert!(NumberFormat::INTEGER_INTERNAL_DIGIT_SEPARATOR.bits == 1 << 32);
    check_subsequent_flags!(INTEGER_INTERNAL_DIGIT_SEPARATOR, INTEGER_LEADING_DIGIT_SEPARATOR);
    check_subsequent_flags!(INTEGER_LEADING_DIGIT_SEPARATOR, INTEGER_TRAILING_DIGIT_SEPARATOR);
    check_subsequent_flags!(INTEGER_TRAILING_DIGIT_SEPARATOR, INTEGER_CONSECUTIVE_DIGIT_SEPARATOR);
    check_subsequent_flags!(INTEGER_CONSECUTIVE_DIGIT_SEPARATOR, FRACTION_INTERNAL_DIGIT_SEPARATOR);
    check_subsequent_flags!(FRACTION_INTERNAL_DIGIT_SEPARATOR, FRACTION_LEADING_DIGIT_SEPARATOR);
    check_subsequent_flags!(FRACTION_LEADING_DIGIT_SEPARATOR, FRACTION_TRAILING_DIGIT_SEPARATOR);
    check_subsequent_flags!(FRACTION_TRAILING_DIGIT_SEPARATOR, FRACTION_CONSECUTIVE_DIGIT_SEPARATOR);
    check_subsequent_flags!(FRACTION_CONSECUTIVE_DIGIT_SEPARATOR, EXPONENT_INTERNAL_DIGIT_SEPARATOR);
    check_subsequent_flags!(EXPONENT_INTERNAL_DIGIT_SEPARATOR, EXPONENT_LEADING_DIGIT_SEPARATOR);
    check_subsequent_flags!(EXPONENT_LEADING_DIGIT_SEPARATOR, EXPONENT_TRAILING_DIGIT_SEPARATOR);
    check_subsequent_flags!(EXPONENT_TRAILING_DIGIT_SEPARATOR, EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR);
    check_subsequent_flags!(EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR, SPECIAL_DIGIT_SEPARATOR);

    /// Add flag to flags
    macro_rules! add_flag {
        ($flags:ident, $bool:ident, $flag:ident) => {
            if $bool {
                $flags |= NumberFormat::$flag;
            }
        };
    }

    impl NumberFormat {
        /// Compile float format value from specifications.
        ///
        /// * `digit_separator`                         - Character to separate digits.
        /// * `required_integer_digits`                 - If digits are required before the decimal point.
        /// * `required_fraction_digits`                - If digits are required after the decimal point.
        /// * `required_exponent_digits`                - If digits are required after the exponent character.
        /// * `no_positive_mantissa_sign`               - If positive sign before the mantissa is not allowed.
        /// * `required_mantissa_sign`                  - If positive sign before the mantissa is required.
        /// * `no_exponent_notation`                    - If exponent notation is not allowed.
        /// * `no_positive_exponent_sign`               - If positive sign before the exponent is not allowed.
        /// * `required_exponent_sign`                  - If sign before the exponent is required.
        /// * `no_exponent_without_fraction`            - If exponent without fraction is not allowed.
        /// * `no_special`                              - If special (non-finite) values are not allowed.
        /// * `case_sensitive_special`                  - If special (non-finite) values are case-sensitive.
        /// * `no_integer_leading_zeros`                - If leading zeros before an integer are not allowed.
        /// * `no_float_leading_zeros`                  - If leading zeros before a float are not allowed.
        /// * `integer_internal_digit_separator`        - If digit separators are allowed between integer digits.
        /// * `fraction_internal_digit_separator`       - If digit separators are allowed between fraction digits.
        /// * `exponent_internal_digit_separator`       - If digit separators are allowed between exponent digits.
        /// * `integer_leading_digit_separator`         - If a digit separator is allowed before any integer digits.
        /// * `fraction_leading_digit_separator`        - If a digit separator is allowed before any fraction digits.
        /// * `exponent_leading_digit_separator`        - If a digit separator is allowed before any exponent digits.
        /// * `integer_trailing_digit_separator`        - If a digit separator is allowed after any integer digits.
        /// * `fraction_trailing_digit_separator`       - If a digit separator is allowed after any fraction digits.
        /// * `exponent_trailing_digit_separator`       - If a digit separator is allowed after any exponent digits.
        /// * `integer_consecutive_digit_separator`     - If multiple consecutive integer digit separators are allowed.
        /// * `fraction_consecutive_digit_separator`    - If multiple consecutive fraction digit separators are allowed.
        /// * `special_digit_separator`                 - If any digit separators are allowed in special (non-finite) values.
        ///
        /// Returns the value if it was able to compile the format,
        /// otherwise, returns None.
        #[cfg_attr(feature = "radix", doc = " Digit separators must not be in the character group `[A-Za-z0-9+.-]`, nor be equal to")]
        #[cfg_attr(feature = "radix", doc = " [`get_exponent_default_char`](fn.get_exponent_default_char.html) or")]
        #[cfg_attr(feature = "radix", doc = " [`get_exponent_backup_char`](fn.get_exponent_backup_char.html).")]
        #[cfg_attr(not(feature = "radix"), doc = " Digit separators must not be in the character group `[0-9+.-]`, nor be equal to")]
        #[cfg_attr(not(feature = "radix"), doc = " [get_exponent_default_char](fn.get_exponent_default_char.html).")]
        ///
        /// # Versioning
        ///
        /// Due to the potential addition of bitflags required to parse a given
        /// number, this function is not considered stable and will not
        /// be stabilized until lexical-core version 1.0. Any changes will
        /// ensure they introduce compile errors in existing code, and will
        /// not the current major/minor version.
        #[inline]
        pub fn compile(
            digit_separator: u8,
            required_integer_digits: bool,
            required_fraction_digits: bool,
            required_exponent_digits: bool,
            no_positive_mantissa_sign: bool,
            required_mantissa_sign: bool,
            no_exponent_notation: bool,
            no_positive_exponent_sign: bool,
            required_exponent_sign: bool,
            no_exponent_without_fraction: bool,
            no_special: bool,
            case_sensitive_special: bool,
            no_integer_leading_zeros: bool,
            no_float_leading_zeros: bool,
            integer_internal_digit_separator: bool,
            fraction_internal_digit_separator: bool,
            exponent_internal_digit_separator: bool,
            integer_leading_digit_separator: bool,
            fraction_leading_digit_separator: bool,
            exponent_leading_digit_separator: bool,
            integer_trailing_digit_separator: bool,
            fraction_trailing_digit_separator: bool,
            exponent_trailing_digit_separator: bool,
            integer_consecutive_digit_separator: bool,
            fraction_consecutive_digit_separator: bool,
            exponent_consecutive_digit_separator: bool,
            special_digit_separator: bool
        ) -> Option<NumberFormat> {
            let mut format = NumberFormat::default();
            // Generic flags.
            add_flag!(format, required_integer_digits, REQUIRED_INTEGER_DIGITS);
            add_flag!(format, required_fraction_digits, REQUIRED_FRACTION_DIGITS);
            add_flag!(format, required_exponent_digits, REQUIRED_EXPONENT_DIGITS);
            add_flag!(format, no_positive_mantissa_sign, NO_POSITIVE_MANTISSA_SIGN);
            add_flag!(format, required_mantissa_sign, REQUIRED_MANTISSA_SIGN);
            add_flag!(format, no_exponent_notation, NO_EXPONENT_NOTATION);
            add_flag!(format, no_positive_exponent_sign, NO_POSITIVE_EXPONENT_SIGN);
            add_flag!(format, required_exponent_sign, REQUIRED_EXPONENT_SIGN);
            add_flag!(format, no_exponent_without_fraction, NO_EXPONENT_WITHOUT_FRACTION);
            add_flag!(format, no_special, NO_SPECIAL);
            add_flag!(format, case_sensitive_special, CASE_SENSITIVE_SPECIAL);
            add_flag!(format, no_integer_leading_zeros, NO_INTEGER_LEADING_ZEROS);
            add_flag!(format, no_float_leading_zeros, NO_FLOAT_LEADING_ZEROS);

            // Digit separator flags.
            add_flag!(format, integer_internal_digit_separator, INTEGER_INTERNAL_DIGIT_SEPARATOR);
            add_flag!(format, fraction_internal_digit_separator, FRACTION_INTERNAL_DIGIT_SEPARATOR);
            add_flag!(format, exponent_internal_digit_separator, EXPONENT_INTERNAL_DIGIT_SEPARATOR);
            add_flag!(format, integer_leading_digit_separator, INTEGER_LEADING_DIGIT_SEPARATOR);
            add_flag!(format, fraction_leading_digit_separator, FRACTION_LEADING_DIGIT_SEPARATOR);
            add_flag!(format, exponent_leading_digit_separator, EXPONENT_LEADING_DIGIT_SEPARATOR);
            add_flag!(format, integer_trailing_digit_separator, INTEGER_TRAILING_DIGIT_SEPARATOR);
            add_flag!(format, fraction_trailing_digit_separator, FRACTION_TRAILING_DIGIT_SEPARATOR);
            add_flag!(format, exponent_trailing_digit_separator, EXPONENT_TRAILING_DIGIT_SEPARATOR);
            add_flag!(format, integer_consecutive_digit_separator, INTEGER_CONSECUTIVE_DIGIT_SEPARATOR);
            add_flag!(format, fraction_consecutive_digit_separator, FRACTION_CONSECUTIVE_DIGIT_SEPARATOR);
            add_flag!(format, exponent_consecutive_digit_separator, EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR);
            add_flag!(format, special_digit_separator, SPECIAL_DIGIT_SEPARATOR);

            // Digit separator.
            if format.intersects(NumberFormat::DIGIT_SEPARATOR_FLAG_MASK) {
                format.bits |= digit_separator_to_flags(digit_separator);
            }

            // Validation.
            let is_invalid =
                !is_valid_separator(digit_separator)
                || format.intersects(NumberFormat::NO_EXPONENT_NOTATION) && format.intersects(NumberFormat::EXPONENT_FLAG_MASK)
                || no_positive_mantissa_sign && required_mantissa_sign
                || no_positive_exponent_sign && required_exponent_sign
                || no_special && (case_sensitive_special || special_digit_separator)
                || format & NumberFormat::INTEGER_DIGIT_SEPARATOR_FLAG_MASK == NumberFormat::INTEGER_CONSECUTIVE_DIGIT_SEPARATOR
                || format & NumberFormat::FRACTION_DIGIT_SEPARATOR_FLAG_MASK == NumberFormat::FRACTION_CONSECUTIVE_DIGIT_SEPARATOR
                || format & NumberFormat::EXPONENT_DIGIT_SEPARATOR_FLAG_MASK == NumberFormat::EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR;
            match is_invalid {
                true  => None,
                false => Some(format)
            }
        }

        /// Compile permissive number format.
        ///
        /// The permissive number format does not require any control
        /// grammar, besides the presence of mantissa digits.
        ///
        /// This function cannot fail, but returns an option for consistency
        /// with other grammar compilers.
        pub fn permissive() -> Option<NumberFormat> {
            Some(NumberFormat::PERMISSIVE)
        }

        /// Compile standard number format.
        ///
        /// The standard number format is guaranteed to be identical
        /// to the format expected by Rust's string to number parsers.
        ///
        /// This function cannot fail, but returns an option for consistency
        /// with other grammar compilers.
        pub fn standard() -> Option<NumberFormat> {
            Some(NumberFormat::STANDARD)
        }

        /// Compile ignore number format.
        ///
        /// The ignore number format ignores all digit separators,
        /// and is permissive for all other control grammar, so
        /// implements a fast parser.
        ///
        /// * `digit_separator`                         - Character to separate digits.
        ///
        /// Returns the value if it was able to compile the format,
        /// otherwise, returns None.
        pub fn ignore(digit_separator: u8) -> Option<NumberFormat> {
            if !is_valid_separator(digit_separator) {
                return None
            }

            let mut format = NumberFormat::IGNORE;
            format.bits |= digit_separator_to_flags(digit_separator);

            Some(format)
        }

        /// Create float format directly from digit separator for unittests.
        #[cfg(test)]
        #[inline]
        pub(crate) fn from_separator(digit_separator: u8) -> NumberFormat {
            NumberFormat { bits: digit_separator_to_flags(digit_separator) }
        }

        /// Get the flag bits from the compiled float format.
        #[inline]
        pub fn flags(self) -> NumberFormat {
            return self & NumberFormat::FLAG_MASK
        }

        /// Get the interface flag bits from the compiled float format.
        #[inline]
        pub(crate) fn interface_flags(self) -> NumberFormat {
            return self & NumberFormat::INTERFACE_FLAG_MASK
        }

        /// Get the digit separator from the compiled float format.
        #[inline]
        pub fn digit_separator(self) -> u8 {
            digit_separator_from_flags(self.bits)
        }

        /// Get if digits are required before the decimal point.
        #[inline]
        pub fn required_integer_digits(self) -> bool {
            self.intersects(NumberFormat::REQUIRED_INTEGER_DIGITS)
        }

        /// Get if digits are required after the decimal point.
        #[inline]
        pub fn required_fraction_digits(self) -> bool {
            self.intersects(NumberFormat::REQUIRED_FRACTION_DIGITS)
        }

        /// Get if digits are required after the exponent character.
        #[inline]
        pub fn required_exponent_digits(self) -> bool {
            self.intersects(NumberFormat::REQUIRED_EXPONENT_DIGITS)
        }

        /// Get if digits are required before or after the decimal point.
        #[inline]
        pub fn required_digits(self) -> bool {
            self.intersects(NumberFormat::REQUIRED_DIGITS)
        }

        /// Get if positive sign before the mantissa is not allowed.
        #[inline]
        pub fn no_positive_mantissa_sign(self) -> bool {
            self.intersects(NumberFormat::NO_POSITIVE_MANTISSA_SIGN)
        }

        /// Get if positive sign before the mantissa is required.
        #[inline]
        pub fn required_mantissa_sign(self) -> bool {
            self.intersects(NumberFormat::REQUIRED_MANTISSA_SIGN)
        }

        /// Get if exponent notation is not allowed.
        #[inline]
        pub fn no_exponent_notation(self) -> bool {
            self.intersects(NumberFormat::NO_EXPONENT_NOTATION)
        }

        /// Get if positive sign before the exponent is not allowed.
        #[inline]
        pub fn no_positive_exponent_sign(self) -> bool {
            self.intersects(NumberFormat::NO_POSITIVE_EXPONENT_SIGN)
        }

        /// Get if sign before the exponent is required.
        #[inline]
        pub fn required_exponent_sign(self) -> bool {
            self.intersects(NumberFormat::REQUIRED_EXPONENT_SIGN)
        }

        /// Get if exponent without fraction is not allowed.
        #[inline]
        pub fn no_exponent_without_fraction(self) -> bool {
            self.intersects(NumberFormat::NO_EXPONENT_WITHOUT_FRACTION)
        }

        /// Get if special (non-finite) values are not allowed.
        #[inline]
        pub fn no_special(self) -> bool {
            self.intersects(NumberFormat::NO_SPECIAL)
        }

        /// Get if special (non-finite) values are case-sensitive.
        #[inline]
        pub fn case_sensitive_special(self) -> bool {
            self.intersects(NumberFormat::CASE_SENSITIVE_SPECIAL)
        }

        /// Get if leading zeros before an integer are not allowed.
        #[inline]
        pub fn no_integer_leading_zeros(self) -> bool {
            self.intersects(NumberFormat::NO_INTEGER_LEADING_ZEROS)
        }

        /// Get if leading zeros before a float are not allowed.
        #[inline]
        pub fn no_float_leading_zeros(self) -> bool {
            self.intersects(NumberFormat::NO_FLOAT_LEADING_ZEROS)
        }

        /// Get if digit separators are allowed between integer digits.
        #[inline]
        pub fn integer_internal_digit_separator(self) -> bool {
            self.intersects(NumberFormat::INTEGER_INTERNAL_DIGIT_SEPARATOR)
        }

        /// Get if digit separators are allowed between fraction digits.
        #[inline]
        pub fn fraction_internal_digit_separator(self) -> bool {
            self.intersects(NumberFormat::FRACTION_INTERNAL_DIGIT_SEPARATOR)
        }

        /// Get if digit separators are allowed between exponent digits.
        #[inline]
        pub fn exponent_internal_digit_separator(self) -> bool {
            self.intersects(NumberFormat::EXPONENT_INTERNAL_DIGIT_SEPARATOR)
        }

        /// Get if digit separators are allowed between digits.
        #[inline]
        pub fn internal_digit_separator(self) -> bool {
            self.intersects(NumberFormat::INTERNAL_DIGIT_SEPARATOR)
        }

        /// Get if a digit separator is allowed before any integer digits.
        #[inline]
        pub fn integer_leading_digit_separator(self) -> bool {
            self.intersects(NumberFormat::INTEGER_LEADING_DIGIT_SEPARATOR)
        }

        /// Get if a digit separator is allowed before any fraction digits.
        #[inline]
        pub fn fraction_leading_digit_separator(self) -> bool {
            self.intersects(NumberFormat::FRACTION_LEADING_DIGIT_SEPARATOR)
        }

        /// Get if a digit separator is allowed before any exponent digits.
        #[inline]
        pub fn exponent_leading_digit_separator(self) -> bool {
            self.intersects(NumberFormat::EXPONENT_LEADING_DIGIT_SEPARATOR)
        }

        /// Get if a digit separator is allowed before any digits.
        #[inline]
        pub fn leading_digit_separator(self) -> bool {
            self.intersects(NumberFormat::LEADING_DIGIT_SEPARATOR)
        }

        /// Get if a digit separator is allowed after any integer digits.
        #[inline]
        pub fn integer_trailing_digit_separator(self) -> bool {
            self.intersects(NumberFormat::INTEGER_TRAILING_DIGIT_SEPARATOR)
        }

        /// Get if a digit separator is allowed after any fraction digits.
        #[inline]
        pub fn fraction_trailing_digit_separator(self) -> bool {
            self.intersects(NumberFormat::FRACTION_TRAILING_DIGIT_SEPARATOR)
        }

        /// Get if a digit separator is allowed after any exponent digits.
        #[inline]
        pub fn exponent_trailing_digit_separator(self) -> bool {
            self.intersects(NumberFormat::EXPONENT_TRAILING_DIGIT_SEPARATOR)
        }

        /// Get if a digit separator is allowed after any digits.
        #[inline]
        pub fn trailing_digit_separator(self) -> bool {
            self.intersects(NumberFormat::TRAILING_DIGIT_SEPARATOR)
        }

        /// Get if multiple consecutive integer digit separators are allowed.
        #[inline]
        pub fn integer_consecutive_digit_separator(self) -> bool {
            self.intersects(NumberFormat::INTEGER_CONSECUTIVE_DIGIT_SEPARATOR)
        }

        /// Get if multiple consecutive fraction digit separators are allowed.
        #[inline]
        pub fn fraction_consecutive_digit_separator(self) -> bool {
            self.intersects(NumberFormat::FRACTION_CONSECUTIVE_DIGIT_SEPARATOR)
        }

        /// Get if multiple consecutive exponent digit separators are allowed.
        #[inline]
        pub fn exponent_consecutive_digit_separator(self) -> bool {
            self.intersects(NumberFormat::EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR)
        }

        /// Get if multiple consecutive digit separators are allowed.
        #[inline]
        pub fn consecutive_digit_separator(self) -> bool {
            self.intersects(NumberFormat::CONSECUTIVE_DIGIT_SEPARATOR)
        }

        /// Get if any digit separators are allowed in special (non-finite) values.
        #[inline]
        pub fn special_digit_separator(self) -> bool {
            self.intersects(NumberFormat::SPECIAL_DIGIT_SEPARATOR)
        }
    }

    // TESTS
    // -----

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_is_valid_separator() {
            assert_eq!(is_valid_separator(b'_'), true);
            assert_eq!(is_valid_separator(b'\''), true);
            assert_eq!(is_valid_separator(b'0'), false);
            assert_eq!(is_valid_separator(128), false);
        }

        #[test]
        fn test_compile() {
            // Test all false
            let flags = NumberFormat::compile(b'_', false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false, false).unwrap();
            assert_eq!(flags.flags(), NumberFormat::default());
            assert_eq!(flags.digit_separator(), 0);
        }

        #[test]
        fn test_permissive() {
            let flags = NumberFormat::ignore(b'_').unwrap();
            assert_eq!(flags.flags(), NumberFormat::DIGIT_SEPARATOR_FLAG_MASK);
        }

        #[test]
        fn test_ignore() {
            let flags = NumberFormat::ignore(b'_').unwrap();
            assert_eq!(flags.flags(), NumberFormat::DIGIT_SEPARATOR_FLAG_MASK);
            assert_eq!(flags.digit_separator(), b'_');
            assert_eq!(flags.required_integer_digits(), false);
            assert_eq!(flags.required_fraction_digits(), false);
            assert_eq!(flags.required_exponent_digits(), false);
            assert_eq!(flags.required_digits(), false);
            assert_eq!(flags.no_positive_mantissa_sign(), false);
            assert_eq!(flags.required_mantissa_sign(), false);
            assert_eq!(flags.no_exponent_notation(), false);
            assert_eq!(flags.no_positive_exponent_sign(), false);
            assert_eq!(flags.required_exponent_sign(), false);
            assert_eq!(flags.no_exponent_without_fraction(), false);
            assert_eq!(flags.no_special(), false);
            assert_eq!(flags.case_sensitive_special(), false);
            assert_eq!(flags.no_integer_leading_zeros(), false);
            assert_eq!(flags.no_float_leading_zeros(), false);
            assert_eq!(flags.integer_internal_digit_separator(), true);
            assert_eq!(flags.fraction_internal_digit_separator(), true);
            assert_eq!(flags.exponent_internal_digit_separator(), true);
            assert_eq!(flags.internal_digit_separator(), true);
            assert_eq!(flags.integer_leading_digit_separator(), true);
            assert_eq!(flags.fraction_leading_digit_separator(), true);
            assert_eq!(flags.exponent_leading_digit_separator(), true);
            assert_eq!(flags.leading_digit_separator(), true);
            assert_eq!(flags.integer_trailing_digit_separator(), true);
            assert_eq!(flags.fraction_trailing_digit_separator(), true);
            assert_eq!(flags.exponent_trailing_digit_separator(), true);
            assert_eq!(flags.trailing_digit_separator(), true);
            assert_eq!(flags.integer_consecutive_digit_separator(), true);
            assert_eq!(flags.fraction_consecutive_digit_separator(), true);
            assert_eq!(flags.exponent_consecutive_digit_separator(), true);
            assert_eq!(flags.consecutive_digit_separator(), true);
            assert_eq!(flags.special_digit_separator(), true);
        }

        #[test]
        fn test_flags() {
            let flags = [
                NumberFormat::REQUIRED_INTEGER_DIGITS,
                NumberFormat::REQUIRED_FRACTION_DIGITS,
                NumberFormat::REQUIRED_EXPONENT_DIGITS,
                NumberFormat::NO_POSITIVE_MANTISSA_SIGN,
                NumberFormat::REQUIRED_MANTISSA_SIGN,
                NumberFormat::NO_EXPONENT_NOTATION,
                NumberFormat::NO_POSITIVE_EXPONENT_SIGN,
                NumberFormat::REQUIRED_EXPONENT_SIGN,
                NumberFormat::NO_EXPONENT_WITHOUT_FRACTION,
                NumberFormat::NO_SPECIAL,
                NumberFormat::CASE_SENSITIVE_SPECIAL,
                NumberFormat::NO_INTEGER_LEADING_ZEROS,
                NumberFormat::NO_FLOAT_LEADING_ZEROS,
                NumberFormat::INTEGER_INTERNAL_DIGIT_SEPARATOR,
                NumberFormat::FRACTION_INTERNAL_DIGIT_SEPARATOR,
                NumberFormat::EXPONENT_INTERNAL_DIGIT_SEPARATOR,
                NumberFormat::INTEGER_LEADING_DIGIT_SEPARATOR,
                NumberFormat::FRACTION_LEADING_DIGIT_SEPARATOR,
                NumberFormat::EXPONENT_LEADING_DIGIT_SEPARATOR,
                NumberFormat::INTEGER_TRAILING_DIGIT_SEPARATOR,
                NumberFormat::FRACTION_TRAILING_DIGIT_SEPARATOR,
                NumberFormat::EXPONENT_TRAILING_DIGIT_SEPARATOR,
                NumberFormat::INTEGER_CONSECUTIVE_DIGIT_SEPARATOR,
                NumberFormat::FRACTION_CONSECUTIVE_DIGIT_SEPARATOR,
                NumberFormat::EXPONENT_CONSECUTIVE_DIGIT_SEPARATOR,
                NumberFormat::SPECIAL_DIGIT_SEPARATOR
            ];
            for &flag in flags.iter() {
                assert_eq!(flag.flags(), flag);
                assert_eq!(flag.digit_separator(), 0);
            }
        }

        #[test]
        fn test_constants() {
            let flags = [
                NumberFormat::RUST_LITERAL,
                NumberFormat::RUST_STRING,
                NumberFormat::RUST_STRING_STRICT,
                NumberFormat::PYTHON_LITERAL,
                NumberFormat::PYTHON_STRING,
                NumberFormat::CXX17_LITERAL,
                NumberFormat::CXX17_STRING,
                NumberFormat::CXX14_LITERAL,
                NumberFormat::CXX14_STRING,
                NumberFormat::CXX11_LITERAL,
                NumberFormat::CXX11_STRING,
                NumberFormat::CXX03_LITERAL,
                NumberFormat::CXX03_STRING,
                NumberFormat::CXX98_LITERAL,
                NumberFormat::CXX98_STRING,
                NumberFormat::C18_LITERAL,
                NumberFormat::C18_STRING,
                NumberFormat::C11_LITERAL,
                NumberFormat::C11_STRING,
                NumberFormat::C99_LITERAL,
                NumberFormat::C99_STRING,
                NumberFormat::C90_LITERAL,
                NumberFormat::C90_STRING,
                NumberFormat::C89_LITERAL,
                NumberFormat::C89_STRING,
                NumberFormat::RUBY_LITERAL,
                NumberFormat::RUBY_STRING,
                NumberFormat::SWIFT_LITERAL,
                NumberFormat::SWIFT_STRING,
                NumberFormat::GO_LITERAL,
                NumberFormat::GO_STRING,
                NumberFormat::HASKELL_LITERAL,
                NumberFormat::HASKELL_STRING,
                NumberFormat::JAVASCRIPT_LITERAL,
                NumberFormat::JAVASCRIPT_STRING,
                NumberFormat::PERL_LITERAL,
                NumberFormat::PERL_STRING,
                NumberFormat::PHP_LITERAL,
                NumberFormat::PHP_STRING,
                NumberFormat::JAVA_LITERAL,
                NumberFormat::JAVA_STRING,
                NumberFormat::R_LITERAL,
                NumberFormat::R_STRING,
                NumberFormat::KOTLIN_LITERAL,
                NumberFormat::KOTLIN_STRING,
                NumberFormat::JULIA_LITERAL,
                NumberFormat::JULIA_STRING,
                NumberFormat::CSHARP7_LITERAL,
                NumberFormat::CSHARP7_STRING,
                NumberFormat::CSHARP6_LITERAL,
                NumberFormat::CSHARP6_STRING,
                NumberFormat::CSHARP5_LITERAL,
                NumberFormat::CSHARP5_STRING,
                NumberFormat::CSHARP4_LITERAL,
                NumberFormat::CSHARP4_STRING,
                NumberFormat::CSHARP3_LITERAL,
                NumberFormat::CSHARP3_STRING,
                NumberFormat::CSHARP2_LITERAL,
                NumberFormat::CSHARP2_STRING,
                NumberFormat::CSHARP1_LITERAL,
                NumberFormat::CSHARP1_STRING,
                NumberFormat::KAWA_LITERAL,
                NumberFormat::KAWA_STRING,
                NumberFormat::GAMBITC_LITERAL,
                NumberFormat::GAMBITC_STRING,
                NumberFormat::GUILE_LITERAL,
                NumberFormat::GUILE_STRING,
                NumberFormat::CLOJURE_LITERAL,
                NumberFormat::CLOJURE_STRING,
                NumberFormat::ERLANG_LITERAL,
                NumberFormat::ERLANG_STRING,
                NumberFormat::ELM_LITERAL,
                NumberFormat::ELM_STRING,
                NumberFormat::SCALA_LITERAL,
                NumberFormat::SCALA_STRING,
                NumberFormat::ELIXIR_LITERAL,
                NumberFormat::ELIXIR_STRING,
                NumberFormat::FORTRAN_LITERAL,
                NumberFormat::FORTRAN_STRING,
                NumberFormat::D_LITERAL,
                NumberFormat::D_STRING,
                NumberFormat::COFFEESCRIPT_LITERAL,
                NumberFormat::COFFEESCRIPT_STRING,
                NumberFormat::COBOL_LITERAL,
                NumberFormat::COBOL_STRING,
                NumberFormat::FSHARP_LITERAL,
                NumberFormat::FSHARP_STRING,
                NumberFormat::VB_LITERAL,
                NumberFormat::VB_STRING,
                NumberFormat::OCAML_LITERAL,
                NumberFormat::OCAML_STRING,
                NumberFormat::OBJECTIVEC_LITERAL,
                NumberFormat::OBJECTIVEC_STRING,
                NumberFormat::REASONML_LITERAL,
                NumberFormat::REASONML_STRING,
                NumberFormat::OCTAVE_LITERAL,
                NumberFormat::OCTAVE_STRING,
                NumberFormat::MATLAB_LITERAL,
                NumberFormat::MATLAB_STRING,
                NumberFormat::ZIG_LITERAL,
                NumberFormat::ZIG_STRING,
                NumberFormat::SAGE_LITERAL,
                NumberFormat::SAGE_STRING,
                NumberFormat::JSON,
                NumberFormat::TOML,
                NumberFormat::YAML,
                NumberFormat::XML,
                NumberFormat::SQLITE,
                NumberFormat::POSTGRESQL,
                NumberFormat::MYSQL,
                NumberFormat::MONGODB
            ];
            for &flag in flags.iter() {
                // Just wanna check the flags are defined.
                assert!((flag.bits == 0) | true);
                assert!((flag.digit_separator() == 0) | true);
            }
        }
    }
}}   // cfg_if
