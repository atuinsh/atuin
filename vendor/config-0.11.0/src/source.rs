use error::*;
use path;
use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use value::{Value, ValueKind};

/// Describes a generic _source_ of configuration properties.
pub trait Source: Debug {
    fn clone_into_box(&self) -> Box<dyn Source + Send + Sync>;

    /// Collect all configuration properties available from this source and return
    /// a HashMap.
    fn collect(&self) -> Result<HashMap<String, Value>>;

    fn collect_to(&self, cache: &mut Value) -> Result<()> {
        let props = match self.collect() {
            Ok(props) => props,
            Err(error) => {
                return Err(error);
            }
        };

        for (key, val) in &props {
            match path::Expression::from_str(key) {
                // Set using the path
                Ok(expr) => expr.set(cache, val.clone()),

                // Set diretly anyway
                _ => path::Expression::Identifier(key.clone()).set(cache, val.clone()),
            }
        }

        Ok(())
    }
}

impl Clone for Box<dyn Source + Send + Sync> {
    fn clone(&self) -> Box<dyn Source + Send + Sync> {
        self.clone_into_box()
    }
}

impl Source for Vec<Box<dyn Source + Send + Sync>> {
    fn clone_into_box(&self) -> Box<dyn Source + Send + Sync> {
        Box::new((*self).clone())
    }

    fn collect(&self) -> Result<HashMap<String, Value>> {
        let mut cache: Value = HashMap::<String, Value>::new().into();

        for source in self {
            source.collect_to(&mut cache)?;
        }

        if let ValueKind::Table(table) = cache.kind {
            Ok(table)
        } else {
            unreachable!();
        }
    }
}

impl<T> Source for Vec<T>
where
    T: Source + Sync + Send,
    T: Clone,
    T: 'static,
{
    fn clone_into_box(&self) -> Box<dyn Source + Send + Sync> {
        Box::new((*self).clone())
    }

    fn collect(&self) -> Result<HashMap<String, Value>> {
        let mut cache: Value = HashMap::<String, Value>::new().into();

        for source in self {
            source.collect_to(&mut cache)?;
        }

        if let ValueKind::Table(table) = cache.kind {
            Ok(table)
        } else {
            unreachable!();
        }
    }
}
