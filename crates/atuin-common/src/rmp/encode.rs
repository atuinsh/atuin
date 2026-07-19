use rmp::encode::{RmpWrite, RmpWriteErr, ValueWriteError};

#[derive(Debug, derive_more::Display, derive_more::From, thiserror::Error)]
#[display("could not write MessagePack value: {_0:?}")]
pub struct EncodeError<E: RmpWriteErr = std::io::Error>(ValueWriteError<E>);

pub fn write_u8<W: RmpWrite>(out: &mut W, val: u8) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_u8(out, val)?;
    Ok(())
}

pub fn write_u16<W: RmpWrite>(out: &mut W, val: u16) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_u16(out, val)?;
    Ok(())
}

pub fn write_u64<W: RmpWrite>(out: &mut W, val: u64) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_u64(out, val)?;
    Ok(())
}

pub fn write_uint<W: RmpWrite>(out: &mut W, val: u64) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_uint(out, val)?;
    Ok(())
}

pub fn write_sint<W: RmpWrite>(out: &mut W, val: i64) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_sint(out, val)?;
    Ok(())
}

pub fn write_bool<W: RmpWrite>(out: &mut W, val: bool) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_bool(out, val)
        .map_err(|e| EncodeError::from(ValueWriteError::InvalidMarkerWrite(e)))
}

pub fn write_str<W: RmpWrite>(out: &mut W, data: &str) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_str(out, data)?;
    Ok(())
}

pub fn write_bin<W: RmpWrite>(out: &mut W, data: &[u8]) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_bin(out, data)?;
    Ok(())
}

pub fn write_array_len<W: RmpWrite>(out: &mut W, len: u32) -> Result<(), EncodeError<W::Error>> {
    rmp::encode::write_array_len(out, len)?;
    Ok(())
}

pub fn write_optional<W: RmpWrite, T>(
    out: &mut W,
    value: Option<T>,
    write: impl FnOnce(&mut W, T) -> Result<(), EncodeError<W::Error>>,
) -> Result<(), EncodeError<W::Error>> {
    match value {
        Some(v) => write(out, v),
        None => rmp::encode::write_nil(out)
            .map_err(|e| EncodeError::from(ValueWriteError::InvalidMarkerWrite(e))),
    }
}
