use typed_builder::TypedBuilder;

#[derive(Debug, Clone, PartialEq, Eq, TypedBuilder)]
pub struct KvEntry {
    pub namespace: String,
    pub key: String,
    pub value: String,
}
