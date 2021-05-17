extern crate crc;

mod crc16 {
    use crc::{crc16, Hasher16};

    const X25_CHECK_VALUE: u16 = 0x906e;
    const USB_CHECK_VALUE: u16 = 0xb4c8;

    #[test]
    fn checksum_x25() {
        assert_eq!(crc16::checksum_x25(b"123456789"), X25_CHECK_VALUE)
    }

    #[test]
    fn checksum_usb() {
        assert_eq!(crc16::checksum_usb(b"123456789"), USB_CHECK_VALUE)
    }

    #[test]
    fn digest_x25() {
        verify_checksum(crc16::X25, X25_CHECK_VALUE);
    }

    #[test]
    fn digest_usb() {
        verify_checksum(crc16::USB, USB_CHECK_VALUE);
    }

    fn verify_checksum(poly: u16, check_value: u16) {
        let mut digest = crc16::Digest::new(poly);
        digest.write(b"123456789");
        assert_eq!(digest.sum16(), check_value);
        digest.reset();
        for i in 1..10 {
            digest.write(i.to_string().as_bytes());
        }
        assert_eq!(digest.sum16(), check_value);
    }
}

mod crc32 {
    use crc::{crc32, Hasher32};

    const CASTAGNOLI_CHECK_VALUE: u32 = 0xe3069283;
    const IEEE_CHECK_VALUE: u32 = 0xcbf43926;
    const KOOPMAN_CHECK_VALUE: u32 = 0x2d3dd0ae;

    #[test]
    fn checksum_castagnoli() {
        assert_eq!(crc32::checksum_castagnoli(b"123456789"), CASTAGNOLI_CHECK_VALUE)
    }

    #[test]
    fn checksum_ieee() {
        assert_eq!(crc32::checksum_ieee(b"123456789"), IEEE_CHECK_VALUE)
    }

    #[test]
    fn checksum_koopman() {
        assert_eq!(crc32::checksum_koopman(b"123456789"), KOOPMAN_CHECK_VALUE)
    }

    #[test]
    fn digest_castagnoli() {
        verify_checksum(crc32::CASTAGNOLI, CASTAGNOLI_CHECK_VALUE);
    }

    #[test]
    fn digest_ieee() {
        verify_checksum(crc32::IEEE, IEEE_CHECK_VALUE);
    }

    #[test]
    fn digest_koopman() {
        verify_checksum(crc32::KOOPMAN, KOOPMAN_CHECK_VALUE);
    }

    fn verify_checksum(poly: u32, check_value: u32) {
        let mut digest = crc32::Digest::new(poly);
        digest.write(b"123456789");
        assert_eq!(digest.sum32(), check_value);
        digest.reset();
        for i in 1..10 {
            digest.write(i.to_string().as_bytes());
        }
        assert_eq!(digest.sum32(), check_value);
    }
}

mod crc64 {
    use crc::{crc64, Hasher64};

    const ECMA_CHECK_VALUE: u64 = 0x995dc9bbdf1939fa;
    const ISO_CHECK_VALUE: u64 = 0xb90956c775a41001;

    #[test]
    fn checksum_ecma() {
        assert_eq!(crc64::checksum_ecma(b"123456789"), ECMA_CHECK_VALUE)
    }

    #[test]
    fn checksum_iso() {
        assert_eq!(crc64::checksum_iso(b"123456789"), ISO_CHECK_VALUE)
    }

    #[test]
    fn digest_ecma() {
        verify_checksum(crc64::ECMA, ECMA_CHECK_VALUE);
    }

    #[test]
    fn digest_iso() {
        verify_checksum(crc64::ISO, ISO_CHECK_VALUE);
    }

    fn verify_checksum(poly: u64, check_value: u64) {
        let mut digest = crc64::Digest::new(poly);
        digest.write(b"123456789");
        assert_eq!(digest.sum64(), check_value);
        digest.reset();
        for i in 1..10 {
            digest.write(i.to_string().as_bytes());
        }
        assert_eq!(digest.sum64(), check_value);
    }
}
