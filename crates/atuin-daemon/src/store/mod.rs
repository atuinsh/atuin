//! Store module for the daemon gRPC record-store service.
//!
//! Exposes the `atuin_client::record::store::Store` surface over gRPC so the
//! CLI never opens the local record store itself. Records stay encrypted on the
//! wire (the daemon is a blob store; crypto stays client-side).

use atuin_common::record::{
    EncryptedData as CommonEncryptedData, Host as CommonHost, HostId, Record, RecordId,
    RecordStatus,
};
use uuid::Uuid;

// Include the generated proto code
tonic::include_proto!("store");

fn parse_uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).unwrap_or(Uuid::nil())
}

impl From<CommonHost> for Host {
    fn from(h: CommonHost) -> Self {
        Self {
            id: h.id.0.as_hyphenated().to_string(),
            name: h.name,
        }
    }
}

impl From<Host> for CommonHost {
    fn from(h: Host) -> Self {
        Self {
            id: HostId(parse_uuid(&h.id)),
            name: h.name,
        }
    }
}

impl From<CommonEncryptedData> for EncryptedData {
    fn from(d: CommonEncryptedData) -> Self {
        Self {
            data: d.data,
            content_encryption_key: d.content_encryption_key,
        }
    }
}

impl From<EncryptedData> for CommonEncryptedData {
    fn from(d: EncryptedData) -> Self {
        Self {
            data: d.data,
            content_encryption_key: d.content_encryption_key,
        }
    }
}

impl From<Record<CommonEncryptedData>> for EncryptedRecord {
    fn from(r: Record<CommonEncryptedData>) -> Self {
        Self {
            id: r.id.0.as_hyphenated().to_string(),
            idx: r.idx,
            host: Some(r.host.into()),
            timestamp: r.timestamp,
            version: r.version,
            tag: r.tag,
            data: Some(r.data.into()),
        }
    }
}

impl From<EncryptedRecord> for Record<CommonEncryptedData> {
    fn from(r: EncryptedRecord) -> Self {
        Self {
            id: RecordId(parse_uuid(&r.id)),
            idx: r.idx,
            host: r.host.map(Into::into).unwrap_or_else(|| CommonHost {
                id: HostId(Uuid::nil()),
                name: String::new(),
            }),
            timestamp: r.timestamp,
            version: r.version,
            tag: r.tag,
            data: r.data.map(Into::into).unwrap_or(CommonEncryptedData {
                data: String::new(),
                content_encryption_key: String::new(),
            }),
        }
    }
}

impl Records {
    pub fn into_records(self) -> Vec<Record<CommonEncryptedData>> {
        self.records.into_iter().map(Into::into).collect()
    }
    pub fn from_records(records: Vec<Record<CommonEncryptedData>>) -> Self {
        Self {
            records: records.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<RecordStatus> for RecordStatusReply {
    fn from(s: RecordStatus) -> Self {
        Self {
            hosts: s
                .hosts
                .into_iter()
                .map(|(host, tags)| HostTags {
                    host: host.0.as_hyphenated().to_string(),
                    tags: tags.into_iter().collect(),
                })
                .collect(),
        }
    }
}

impl From<RecordStatusReply> for RecordStatus {
    fn from(s: RecordStatusReply) -> Self {
        let mut status = RecordStatus::new();
        for host in s.hosts {
            let host_id = HostId(parse_uuid(&host.host));
            for (tag, idx) in host.tags {
                status.set_raw(host_id, tag, idx);
            }
        }
        status
    }
}
