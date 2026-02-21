use atuin_common::record::RecordId;

#[derive(Debug, Clone)]
pub enum DaemonEvent {
    RecordsAdded(Vec<RecordId>),
}
