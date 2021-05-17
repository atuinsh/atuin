mod container;
mod fields;

use syn::{DeriveInput, Result};

use self::{container::Container, fields::Fields};

pub struct Context<'a> {
    pub container: Container<'a>,
    pub fields: Fields,
}

impl<'a> Context<'a> {
    pub fn new(input: &'a DeriveInput) -> Result<Self> {
        let container = Container::new(input)?;
        let fields = Fields::new(input)?;

        Ok(Self { container, fields })
    }
}
