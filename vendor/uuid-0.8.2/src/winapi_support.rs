use crate::prelude::*;
use winapi::shared::guiddef;

#[cfg(feature = "guid")]
impl Uuid {
    /// Attempts to create a [`Uuid`] from a little endian winapi `GUID`
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    pub fn from_guid(guid: guiddef::GUID) -> Result<Uuid, crate::Error> {
        Uuid::from_fields_le(
            guid.Data1 as u32,
            guid.Data2 as u16,
            guid.Data3 as u16,
            &(guid.Data4 as [u8; 8]),
        )
    }

    /// Converts a [`Uuid`] into a little endian winapi `GUID`
    ///
    /// [`Uuid`]: ../struct.Uuid.html
    pub fn to_guid(&self) -> guiddef::GUID {
        let (data1, data2, data3, data4) = self.to_fields_le();

        guiddef::GUID {
            Data1: data1,
            Data2: data2,
            Data3: data3,
            Data4: *data4,
        }
    }
}

#[cfg(feature = "guid")]
#[cfg(test)]
mod tests {
    use super::*;

    use crate::std::string::ToString;
    use winapi::shared::guiddef;

    #[test]
    fn test_from_guid() {
        let guid = guiddef::GUID {
            Data1: 0x4a35229d,
            Data2: 0x5527,
            Data3: 0x4f30,
            Data4: [0x86, 0x47, 0x9d, 0xc5, 0x4e, 0x1e, 0xe1, 0xe8],
        };

        let uuid = Uuid::from_guid(guid).unwrap();
        assert_eq!(
            "9d22354a-2755-304f-8647-9dc54e1ee1e8",
            uuid.to_hyphenated().to_string()
        );
    }

    #[test]
    fn test_guid_roundtrip() {
        let guid_in = guiddef::GUID {
            Data1: 0x4a35229d,
            Data2: 0x5527,
            Data3: 0x4f30,
            Data4: [0x86, 0x47, 0x9d, 0xc5, 0x4e, 0x1e, 0xe1, 0xe8],
        };

        let uuid = Uuid::from_guid(guid_in).unwrap();
        let guid_out = uuid.to_guid();

        assert_eq!(
            (guid_in.Data1, guid_in.Data2, guid_in.Data3, guid_in.Data4),
            (
                guid_out.Data1,
                guid_out.Data2,
                guid_out.Data3,
                guid_out.Data4
            )
        );
    }
}
