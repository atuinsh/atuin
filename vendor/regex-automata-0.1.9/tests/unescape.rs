#[derive(Clone, Copy, Eq, PartialEq)]
enum State {
    /// The state after seeing a `\`.
    Escape,
    /// The state after seeing a `\x`.
    HexFirst,
    /// The state after seeing a `\x[0-9A-Fa-f]`.
    HexSecond(char),
    /// Default state.
    Literal,
}

pub fn unescape(s: &str) -> Vec<u8> {
    use self::State::*;

    let mut bytes = vec![];
    let mut state = Literal;
    for c in s.chars() {
        match state {
            Escape => match c {
                '\\' => {
                    bytes.push(b'\\');
                    state = Literal;
                }
                'n' => {
                    bytes.push(b'\n');
                    state = Literal;
                }
                'r' => {
                    bytes.push(b'\r');
                    state = Literal;
                }
                't' => {
                    bytes.push(b'\t');
                    state = Literal;
                }
                'x' => {
                    state = HexFirst;
                }
                c => {
                    bytes.extend(format!(r"\{}", c).into_bytes());
                    state = Literal;
                }
            },
            HexFirst => match c {
                '0'..='9' | 'A'..='F' | 'a'..='f' => {
                    state = HexSecond(c);
                }
                c => {
                    bytes.extend(format!(r"\x{}", c).into_bytes());
                    state = Literal;
                }
            },
            HexSecond(first) => match c {
                '0'..='9' | 'A'..='F' | 'a'..='f' => {
                    let ordinal = format!("{}{}", first, c);
                    let byte = u8::from_str_radix(&ordinal, 16).unwrap();
                    bytes.push(byte);
                    state = Literal;
                }
                c => {
                    let original = format!(r"\x{}{}", first, c);
                    bytes.extend(original.into_bytes());
                    state = Literal;
                }
            },
            Literal => match c {
                '\\' => {
                    state = Escape;
                }
                c => {
                    bytes.extend(c.to_string().as_bytes());
                }
            },
        }
    }
    match state {
        Escape => bytes.push(b'\\'),
        HexFirst => bytes.extend(b"\\x"),
        HexSecond(c) => bytes.extend(format!("\\x{}", c).into_bytes()),
        Literal => {}
    }
    bytes
}
