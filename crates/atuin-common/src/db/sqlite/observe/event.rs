use strum_macros::{Display, EnumDiscriminants, EnumIter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended<T>(pub T);

#[derive(Debug, Clone, PartialEq, Eq, EnumDiscriminants)]
#[strum_discriminants(name(ChangeKind), derive(Display, EnumIter, Hash))]
pub enum Change<T> {
    Inserted(T),
    Updated { old: T, new: T },
    Deleted(T),
}
