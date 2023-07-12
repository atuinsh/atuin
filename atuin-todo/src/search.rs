use atuin_common::record::Record;
use eyre::Result;
use tantivy::{
    directory::MmapDirectory,
    doc,
    schema::{Field, Schema, STORED, STRING, TEXT},
    DateTime, Index, IndexWriter, Term,
};

use crate::{record::TodoRecord, TodoId};

pub fn schema() -> (TodoSchema, Schema) {
    let mut schema_builder = Schema::builder();

    (
        TodoSchema {
            id: schema_builder.add_text_field("id", STRING | STORED),
            text: schema_builder.add_text_field("text", TEXT),
            timestamp: schema_builder.add_date_field("timestamp", STORED),
            tag: schema_builder.add_text_field("tag", TEXT),
        },
        schema_builder.build(),
    )
}

pub struct TodoSchema {
    pub id: Field,
    pub text: Field,
    pub timestamp: Field,
    pub tag: Field,
}

pub fn index(schema: Schema) -> Result<Index> {
    let data_dir = atuin_common::utils::data_dir().join("todo");
    let tantivy_dir = data_dir.join("tantivy");

    fs_err::create_dir_all(&tantivy_dir)?;
    let dir = MmapDirectory::open(tantivy_dir)?;

    Ok(Index::open_or_create(dir, schema)?)
}

pub fn write_record(
    writer: &mut IndexWriter,
    schema: &TodoSchema,
    record: &Record<TodoRecord>,
) -> Result<()> {
    let timestamp = DateTime::from_timestamp_nanos(record.timestamp as i64);
    let mut doc = doc!(
        schema.id => TodoId::from_uuid(record.id.0).to_string(),
        schema.text => record.data.text.clone(),
        schema.timestamp => timestamp,
    );
    for tag in record.data.tags.clone() {
        doc.add_field_value(schema.tag, tag)
    }
    writer.add_document(doc)?;

    if !record.data.updates.uuid().is_nil() {
        writer.delete_term(Term::from_field_text(
            schema.id,
            &record.data.updates.to_string(),
        ));
    }

    Ok(())
}
