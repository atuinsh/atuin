use proc_macro2::Span;
use quote::quote;
use syn::{spanned::Spanned, DeriveInput, Error, Ident, Lit, Path, Result};

use crate::utils::get_attributes;

pub struct Container<'a> {
    pub crate_name: Path,
    pub name: &'a Ident,
    pub span: Span,
}

impl<'a> Container<'a> {
    pub fn new(input: &'a DeriveInput) -> Result<Self> {
        let container_attributes = get_attributes(&input.attrs)?;

        let mut crate_name = None;

        for (key, value) in container_attributes {
            if key.is_ident("crate") {
                crate_name = Some(match value {
                    Lit::Str(lit_str) => lit_str.parse::<Path>(),
                    bad => Err(Error::new_spanned(
                        bad,
                        "Invalid value for #[table(crate = \"crate_path\")]",
                    )),
                }?);
            }
        }

        let mut container_builder = Container::builder(&input.ident, input.span());

        if let Some(crate_name) = crate_name {
            container_builder.crate_name(crate_name);
        }

        Ok(container_builder.build())
    }

    fn builder(name: &'a Ident, span: Span) -> ContainerBuilder<'a> {
        ContainerBuilder::new(name, span)
    }
}

struct ContainerBuilder<'a> {
    crate_name: Option<Path>,
    name: &'a Ident,
    span: Span,
}

impl<'a> ContainerBuilder<'a> {
    pub fn new(name: &'a Ident, span: Span) -> Self {
        Self {
            crate_name: None,
            name,
            span,
        }
    }

    pub fn crate_name(&mut self, crate_name: Path) -> &mut Self {
        self.crate_name = Some(crate_name);
        self
    }

    pub fn build(self) -> Container<'a> {
        Container {
            crate_name: self
                .crate_name
                .unwrap_or_else(|| syn::parse2(quote!(::cli_table)).unwrap()),
            name: self.name,
            span: self.span,
        }
    }
}
