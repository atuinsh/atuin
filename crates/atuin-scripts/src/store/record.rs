use super::script::Script;

pub enum ScriptRecord {
    Create(Script),
    Update(Script),
    Delete(Script),
}

impl ScriptRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        // probably don't actually need to use rmp here, but if we ever need to extend it, it's a
        // nice wrapper around raw byte stuff
        use rmp::encode;

        let mut output = vec![];

        match self {
            HistoryRecord::Create(history) => {
                // 0 -> a history create
                encode::write_u8(&mut output, 0)?;

                let bytes = history.serialize()?;

                encode::write_bin(&mut output, &bytes.0)?;
            }
            HistoryRecord::Delete(id) => {
                // 1 -> a history delete
                encode::write_u8(&mut output, 1)?;
                encode::write_str(&mut output, id.0.as_str())?;
            }
        };

        Ok(DecryptedData(output))
    }

}