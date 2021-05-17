#[cfg(test)]
mod test {
  use crate::{error, error::ErrorKind, Err, IResult};

  #[test]
  fn tagtr_succeed() {
    const INPUT: &str = "Hello World!";
    const TAG: &str = "Hello";
    fn test(input: &str) -> IResult<&str, &str> {
      tag!(input, TAG)
    }

    match test(INPUT) {
      Ok((extra, output)) => {
        assert!(extra == " World!", "Parser `tag` consumed leftover input.");
        assert!(
          output == TAG,
          "Parser `tag` doesn't return the tag it matched on success. \
           Expected `{}`, got `{}`.",
          TAG,
          output
        );
      }
      other => panic!(
        "Parser `tag` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn tagtr_incomplete() {
    const INPUT: &str = "Hello";
    const TAG: &str = "Hello World!";

    let res: IResult<_, _, error::Error<_>> = tag!(INPUT, TAG);
    match res {
      Err(Err::Incomplete(_)) => (),
      other => {
        panic!(
          "Parser `tag` didn't require more input when it should have. \
           Got `{:?}`.",
          other
        );
      }
    };
  }

  #[test]
  fn tagtr_error() {
    const INPUT: &str = "Hello World!";
    const TAG: &str = "Random"; // TAG must be closer than INPUT.

    let res: IResult<_, _, error::Error<_>> = tag!(INPUT, TAG);
    match res {
      Err(Err::Error(_)) => (),
      other => {
        panic!(
          "Parser `tag` didn't fail when it should have. Got `{:?}`.`",
          other
        );
      }
    };
  }

  #[test]
  fn take_s_succeed() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const CONSUMED: &str = "βèƒôřèÂßÇ";
    const LEFTOVER: &str = "áƒƭèř";

    let res: IResult<_, _, error::Error<_>> = take!(INPUT, 9);
    match res {
      Ok((extra, output)) => {
        assert!(
          extra == LEFTOVER,
          "Parser `take_s` consumed leftover input. Leftover `{}`.",
          extra
        );
        assert!(
          output == CONSUMED,
          "Parser `take_s` doens't return the string it consumed on success. Expected `{}`, got `{}`.",
          CONSUMED,
          output
        );
      }
      other => panic!(
        "Parser `take_s` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn take_until_succeed() {
    const INPUT: &str = "βèƒôřèÂßÇ∂áƒƭèř";
    const FIND: &str = "ÂßÇ∂";
    const CONSUMED: &str = "βèƒôřè";
    const LEFTOVER: &str = "ÂßÇ∂áƒƭèř";

    let res: IResult<_, _, (_, ErrorKind)> = take_until!(INPUT, FIND);
    match res {
      Ok((extra, output)) => {
        assert!(
          extra == LEFTOVER,
          "Parser `take_until`\
           consumed leftover input. Leftover `{}`.",
          extra
        );
        assert!(
          output == CONSUMED,
          "Parser `take_until`\
           doens't return the string it consumed on success. Expected `{}`, got `{}`.",
          CONSUMED,
          output
        );
      }
      other => panic!(
        "Parser `take_until` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn take_s_incomplete() {
    const INPUT: &str = "βèƒôřèÂßÇá";

    let res: IResult<_, _, (_, ErrorKind)> = take!(INPUT, 13);
    match res {
      Err(Err::Incomplete(_)) => (),
      other => panic!(
        "Parser `take` didn't require more input when it should have. \
         Got `{:?}`.",
        other
      ),
    }
  }

  use crate::internal::Needed;

  fn is_alphabetic(c: char) -> bool {
    (c as u8 >= 0x41 && c as u8 <= 0x5A) || (c as u8 >= 0x61 && c as u8 <= 0x7A)
  }

  #[test]
  fn take_while() {
    named!(f<&str,&str>, take_while!(is_alphabetic));
    let a = "";
    let b = "abcd";
    let c = "abcd123";
    let d = "123";

    assert_eq!(f(&a[..]), Err(Err::Incomplete(Needed::new(1))));
    assert_eq!(f(&b[..]), Err(Err::Incomplete(Needed::new(1))));
    assert_eq!(f(&c[..]), Ok((&d[..], &b[..])));
    assert_eq!(f(&d[..]), Ok((&d[..], &a[..])));
  }

  #[test]
  fn take_while1() {
    named!(f<&str,&str>, take_while1!(is_alphabetic));
    let a = "";
    let b = "abcd";
    let c = "abcd123";
    let d = "123";

    assert_eq!(f(&a[..]), Err(Err::Incomplete(Needed::new(1))));
    assert_eq!(f(&b[..]), Err(Err::Incomplete(Needed::new(1))));
    assert_eq!(f(&c[..]), Ok((&"123"[..], &b[..])));
    assert_eq!(
      f(&d[..]),
      Err(Err::Error(error_position!(&d[..], ErrorKind::TakeWhile1)))
    );
  }

  #[test]
  fn take_till_s_succeed() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const CONSUMED: &str = "βèƒôřèÂßÇ";
    const LEFTOVER: &str = "áƒƭèř";
    fn till_s(c: char) -> bool {
      c == 'á'
    }
    fn test(input: &str) -> IResult<&str, &str> {
      take_till!(input, till_s)
    }
    match test(INPUT) {
      Ok((extra, output)) => {
        assert!(
          extra == LEFTOVER,
          "Parser `take_till` consumed leftover input."
        );
        assert!(
          output == CONSUMED,
          "Parser `take_till` doesn't return the string it consumed on success. \
           Expected `{}`, got `{}`.",
          CONSUMED,
          output
        );
      }
      other => panic!(
        "Parser `take_till` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn take_while_succeed_none() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const CONSUMED: &str = "";
    const LEFTOVER: &str = "βèƒôřèÂßÇáƒƭèř";
    fn while_s(c: char) -> bool {
      c == '9'
    }
    fn test(input: &str) -> IResult<&str, &str> {
      take_while!(input, while_s)
    }
    match test(INPUT) {
      Ok((extra, output)) => {
        assert!(
          extra == LEFTOVER,
          "Parser `take_while` consumed leftover input."
        );
        assert!(
          output == CONSUMED,
          "Parser `take_while` doesn't return the string it consumed on success. \
           Expected `{}`, got `{}`.",
          CONSUMED,
          output
        );
      }
      other => panic!(
        "Parser `take_while` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn is_not_succeed() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const AVOID: &str = "£úçƙ¥á";
    const CONSUMED: &str = "βèƒôřèÂßÇ";
    const LEFTOVER: &str = "áƒƭèř";
    fn test(input: &str) -> IResult<&str, &str> {
      is_not!(input, AVOID)
    }
    match test(INPUT) {
      Ok((extra, output)) => {
        assert!(
          extra == LEFTOVER,
          "Parser `is_not` consumed leftover input. Leftover `{}`.",
          extra
        );
        assert!(
          output == CONSUMED,
          "Parser `is_not` doens't return the string it consumed on success. Expected `{}`, got `{}`.",
          CONSUMED,
          output
        );
      }
      other => panic!(
        "Parser `is_not` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn take_while_succeed_some() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const CONSUMED: &str = "βèƒôřèÂßÇ";
    const LEFTOVER: &str = "áƒƭèř";
    fn while_s(c: char) -> bool {
      c == 'β'
        || c == 'è'
        || c == 'ƒ'
        || c == 'ô'
        || c == 'ř'
        || c == 'è'
        || c == 'Â'
        || c == 'ß'
        || c == 'Ç'
    }
    fn test(input: &str) -> IResult<&str, &str> {
      take_while!(input, while_s)
    }
    match test(INPUT) {
      Ok((extra, output)) => {
        assert!(
          extra == LEFTOVER,
          "Parser `take_while` consumed leftover input."
        );
        assert!(
          output == CONSUMED,
          "Parser `take_while` doesn't return the string it consumed on success. \
           Expected `{}`, got `{}`.",
          CONSUMED,
          output
        );
      }
      other => panic!(
        "Parser `take_while` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn is_not_fail() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const AVOID: &str = "βúçƙ¥";
    fn test(input: &str) -> IResult<&str, &str> {
      is_not!(input, AVOID)
    }
    match test(INPUT) {
      Err(Err::Error(_)) => (),
      other => panic!(
        "Parser `is_not` didn't fail when it should have. Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn take_while1_succeed() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const CONSUMED: &str = "βèƒôřèÂßÇ";
    const LEFTOVER: &str = "áƒƭèř";
    fn while1_s(c: char) -> bool {
      c == 'β'
        || c == 'è'
        || c == 'ƒ'
        || c == 'ô'
        || c == 'ř'
        || c == 'è'
        || c == 'Â'
        || c == 'ß'
        || c == 'Ç'
    }
    fn test(input: &str) -> IResult<&str, &str> {
      take_while1!(input, while1_s)
    }
    match test(INPUT) {
      Ok((extra, output)) => {
        assert!(
          extra == LEFTOVER,
          "Parser `take_while1` consumed leftover input."
        );
        assert!(
          output == CONSUMED,
          "Parser `take_while1` doesn't return the string it consumed on success. \
           Expected `{}`, got `{}`.",
          CONSUMED,
          output
        );
      }
      other => panic!(
        "Parser `take_while1` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn take_until_incomplete() {
    const INPUT: &str = "βèƒôřè";
    const FIND: &str = "βèƒôřèÂßÇ";

    let res: IResult<_, _, (_, ErrorKind)> = take_until!(INPUT, FIND);
    match res {
      Err(Err::Incomplete(_)) => (),
      other => panic!(
        "Parser `take_until` didn't require more input when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn is_a_succeed() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const MATCH: &str = "βèƒôřèÂßÇ";
    const CONSUMED: &str = "βèƒôřèÂßÇ";
    const LEFTOVER: &str = "áƒƭèř";
    fn test(input: &str) -> IResult<&str, &str> {
      is_a!(input, MATCH)
    }
    match test(INPUT) {
      Ok((extra, output)) => {
        assert!(
          extra == LEFTOVER,
          "Parser `is_a` consumed leftover input. Leftover `{}`.",
          extra
        );
        assert!(
          output == CONSUMED,
          "Parser `is_a` doens't return the string it consumed on success. Expected `{}`, got `{}`.",
          CONSUMED,
          output
        );
      }
      other => panic!(
        "Parser `is_a` didn't succeed when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn take_while1_fail() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    fn while1_s(c: char) -> bool {
      c == '9'
    }
    fn test(input: &str) -> IResult<&str, &str> {
      take_while1!(input, while1_s)
    }
    match test(INPUT) {
      Err(Err::Error(_)) => (),
      other => panic!(
        "Parser `take_while1` didn't fail when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn is_a_fail() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const MATCH: &str = "Ûñℓúçƙ¥";
    fn test(input: &str) -> IResult<&str, &str> {
      is_a!(input, MATCH)
    }
    match test(INPUT) {
      Err(Err::Error(_)) => (),
      other => panic!(
        "Parser `is_a` didn't fail when it should have. Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  fn take_until_error() {
    const INPUT: &str = "βèƒôřèÂßÇáƒƭèř";
    const FIND: &str = "Ráñδô₥";

    let res: IResult<_, _, (_, ErrorKind)> = take_until!(INPUT, FIND);
    match res {
      Err(Err::Incomplete(_)) => (),
      other => panic!(
        "Parser `take_until` didn't fail when it should have. \
         Got `{:?}`.",
        other
      ),
    };
  }

  #[test]
  #[cfg(feature = "alloc")]
  fn recognize_is_a() {
    let a = "aabbab";
    let b = "ababcd";

    named!(f <&str,&str>, recognize!(many1!(complete!(alt!( tag!("a") | tag!("b") )))));

    assert_eq!(f(&a[..]), Ok((&a[6..], &a[..])));
    assert_eq!(f(&b[..]), Ok((&b[4..], &b[..4])));
  }

  #[test]
  fn utf8_indexing() {
    named!(dot(&str) -> &str,
      tag!(".")
    );

    let _ = dot("點");
  }

  #[cfg(feature = "alloc")]
  #[test]
  fn case_insensitive() {
    named!(test<&str,&str>, tag_no_case!("ABcd"));
    assert_eq!(test("aBCdefgh"), Ok(("efgh", "aBCd")));
    assert_eq!(test("abcdefgh"), Ok(("efgh", "abcd")));
    assert_eq!(test("ABCDefgh"), Ok(("efgh", "ABCD")));

    named!(test2<&str,&str>, tag_no_case!("ABcd"));
    assert_eq!(test2("aBCdefgh"), Ok(("efgh", "aBCd")));
    assert_eq!(test2("abcdefgh"), Ok(("efgh", "abcd")));
    assert_eq!(test2("ABCDefgh"), Ok(("efgh", "ABCD")));
  }
}
