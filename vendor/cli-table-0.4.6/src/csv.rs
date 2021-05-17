use std::{convert::TryFrom, io::Read};

use csv::{Error, Reader, StringRecord};

use crate::{Cell, RowStruct, Style, Table, TableStruct};

impl<R: Read> TryFrom<&mut Reader<R>> for TableStruct {
    type Error = Error;

    fn try_from(reader: &mut Reader<R>) -> Result<Self, Self::Error> {
        let records = reader.records();
        let rows = records
            .map(|record| Ok(row(&record?)))
            .collect::<Result<Vec<RowStruct>, Error>>()?;

        let table = if reader.has_headers() {
            let headers = reader.headers()?;
            let title: RowStruct = title(headers);

            rows.table().title(title)
        } else {
            rows.table()
        };

        Ok(table)
    }
}

fn row(record: &StringRecord) -> RowStruct {
    RowStruct {
        cells: record.iter().map(Cell::cell).collect(),
    }
}

fn title(record: &StringRecord) -> RowStruct {
    RowStruct {
        cells: record.iter().map(|cell| cell.cell().bold(true)).collect(),
    }
}
