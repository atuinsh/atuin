mod enc;
pub use enc::encode;
pub use enc::encode_binary;
pub use enc::Encoded;

mod dec;
pub use dec::decode;
pub use dec::decode_binary;
pub use dec::FromUrlEncodingError;

#[cfg(test)]
mod tests {
    use super::encode;
    use super::decode;
    use crate::dec::from_hex_digit;

    #[test]
    fn it_encodes_successfully() {
        let expected = "this%20that";
        assert_eq!(expected, encode("this that"));
    }

    #[test]
    fn it_encodes_successfully_emoji() {
        let emoji_string = "👾 Exterminate!";
        let expected = "%F0%9F%91%BE%20Exterminate%21";
        assert_eq!(expected, encode(emoji_string));
    }

    #[test]
    fn it_decodes_successfully() {
        let expected = String::from("this that");
        let encoded = "this%20that";
        assert_eq!(expected, decode(encoded).unwrap());
    }

    #[test]
    fn it_decodes_successfully_emoji() {
        let expected = String::from("👾 Exterminate!");
        let encoded = "%F0%9F%91%BE%20Exterminate%21";
        assert_eq!(expected, decode(encoded).unwrap());
    }

    #[test]
    fn it_decodes_unsuccessfully_emoji() {
        let bad_encoded_string = "👾 Exterminate!";

        assert_eq!(bad_encoded_string, decode(bad_encoded_string).unwrap());
    }


    #[test]
    fn misc() {
        assert_eq!(3, from_hex_digit(b'3').unwrap());
        assert_eq!(10, from_hex_digit(b'a').unwrap());
        assert_eq!(15, from_hex_digit(b'F').unwrap());
        assert_eq!(None, from_hex_digit(b'G'));
        assert_eq!(None, from_hex_digit(9));

        assert_eq!("pureascii", encode("pureascii"));
        assert_eq!("pureascii", decode("pureascii").unwrap());
        assert_eq!("", encode(""));
        assert_eq!("", decode("").unwrap());
        assert_eq!("%26a%25b%21c.d%3Fe", encode("&a%b!c.d?e"));
        assert_eq!("%00", encode("\0"));
        assert_eq!("%00x", encode("\0x"));
        assert_eq!("x%00", encode("x\0"));
        assert_eq!("x%00x", encode("x\0x"));
        assert_eq!("aa%00%00bb", encode("aa\0\0bb"));
        assert_eq!("\0", decode("\0").unwrap());
        assert!(decode("%F0%0F%91%BE%20Hello%21").is_err());
        assert_eq!("this that", decode("this%20that").unwrap());
        assert_eq!("this that%", decode("this%20that%").unwrap());
        assert_eq!("this that%2", decode("this%20that%2").unwrap());
        assert_eq!("this that%%", decode("this%20that%%").unwrap());
        assert_eq!("this that%2%", decode("this%20that%2%").unwrap());
        assert_eq!("this%2that", decode("this%2that").unwrap());
        assert_eq!("this%%2that", decode("this%%2that").unwrap());
        assert_eq!("this%2x&that", decode("this%2x%26that").unwrap());
        // assert_eq!("this%2&that", decode("this%2%26that").unwrap());
    }
}
