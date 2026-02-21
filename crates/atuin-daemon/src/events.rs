use atuin_common::record::RecordId;

pub enum DaemonEvent {
    RecordsAdded(Vec<RecordId>),
}
