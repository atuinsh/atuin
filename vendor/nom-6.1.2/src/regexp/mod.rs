//! Parser combinators that use regular expressions.

mod macros;

///Regular expression parser combinators for strings.
pub mod str {
  use crate::error::{ErrorKind, ParseError};
  use crate::lib::regex::Regex;
  #[cfg(feature = "alloc")]
  use crate::lib::std::vec::Vec;
  use crate::traits::{InputLength, Slice};
  use crate::{Err, IResult};

  /// Compares the input with a regular expression and returns the
  /// whole input if a match is found.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::str::re_match;
  /// # fn main() {
  /// let re = regex::Regex::new(r"^\d{4}").unwrap();
  /// let parser = re_match::<(&str, ErrorKind)>(re);
  /// assert_eq!(parser("2019"), Ok(("", "2019")));
  /// assert_eq!(parser("abc"), Err(Err::Error(("abc", ErrorKind::RegexpMatch))));
  /// assert_eq!(parser("2019-10"), Ok(("", "2019-10")));
  /// # }
  /// ```
  #[cfg(feature = "regexp")]
  #[cfg_attr(feature = "docsrs", doc(cfg(feature = "regexp")))]
  pub fn re_match<'a, E>(re: Regex) -> impl Fn(&'a str) -> IResult<&'a str, &'a str, E>
  where
    E: ParseError<&'a str>,
  {
    move |i| {
      if re.is_match(i) {
        Ok((i.slice(i.input_len()..), i))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpMatch)))
      }
    }
  }

  /// Compares the input with a regular expression and returns all matches in a `Vec`.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::str::re_matches;
  /// # fn main() {
  /// let re = regex::Regex::new(r"a\d").unwrap();
  /// let parser = re_matches::<(&str, ErrorKind)>(re);
  /// assert_eq!(parser("a1ba2"), Ok(("", vec!["a1", "a2"])));
  /// assert_eq!(parser("abc"), Err(Err::Error(("abc", ErrorKind::RegexpMatches))));
  /// # }
  /// ```
  #[cfg(all(feature = "regexp", feature = "alloc"))]
  #[cfg_attr(
    feature = "docsrs",
    doc(cfg(all(feature = "regexp", feature = "alloc")))
  )]
  pub fn re_matches<'a, E>(re: Regex) -> impl Fn(&'a str) -> IResult<&'a str, Vec<&'a str>, E>
  where
    E: ParseError<&'a str>,
  {
    move |i| {
      let v: Vec<_> = re
        .find_iter(i)
        .map(|m| i.slice(m.start()..m.end()))
        .collect();
      if !v.is_empty() {
        let offset = {
          let end = v.last().unwrap();
          end.as_ptr() as usize + end.len() - i.as_ptr() as usize
        };
        Ok((i.slice(offset..), v))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpMatches)))
      }
    }
  }

  /// Compares the input with a regular expression and returns the
  /// first match.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::str::re_find;
  /// # fn main() {
  /// let re = regex::Regex::new(r"\d{4}").unwrap();
  /// let parser = re_find::<(&str, ErrorKind)>(re);
  /// assert_eq!(parser("abc2019"), Ok(("", "2019")));
  /// assert_eq!(parser("abc"), Err(Err::Error(("abc", ErrorKind::RegexpFind))));
  /// assert_eq!(parser("2019-10"), Ok(("-10", "2019")));
  /// # }
  /// ```
  #[cfg(feature = "regexp")]
  #[cfg_attr(feature = "docsrs", doc(cfg(feature = "regexp")))]
  pub fn re_find<'a, E>(re: Regex) -> impl Fn(&'a str) -> IResult<&'a str, &'a str, E>
  where
    E: ParseError<&'a str>,
  {
    move |i| {
      if let Some(m) = re.find(i) {
        Ok((i.slice(m.end()..), i.slice(m.start()..m.end())))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpFind)))
      }
    }
  }

  /// Compares the input with a regular expression and returns
  /// the capture groups of the first match in a `Vec`.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::str::re_capture;
  /// # fn main() {
  /// let re = regex::Regex::new(r"(a)(\d)").unwrap();
  /// let parser = re_capture::<(&str, ErrorKind)>(re);
  /// assert_eq!(parser("a1ba2"), Ok(("ba2", vec!["a1", "a", "1"])));
  /// assert_eq!(parser("abc"), Err(Err::Error(("abc", ErrorKind::RegexpCapture))));
  /// # }
  /// ```
  #[cfg(all(feature = "regexp", feature = "alloc"))]
  #[cfg_attr(
    feature = "docsrs",
    doc(cfg(all(feature = "regexp", feature = "alloc")))
  )]
  pub fn re_capture<'a, E>(re: Regex) -> impl Fn(&'a str) -> IResult<&'a str, Vec<&'a str>, E>
  where
    E: ParseError<&'a str>,
  {
    move |i| {
      if let Some(c) = re.captures(i) {
        let v: Vec<_> = c
          .iter()
          .filter(|el| el.is_some())
          .map(|el| el.unwrap())
          .map(|m| i.slice(m.start()..m.end()))
          .collect();
        let offset = {
          let end = v.last().unwrap();
          end.as_ptr() as usize + end.len() - i.as_ptr() as usize
        };
        Ok((i.slice(offset..), v))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpCapture)))
      }
    }
  }

  /// Compares the input with a regular expression and returns
  /// the capture groups of all matches in a nested `Vec`.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::str::re_captures;
  /// # fn main() {
  /// let re = regex::Regex::new(r"(a)(\d)").unwrap();
  /// let parser = re_captures::<(&str, ErrorKind)>(re);
  /// assert_eq!(parser("a1ba2"), Ok(("", vec![vec!["a1", "a", "1"], vec!["a2", "a", "2"]])));
  /// assert_eq!(parser("abc"), Err(Err::Error(("abc", ErrorKind::RegexpCapture))));
  /// # }
  /// ```
  #[cfg(all(feature = "regexp", feature = "alloc"))]
  #[cfg_attr(
    feature = "docsrs",
    doc(cfg(all(feature = "regexp", feature = "alloc")))
  )]
  pub fn re_captures<'a, E>(re: Regex) -> impl Fn(&'a str) -> IResult<&'a str, Vec<Vec<&'a str>>, E>
  where
    E: ParseError<&'a str>,
  {
    move |i| {
      let v: Vec<Vec<_>> = re
        .captures_iter(i)
        .map(|c| {
          c.iter()
            .filter(|el| el.is_some())
            .map(|el| el.unwrap())
            .map(|m| i.slice(m.start()..m.end()))
            .collect()
        })
        .collect();
      if !v.is_empty() {
        let offset = {
          let end = v.last().unwrap().last().unwrap();
          end.as_ptr() as usize + end.len() - i.as_ptr() as usize
        };
        Ok((i.slice(offset..), v))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpCapture)))
      }
    }
  }

  #[cfg(test)]
  mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::internal::Err;
    use crate::lib::regex::Regex;

    macro_rules! assert_parse(
    ($left: expr, $right: expr) => {
      let res: $crate::IResult<_, _, (_, ErrorKind)> = $left;
      assert_eq!(res, $right);
    };
  );

    #[test]
    fn re_match_str() {
      let re = Regex::new(r"^\d{4}-\d{2}-\d{2}").unwrap();
      let rm = re_match(re);
      assert_parse!(rm("2015-09-07"), Ok(("", "2015-09-07")));
      assert_eq!(
        rm("blah"),
        Err(Err::Error((&"blah"[..], ErrorKind::RegexpMatch)))
      );
      assert_eq!(rm("2015-09-07blah"), Ok(("", "2015-09-07blah")));
    }

    #[test]
    fn re_find_str() {
      let re = Regex::new(r"^\d{4}-\d{2}-\d{2}").unwrap();
      let rm = re_find(re);
      assert_parse!(rm("2015-09-07"), Ok(("", "2015-09-07")));
      assert_eq!(
        rm("blah"),
        Err(Err::Error((&"blah"[..], ErrorKind::RegexpFind)))
      );
      assert_eq!(rm("2015-09-07blah"), Ok(("blah", "2015-09-07")));
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn re_matches_str() {
      let re = Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
      let rm = re_matches(re);
      assert_parse!(rm("2015-09-07"), Ok(("", vec!["2015-09-07"])));
      assert_eq!(
        rm("blah"),
        Err(Err::Error((&"blah"[..], ErrorKind::RegexpMatches)))
      );
      assert_eq!(
        rm("aaa2015-09-07blah2015-09-09pouet"),
        Ok(("pouet", vec!["2015-09-07", "2015-09-09"]))
      );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn re_capture_str() {
      let re = Regex::new(r"([[:alpha:]]+)\s+((\d+).(\d+).(\d+))").unwrap();
      let rm = re_capture(re);
      assert_parse!(
        rm("blah nom 0.3.11pouet"),
        Ok(("pouet", vec!["nom 0.3.11", "nom", "0.3.11", "0", "3", "11"]))
      );
      assert_eq!(
        rm("blah"),
        Err(Err::Error(("blah", ErrorKind::RegexpCapture)))
      );
      assert_eq!(
        rm("hello nom 0.3.11 world regex 0.1.41"),
        Ok((
          " world regex 0.1.41",
          vec!["nom 0.3.11", "nom", "0.3.11", "0", "3", "11"]
        ))
      );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn re_captures_str() {
      let re = Regex::new(r"([[:alpha:]]+)\s+((\d+).(\d+).(\d+))").unwrap();
      let rm = re_captures(re);
      assert_parse!(
        rm("blah nom 0.3.11pouet"),
        Ok((
          "pouet",
          vec![vec!["nom 0.3.11", "nom", "0.3.11", "0", "3", "11"]]
        ))
      );
      assert_eq!(
        rm("blah"),
        Err(Err::Error((&"blah"[..], ErrorKind::RegexpCapture)))
      );
      assert_eq!(
        rm("hello nom 0.3.11 world regex 0.1.41 aaa"),
        Ok((
          " aaa",
          vec![
            vec!["nom 0.3.11", "nom", "0.3.11", "0", "3", "11"],
            vec!["regex 0.1.41", "regex", "0.1.41", "0", "1", "41"],
          ]
        ))
      );
    }
  }
}

///Regular expression parser combinators for bytes.
pub mod bytes {
  use crate::error::{ErrorKind, ParseError};
  use crate::lib::regex::bytes::Regex;
  #[cfg(feature = "alloc")]
  use crate::lib::std::vec::Vec;
  use crate::traits::{InputLength, Slice};
  use crate::{Err, IResult};

  /// Compares the input with a regular expression and returns the
  /// whole input if a match is found.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::bytes::re_match;
  /// # fn main() {
  /// let re = regex::bytes::Regex::new(r"^\d{4}").unwrap();
  /// let parser = re_match::<(&[u8], ErrorKind)>(re);
  /// assert_eq!(parser(&b"2019"[..]), Ok((&b""[..], &b"2019"[..])));
  /// assert_eq!(parser(&b"abc"[..]), Err(Err::Error((&b"abc"[..], ErrorKind::RegexpMatch))));
  /// assert_eq!(parser(&b"2019-10"[..]), Ok((&b""[..], &b"2019-10"[..])));
  /// # }
  /// ```
  #[cfg(feature = "regexp")]
  pub fn re_match<'a, E>(re: Regex) -> impl Fn(&'a [u8]) -> IResult<&'a [u8], &'a [u8], E>
  where
    E: ParseError<&'a [u8]>,
  {
    move |i| {
      if re.is_match(i) {
        Ok((i.slice(i.input_len()..), i))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpMatch)))
      }
    }
  }

  /// Compares the input with a regular expression and returns all matches in a `Vec`.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::bytes::re_matches;
  /// # fn main() {
  /// let re = regex::bytes::Regex::new(r"a\d").unwrap();
  /// let parser = re_matches::<(&[u8], ErrorKind)>(re);
  /// assert_eq!(parser(&b"a1ba2"[..]), Ok((&b""[..], vec![&b"a1"[..], &b"a2"[..]])));
  /// assert_eq!(parser(&b"abc"[..]), Err(Err::Error((&b"abc"[..], ErrorKind::RegexpMatches))));
  /// # }
  /// ```
  #[cfg(all(feature = "regexp", feature = "alloc"))]
  #[cfg_attr(
    feature = "docsrs",
    doc(cfg(all(feature = "regexp", feature = "alloc")))
  )]
  pub fn re_matches<'a, E>(re: Regex) -> impl Fn(&'a [u8]) -> IResult<&'a [u8], Vec<&'a [u8]>, E>
  where
    E: ParseError<&'a [u8]>,
  {
    move |i| {
      let v: Vec<_> = re
        .find_iter(i)
        .map(|m| i.slice(m.start()..m.end()))
        .collect();
      if !v.is_empty() {
        let offset = {
          let end = v.last().unwrap();
          end.as_ptr() as usize + end.len() - i.as_ptr() as usize
        };
        Ok((i.slice(offset..), v))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpMatches)))
      }
    }
  }

  /// Compares the input with a regular expression and returns the
  /// first match.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::bytes::re_find;
  /// # fn main() {
  /// let re = regex::bytes::Regex::new(r"\d{4}").unwrap();
  /// let parser = re_find::<(&[u8], ErrorKind)>(re);
  /// assert_eq!(parser(&b"abc2019"[..]), Ok((&b""[..], &b"2019"[..])));
  /// assert_eq!(parser(&b"abc"[..]), Err(Err::Error((&b"abc"[..], ErrorKind::RegexpFind))));
  /// assert_eq!(parser(&b"2019-10"[..]), Ok((&b"-10"[..], &b"2019"[..])));
  /// # }
  /// ```
  #[cfg(feature = "regexp")]
  pub fn re_find<'a, E>(re: Regex) -> impl Fn(&'a [u8]) -> IResult<&'a [u8], &'a [u8], E>
  where
    E: ParseError<&'a [u8]>,
  {
    move |i| {
      if let Some(m) = re.find(i) {
        Ok((i.slice(m.end()..), i.slice(m.start()..m.end())))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpFind)))
      }
    }
  }

  /// Compares the input with a regular expression and returns
  /// the capture groups of the first match in a `Vec`.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::bytes::re_capture;
  /// # fn main() {
  /// let re = regex::bytes::Regex::new(r"(a)(\d)").unwrap();
  /// let parser = re_capture::<(&[u8], ErrorKind)>(re);
  /// assert_eq!(parser(&b"a1ba2"[..]), Ok((&b"ba2"[..], vec![&b"a1"[..], &b"a"[..], &b"1"[..]])));
  /// assert_eq!(parser(&b"abc"[..]), Err(Err::Error((&b"abc"[..], ErrorKind::RegexpCapture))));
  /// # }
  /// ```
  #[cfg(all(feature = "regexp", feature = "alloc"))]
  #[cfg_attr(
    feature = "docsrs",
    doc(cfg(all(feature = "regexp", feature = "alloc")))
  )]
  pub fn re_capture<'a, E>(re: Regex) -> impl Fn(&'a [u8]) -> IResult<&'a [u8], Vec<&'a [u8]>, E>
  where
    E: ParseError<&'a [u8]>,
  {
    move |i| {
      if let Some(c) = re.captures(i) {
        let v: Vec<_> = c
          .iter()
          .filter(|el| el.is_some())
          .map(|el| el.unwrap())
          .map(|m| i.slice(m.start()..m.end()))
          .collect();
        let offset = {
          let end = v.last().unwrap();
          end.as_ptr() as usize + end.len() - i.as_ptr() as usize
        };
        Ok((i.slice(offset..), v))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpCapture)))
      }
    }
  }

  /// Compares the input with a regular expression and returns
  /// the capture groups of all matches in a nested `Vec`.
  ///
  /// Requires the `regexp` feature.
  /// # Example
  ///
  /// ```
  /// # use nom::{Err, error::ErrorKind};
  /// # use nom::regexp::bytes::re_captures;
  /// # fn main() {
  /// let re = regex::bytes::Regex::new(r"(a)(\d)").unwrap();
  /// let parser = re_captures::<(&[u8], ErrorKind)>(re);
  /// assert_eq!(parser(&b"a1ba2"[..]), Ok((&b""[..], vec![vec![&b"a1"[..], &b"a"[..], &b"1"[..]], vec![&b"a2"[..], &b"a"[..], &b"2"[..]]])));
  /// assert_eq!(parser(&b"abc"[..]), Err(Err::Error((&b"abc"[..], ErrorKind::RegexpCapture))));
  /// # }
  /// ```
  #[cfg(all(feature = "regexp", feature = "alloc"))]
  #[cfg_attr(
    feature = "docsrs",
    doc(cfg(all(feature = "regexp", feature = "alloc")))
  )]
  pub fn re_captures<'a, E>(
    re: Regex,
  ) -> impl Fn(&'a [u8]) -> IResult<&'a [u8], Vec<Vec<&'a [u8]>>, E>
  where
    E: ParseError<&'a [u8]>,
  {
    move |i| {
      let v: Vec<Vec<_>> = re
        .captures_iter(i)
        .map(|c| {
          c.iter()
            .filter(|el| el.is_some())
            .map(|el| el.unwrap())
            .map(|m| i.slice(m.start()..m.end()))
            .collect()
        })
        .collect();
      if !v.is_empty() {
        let offset = {
          let end = v.last().unwrap().last().unwrap();
          end.as_ptr() as usize + end.len() - i.as_ptr() as usize
        };
        Ok((i.slice(offset..), v))
      } else {
        Err(Err::Error(E::from_error_kind(i, ErrorKind::RegexpCapture)))
      }
    }
  }

  #[cfg(test)]
  mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::internal::Err;
    use crate::lib::regex::bytes::Regex;

    macro_rules! assert_parse(
    ($left: expr, $right: expr) => {
      let res: $crate::IResult<_, _, (_, ErrorKind)> = $left;
      assert_eq!(res, $right);
    };
  );

    #[test]
    fn re_match_bytes() {
      let re = Regex::new(r"^\d{4}-\d{2}-\d{2}").unwrap();
      let rm = re_match(re);
      assert_parse!(rm(&b"2015-09-07"[..]), Ok((&b""[..], &b"2015-09-07"[..])));
      assert_eq!(
        rm(&b"blah"[..]),
        Err(Err::Error((&b"blah"[..], ErrorKind::RegexpMatch)))
      );
      assert_eq!(
        rm(&b"2015-09-07blah"[..]),
        Ok((&b""[..], &b"2015-09-07blah"[..]))
      );
    }

    #[test]
    fn re_find_bytes() {
      let re = Regex::new(r"^\d{4}-\d{2}-\d{2}").unwrap();
      let rm = re_find(re);
      assert_parse!(rm(&b"2015-09-07"[..]), Ok((&b""[..], &b"2015-09-07"[..])));
      assert_eq!(
        rm(&b"blah"[..]),
        Err(Err::Error((&b"blah"[..], ErrorKind::RegexpFind)))
      );
      assert_eq!(
        rm(&b"2015-09-07blah"[..]),
        Ok((&b"blah"[..], &b"2015-09-07"[..]))
      );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn re_matches_bytes() {
      let re = Regex::new(r"\d{4}-\d{2}-\d{2}").unwrap();
      let rm = re_matches(re);
      assert_parse!(
        rm(&b"2015-09-07"[..]),
        Ok((&b""[..], vec![&b"2015-09-07"[..]]))
      );
      assert_eq!(
        rm(&b"blah"[..]),
        Err(Err::Error((&b"blah"[..], ErrorKind::RegexpMatches)))
      );
      assert_eq!(
        rm(&b"aaa2015-09-07blah2015-09-09pouet"[..]),
        Ok((&b"pouet"[..], vec![&b"2015-09-07"[..], &b"2015-09-09"[..]]))
      );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn re_capture_bytes() {
      let re = Regex::new(r"([[:alpha:]]+)\s+((\d+).(\d+).(\d+))").unwrap();
      let rm = re_capture(re);
      assert_parse!(
        rm(&b"blah nom 0.3.11pouet"[..]),
        Ok((
          &b"pouet"[..],
          vec![
            &b"nom 0.3.11"[..],
            &b"nom"[..],
            &b"0.3.11"[..],
            &b"0"[..],
            &b"3"[..],
            &b"11"[..]
          ]
        ))
      );
      assert_eq!(
        rm(&b"blah"[..]),
        Err(Err::Error((&b"blah"[..], ErrorKind::RegexpCapture)))
      );
      assert_eq!(
        rm(&b"hello nom 0.3.11 world regex 0.1.41"[..]),
        Ok((
          &b" world regex 0.1.41"[..],
          vec![
            &b"nom 0.3.11"[..],
            &b"nom"[..],
            &b"0.3.11"[..],
            &b"0"[..],
            &b"3"[..],
            &b"11"[..]
          ]
        ))
      );
    }

    #[cfg(feature = "alloc")]
    #[test]
    fn re_captures_bytes() {
      let re = Regex::new(r"([[:alpha:]]+)\s+((\d+).(\d+).(\d+))").unwrap();
      let rm = re_captures(re);
      assert_parse!(
        rm(&b"blah nom 0.3.11pouet"[..]),
        Ok((
          &b"pouet"[..],
          vec![vec![
            &b"nom 0.3.11"[..],
            &b"nom"[..],
            &b"0.3.11"[..],
            &b"0"[..],
            &b"3"[..],
            &b"11"[..]
          ]]
        ))
      );

      assert_eq!(
        rm(&b"blah"[..]),
        Err(Err::Error((&b"blah"[..], ErrorKind::RegexpCapture)))
      );
      assert_eq!(
        rm(&b"hello nom 0.3.11 world regex 0.1.41 aaa"[..]),
        Ok((
          &b" aaa"[..],
          vec![
            vec![
              &b"nom 0.3.11"[..],
              &b"nom"[..],
              &b"0.3.11"[..],
              &b"0"[..],
              &b"3"[..],
              &b"11"[..]
            ],
            vec![
              &b"regex 0.1.41"[..],
              &b"regex"[..],
              &b"0.1.41"[..],
              &b"0"[..],
              &b"1"[..],
              &b"41"[..]
            ],
          ]
        ))
      );
    }
  }
}
