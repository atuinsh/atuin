use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::token::Comma;
use syn::{Attribute, DeriveInput, Field, Lit, Meta, MetaNameValue, NestedMeta, Variant};

macro_rules! assert_attribute {
    ($e:expr, $err:expr, $input:expr) => {
        if !$e {
            return Err(syn::Error::new_spanned($input, $err));
        }
    };
}

macro_rules! fail {
    ($t:expr, $m:expr) => {
        return Err(syn::Error::new_spanned($t, $m));
    };
}

macro_rules! try_set {
    ($i:ident, $v:expr, $t:expr) => {
        match $i {
            None => $i = Some($v),
            Some(_) => fail!($t, "duplicate attribute"),
        }
    };
}

pub struct TypeName {
    pub val: String,
    pub span: Span,
    /// Whether the old sqlx(rename) syntax was used instead of sqlx(type_name)
    pub deprecated_rename: bool,
}

impl TypeName {
    pub fn get(&self) -> TokenStream {
        let val = &self.val;
        if self.deprecated_rename {
            quote_spanned!(self.span=> {
                ::sqlx::_rename();
                #val
            })
        } else {
            quote! { #val }
        }
    }
}

#[derive(Copy, Clone)]
pub enum RenameAll {
    LowerCase,
    SnakeCase,
    UpperCase,
    ScreamingSnakeCase,
    KebabCase,
    CamelCase,
    PascalCase,
}

pub struct SqlxContainerAttributes {
    pub transparent: bool,
    pub type_name: Option<TypeName>,
    pub rename_all: Option<RenameAll>,
    pub repr: Option<Ident>,
}

pub struct SqlxChildAttributes {
    pub rename: Option<String>,
    pub default: bool,
}

pub fn parse_container_attributes(input: &[Attribute]) -> syn::Result<SqlxContainerAttributes> {
    let mut transparent = None;
    let mut repr = None;
    let mut type_name = None;
    let mut rename_all = None;

    for attr in input
        .iter()
        .filter(|a| a.path.is_ident("sqlx") || a.path.is_ident("repr"))
    {
        let meta = attr
            .parse_meta()
            .map_err(|e| syn::Error::new_spanned(attr, e))?;
        match meta {
            Meta::List(list) if list.path.is_ident("sqlx") => {
                for value in list.nested.iter() {
                    match value {
                        NestedMeta::Meta(meta) => match meta {
                            Meta::Path(p) if p.is_ident("transparent") => {
                                try_set!(transparent, true, value)
                            }

                            Meta::NameValue(MetaNameValue {
                                path,
                                lit: Lit::Str(val),
                                ..
                            }) if path.is_ident("rename_all") => {
                                let val = match &*val.value() {
                                    "lowercase" => RenameAll::LowerCase,
                                    "snake_case" => RenameAll::SnakeCase,
                                    "UPPERCASE" => RenameAll::UpperCase,
                                    "SCREAMING_SNAKE_CASE" => RenameAll::ScreamingSnakeCase,
                                    "kebab-case" => RenameAll::KebabCase,
                                    "camelCase" => RenameAll::CamelCase,
                                    "PascalCase" => RenameAll::PascalCase,
                                    _ => fail!(meta, "unexpected value for rename_all"),
                                };

                                try_set!(rename_all, val, value)
                            }

                            Meta::NameValue(MetaNameValue {
                                path,
                                lit: Lit::Str(val),
                                ..
                            }) if path.is_ident("type_name") => {
                                try_set!(
                                    type_name,
                                    TypeName {
                                        val: val.value(),
                                        span: value.span(),
                                        deprecated_rename: false
                                    },
                                    value
                                )
                            }

                            Meta::NameValue(MetaNameValue {
                                path,
                                lit: Lit::Str(val),
                                ..
                            }) if path.is_ident("rename") => {
                                try_set!(
                                    type_name,
                                    TypeName {
                                        val: val.value(),
                                        span: value.span(),
                                        deprecated_rename: true
                                    },
                                    value
                                )
                            }

                            u => fail!(u, "unexpected attribute"),
                        },
                        u => fail!(u, "unexpected attribute"),
                    }
                }
            }
            Meta::List(list) if list.path.is_ident("repr") => {
                if list.nested.len() != 1 {
                    fail!(&list.nested, "expected one value")
                }
                match list.nested.first().unwrap() {
                    NestedMeta::Meta(Meta::Path(p)) if p.get_ident().is_some() => {
                        try_set!(repr, p.get_ident().unwrap().clone(), list);
                    }
                    u => fail!(u, "unexpected value"),
                }
            }
            _ => {}
        }
    }

    Ok(SqlxContainerAttributes {
        transparent: transparent.unwrap_or(false),
        repr,
        type_name,
        rename_all,
    })
}

pub fn parse_child_attributes(input: &[Attribute]) -> syn::Result<SqlxChildAttributes> {
    let mut rename = None;
    let mut default = false;

    for attr in input.iter().filter(|a| a.path.is_ident("sqlx")) {
        let meta = attr
            .parse_meta()
            .map_err(|e| syn::Error::new_spanned(attr, e))?;

        if let Meta::List(list) = meta {
            for value in list.nested.iter() {
                match value {
                    NestedMeta::Meta(meta) => match meta {
                        Meta::NameValue(MetaNameValue {
                            path,
                            lit: Lit::Str(val),
                            ..
                        }) if path.is_ident("rename") => try_set!(rename, val.value(), value),
                        Meta::Path(path) if path.is_ident("default") => default = true,
                        u => fail!(u, "unexpected attribute"),
                    },
                    u => fail!(u, "unexpected attribute"),
                }
            }
        }
    }

    Ok(SqlxChildAttributes { rename, default })
}

pub fn check_transparent_attributes(
    input: &DeriveInput,
    field: &Field,
) -> syn::Result<SqlxContainerAttributes> {
    let attributes = parse_container_attributes(&input.attrs)?;

    assert_attribute!(
        attributes.rename_all.is_none(),
        "unexpected #[sqlx(rename_all = ..)]",
        field
    );

    assert_attribute!(attributes.repr.is_none(), "unexpected #[repr(..)]", input);

    let ch_attributes = parse_child_attributes(&field.attrs)?;

    assert_attribute!(
        ch_attributes.rename.is_none(),
        "unexpected #[sqlx(rename = ..)]",
        field
    );

    Ok(attributes)
}

pub fn check_enum_attributes(input: &DeriveInput) -> syn::Result<SqlxContainerAttributes> {
    let attributes = parse_container_attributes(&input.attrs)?;

    assert_attribute!(
        !attributes.transparent,
        "unexpected #[sqlx(transparent)]",
        input
    );

    Ok(attributes)
}

pub fn check_weak_enum_attributes(
    input: &DeriveInput,
    variants: &Punctuated<Variant, Comma>,
) -> syn::Result<SqlxContainerAttributes> {
    let attributes = check_enum_attributes(input)?;

    assert_attribute!(attributes.repr.is_some(), "expected #[repr(..)]", input);

    assert_attribute!(
        attributes.rename_all.is_none(),
        "unexpected #[sqlx(c = ..)]",
        input
    );

    for variant in variants {
        let attributes = parse_child_attributes(&variant.attrs)?;

        assert_attribute!(
            attributes.rename.is_none(),
            "unexpected #[sqlx(rename = ..)]",
            variant
        );
    }

    Ok(attributes)
}

pub fn check_strong_enum_attributes(
    input: &DeriveInput,
    _variants: &Punctuated<Variant, Comma>,
) -> syn::Result<SqlxContainerAttributes> {
    let attributes = check_enum_attributes(input)?;

    assert_attribute!(attributes.repr.is_none(), "unexpected #[repr(..)]", input);

    Ok(attributes)
}

pub fn check_struct_attributes<'a>(
    input: &'a DeriveInput,
    fields: &Punctuated<Field, Comma>,
) -> syn::Result<SqlxContainerAttributes> {
    let attributes = parse_container_attributes(&input.attrs)?;

    assert_attribute!(
        !attributes.transparent,
        "unexpected #[sqlx(transparent)]",
        input
    );

    assert_attribute!(
        attributes.rename_all.is_none(),
        "unexpected #[sqlx(rename_all = ..)]",
        input
    );

    assert_attribute!(attributes.repr.is_none(), "unexpected #[repr(..)]", input);

    for field in fields {
        let attributes = parse_child_attributes(&field.attrs)?;

        assert_attribute!(
            attributes.rename.is_none(),
            "unexpected #[sqlx(rename = ..)]",
            field
        );
    }

    Ok(attributes)
}
