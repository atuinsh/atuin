/// `re_match!(regexp) => &[T] -> IResult<&[T], &[T]>`
/// Returns the whole input if a match is found.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(feature = "docsrs", doc(cfg(feature = "regexp")))]
macro_rules! re_match (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::Regex::new($re).unwrap();
      $crate::regexp::str::re_match(r)($i)
      } )
);

/// `re_bytes_match!(regexp) => &[T] -> IResult<&[T], &[T]>`
/// Returns the whole input if a match is found.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(feature = "docsrs", doc(cfg(feature = "regexp")))]
macro_rules! re_bytes_match (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::bytes::Regex::new($re).unwrap();
      $crate::regexp::bytes::re_match(r)($i)
      } )
);

/// `re_find!(regexp) => &[T] -> IResult<&[T], &[T]>`
/// Returns the first match.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(feature = "docsrs", doc(cfg(feature = "regexp")))]
macro_rules! re_find (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::Regex::new($re).unwrap();
      $crate::regexp::str::re_find(r)($i)
      } )
);

/// `re_bytes_find!(regexp) => &[T] -> IResult<&[T], &[T]>`
/// Returns the first match.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(feature = "docsrs", doc(cfg(feature = "regexp")))]
macro_rules! re_bytes_find (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::bytes::Regex::new($re).unwrap();
      $crate::regexp::bytes::re_find(r)($i)
      } )
);

/// `re_matches!(regexp) => &[T] -> IResult<&[T], Vec<&[T]>>`
/// Returns all the matched parts.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(
  feature = "docsrs",
  doc(cfg(all(feature = "regexp", feature = "alloc")))
)]
macro_rules! re_matches (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::Regex::new($re).unwrap();
      $crate::regexp::str::re_matches(r)($i)
      } )
);

/// `re_bytes_matches!(regexp) => &[T] -> IResult<&[T], Vec<&[T]>>`
/// Returns all the matched parts.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(
  feature = "docsrs",
  doc(cfg(all(feature = "regexp", feature = "alloc")))
)]
macro_rules! re_bytes_matches (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::bytes::Regex::new($re).unwrap();
      $crate::regexp::bytes::re_matches(r)($i)
      } )
);

/// `re_capture!(regexp) => &[T] -> IResult<&[T], Vec<&[T]>>`
/// Returns the capture groups of the first match.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(
  feature = "docsrs",
  doc(cfg(all(feature = "regexp", feature = "alloc")))
)]
macro_rules! re_capture (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::Regex::new($re).unwrap();
      $crate::regexp::str::re_capture(r)($i)
      } )
);

/// `re_bytes_capture!(regexp) => &[T] -> IResult<&[T], Vec<&[T]>>`
/// Returns the capture groups of the first match.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(
  feature = "docsrs",
  doc(cfg(all(feature = "regexp", feature = "alloc")))
)]
macro_rules! re_bytes_capture (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::bytes::Regex::new($re).unwrap();
      $crate::regexp::bytes::re_capture(r)($i)
      }
      )
);

/// `re_captures!(regexp) => &[T] -> IResult<&[T], Vec<Vec<&[T]>>>`
/// Returns the capture groups of all matches.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(
  feature = "docsrs",
  doc(cfg(all(feature = "regexp", feature = "alloc")))
)]
macro_rules! re_captures (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::Regex::new($re).unwrap();
      $crate::regexp::str::re_captures(r)($i)
      } )
);

/// `re_bytes_captures!(regexp) => &[T] -> IResult<&[T], Vec<Vec<&[T]>>>`
/// Returns the capture groups of all matches.
///
/// Requires the `regexp` feature.
#[macro_export(local_inner_macros)]
#[cfg_attr(
  feature = "docsrs",
  doc(cfg(all(feature = "regexp", feature = "alloc")))
)]
macro_rules! re_bytes_captures (
  ($i:expr, $re:expr) => ( {
      let r = $crate::lib::regex::bytes::Regex::new($re).unwrap();
      $crate::regexp::bytes::re_captures(r)($i)
      } )
);

#[cfg(test)]
mod tests {
  use crate::error::ErrorKind;
  use crate::internal::Err;
  #[cfg(feature = "alloc")]
  use crate::lib::std::vec::Vec;

  #[test]
  fn re_match() {
    named!(rm<&str,&str>, re_match!(r"^\d{4}-\d{2}-\d{2}"));
    assert_eq!(rm("2015-09-07"), Ok(("", "2015-09-07")));
    assert_eq!(
      rm("blah"),
      Err(Err::Error(error_position!(
        &"blah"[..],
        ErrorKind::RegexpMatch
      ),))
    );
    assert_eq!(rm("2015-09-07blah"), Ok(("", "2015-09-07blah")));
  }

  #[test]
  fn re_find() {
    named!(rm<&str,&str>, re_find!(r"^\d{4}-\d{2}-\d{2}"));
    assert_eq!(rm("2015-09-07"), Ok(("", "2015-09-07")));
    assert_eq!(
      rm("blah"),
      Err(Err::Error(error_position!(
        &"blah"[..],
        ErrorKind::RegexpFind
      ),))
    );
    assert_eq!(rm("2015-09-07blah"), Ok(("blah", "2015-09-07")));
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn re_matches() {
    named!(rm< &str,Vec<&str> >, re_matches!(r"\d{4}-\d{2}-\d{2}"));
    assert_eq!(rm("2015-09-07"), Ok(("", vec!["2015-09-07"])));
    assert_eq!(
      rm("blah"),
      Err(Err::Error(error_position!(
        &"blah"[..],
        ErrorKind::RegexpMatches
      )))
    );
    assert_eq!(
      rm("aaa2015-09-07blah2015-09-09pouet"),
      Ok(("pouet", vec!["2015-09-07", "2015-09-09"]))
    );
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn re_capture() {
    named!(rm< &str,Vec<&str> >, re_capture!(r"([[:alpha:]]+)\s+((\d+).(\d+).(\d+))"));
    assert_eq!(
      rm("blah nom 0.3.11pouet"),
      Ok(("pouet", vec!["nom 0.3.11", "nom", "0.3.11", "0", "3", "11"]))
    );
    assert_eq!(
      rm("blah"),
      Err(Err::Error(error_position!(
        &"blah"[..],
        ErrorKind::RegexpCapture
      )))
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
  fn re_captures() {
    named!(rm< &str,Vec<Vec<&str>> >, re_captures!(r"([[:alpha:]]+)\s+((\d+).(\d+).(\d+))"));
    assert_eq!(
      rm("blah nom 0.3.11pouet"),
      Ok((
        "pouet",
        vec![vec!["nom 0.3.11", "nom", "0.3.11", "0", "3", "11"]]
      ))
    );
    assert_eq!(
      rm("blah"),
      Err(Err::Error(error_position!(
        &"blah"[..],
        ErrorKind::RegexpCapture
      )))
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

  #[test]
  fn re_bytes_match() {
    named!(rm, re_bytes_match!(r"^\d{4}-\d{2}-\d{2}"));
    assert_eq!(rm(&b"2015-09-07"[..]), Ok((&b""[..], &b"2015-09-07"[..])));
    assert_eq!(
      rm(&b"blah"[..]),
      Err(Err::Error(error_position!(
        &b"blah"[..],
        ErrorKind::RegexpMatch
      )))
    );
    assert_eq!(
      rm(&b"2015-09-07blah"[..]),
      Ok((&b""[..], &b"2015-09-07blah"[..]))
    );
  }

  #[test]
  fn re_bytes_find() {
    named!(rm, re_bytes_find!(r"^\d{4}-\d{2}-\d{2}"));
    assert_eq!(rm(&b"2015-09-07"[..]), Ok((&b""[..], &b"2015-09-07"[..])));
    assert_eq!(
      rm(&b"blah"[..]),
      Err(Err::Error(error_position!(
        &b"blah"[..],
        ErrorKind::RegexpFind
      )))
    );
    assert_eq!(
      rm(&b"2015-09-07blah"[..]),
      Ok((&b"blah"[..], &b"2015-09-07"[..]))
    );
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn re_bytes_matches() {
    named!(rm<Vec<&[u8]>>, re_bytes_matches!(r"\d{4}-\d{2}-\d{2}"));
    assert_eq!(
      rm(&b"2015-09-07"[..]),
      Ok((&b""[..], vec![&b"2015-09-07"[..]]))
    );
    assert_eq!(
      rm(&b"blah"[..]),
      Err(Err::Error(error_position!(
        &b"blah"[..],
        ErrorKind::RegexpMatches
      )))
    );
    assert_eq!(
      rm(&b"aaa2015-09-07blah2015-09-09pouet"[..]),
      Ok((&b"pouet"[..], vec![&b"2015-09-07"[..], &b"2015-09-09"[..]]))
    );
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn re_bytes_capture() {
    named!(
      rm<Vec<&[u8]>>,
      re_bytes_capture!(r"([[:alpha:]]+)\s+((\d+).(\d+).(\d+))")
    );
    assert_eq!(
      rm(&b"blah nom 0.3.11pouet"[..]),
      Ok((
        &b"pouet"[..],
        vec![
          &b"nom 0.3.11"[..],
          &b"nom"[..],
          &b"0.3.11"[..],
          &b"0"[..],
          &b"3"[..],
          &b"11"[..],
        ]
      ))
    );
    assert_eq!(
      rm(&b"blah"[..]),
      Err(Err::Error(error_position!(
        &b"blah"[..],
        ErrorKind::RegexpCapture
      )))
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
          &b"11"[..],
        ]
      ))
    );
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn re_bytes_captures() {
    named!(
      rm<Vec<Vec<&[u8]>>>,
      re_bytes_captures!(r"([[:alpha:]]+)\s+((\d+).(\d+).(\d+))")
    );
    assert_eq!(
      rm(&b"blah nom 0.3.11pouet"[..]),
      Ok((
        &b"pouet"[..],
        vec![vec![
          &b"nom 0.3.11"[..],
          &b"nom"[..],
          &b"0.3.11"[..],
          &b"0"[..],
          &b"3"[..],
          &b"11"[..],
        ],]
      ))
    );
    assert_eq!(
      rm(&b"blah"[..]),
      Err(Err::Error(error_position!(
        &b"blah"[..],
        ErrorKind::RegexpCapture
      )))
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
            &b"11"[..],
          ],
          vec![
            &b"regex 0.1.41"[..],
            &b"regex"[..],
            &b"0.1.41"[..],
            &b"0"[..],
            &b"1"[..],
            &b"41"[..],
          ],
        ]
      ))
    );
  }
}
