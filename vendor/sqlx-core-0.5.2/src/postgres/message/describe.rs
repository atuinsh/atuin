use crate::io::Encode;
use crate::postgres::io::PgBufMutExt;

const DESCRIBE_PORTAL: u8 = b'P';
const DESCRIBE_STATEMENT: u8 = b'S';

// [Describe] will emit both a [RowDescription] and a [ParameterDescription] message

#[derive(Debug)]
#[allow(dead_code)]
pub enum Describe {
    UnnamedStatement,
    Statement(u32),

    UnnamedPortal,
    Portal(u32),
}

impl Encode<'_> for Describe {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        // 15 bytes for 1-digit statement/portal IDs
        buf.reserve(20);
        buf.push(b'D');

        buf.put_length_prefixed(|buf| {
            match self {
                // #[likely]
                Describe::Statement(id) => {
                    buf.push(DESCRIBE_STATEMENT);
                    buf.put_statement_name(*id);
                }

                Describe::UnnamedPortal => {
                    buf.push(DESCRIBE_PORTAL);
                    buf.push(0);
                }

                Describe::UnnamedStatement => {
                    buf.push(DESCRIBE_STATEMENT);
                    buf.push(0);
                }

                Describe::Portal(id) => {
                    buf.push(DESCRIBE_PORTAL);
                    buf.put_portal_name(Some(*id));
                }
            }
        });
    }
}

#[test]
fn test_encode_describe_portal() {
    const EXPECTED: &[u8] = b"D\0\0\0\x0EPsqlx_p_5\0";

    let mut buf = Vec::new();
    let m = Describe::Portal(5);

    m.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}

#[test]
fn test_encode_describe_unnamed_portal() {
    const EXPECTED: &[u8] = b"D\0\0\0\x06P\0";

    let mut buf = Vec::new();
    let m = Describe::UnnamedPortal;

    m.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}

#[test]
fn test_encode_describe_statement() {
    const EXPECTED: &[u8] = b"D\0\0\0\x0ESsqlx_s_5\0";

    let mut buf = Vec::new();
    let m = Describe::Statement(5);

    m.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}

#[test]
fn test_encode_describe_unnamed_statement() {
    const EXPECTED: &[u8] = b"D\0\0\0\x06S\0";

    let mut buf = Vec::new();
    let m = Describe::UnnamedStatement;

    m.encode(&mut buf);

    assert_eq!(buf, EXPECTED);
}

#[cfg(all(test, not(debug_assertions)))]
#[bench]
fn bench_encode_describe_portal(b: &mut test::Bencher) {
    use test::black_box;

    let mut buf = Vec::with_capacity(128);

    b.iter(|| {
        buf.clear();

        black_box(Describe::Portal(5)).encode(&mut buf);
    });
}

#[cfg(all(test, not(debug_assertions)))]
#[bench]
fn bench_encode_describe_unnamed_statement(b: &mut test::Bencher) {
    use test::black_box;

    let mut buf = Vec::with_capacity(128);

    b.iter(|| {
        buf.clear();

        black_box(Describe::UnnamedStatement).encode(&mut buf);
    });
}
